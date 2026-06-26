//! On-demand daemon spawn + readiness (ADR-035 `daemon_mode`).
//!
//! When `daemon_mode = on-demand` and no daemon is already listening, the editor
//! spawns + supervises a co-located `mae-daemon` (the `emacsclient -a ''` model):
//! the user gets persistence/collab without ceremony, and the editor owns the
//! lifecycle. `shared` never spawns (it attaches to an OS-supervised/remote
//! daemon); `off` is the in-process floor. This module owns the *startup* spawn
//! decision + the readiness handshake; session-long restart/supervision is a
//! follow-up.
//!
//! Cross-platform (principle #13): the daemon binary is resolved next to the
//! running editor first (a release ships them together), then `PATH`; the socket
//! is the same XDG-resolved path both sides agree on.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mae_core::DaemonMode;
use tracing::{info, warn};

/// The startup spawn decision, kept pure so it is unit-testable without a process
/// or socket. The editor spawns a co-located daemon **only** for `on-demand` when
/// nothing is already listening — `shared` attaches but never spawns, `off` is
/// the in-process floor, and a responding daemon is just attached to.
pub fn should_spawn(mode: DaemonMode, daemon_responds: bool) -> bool {
    matches!(mode, DaemonMode::OnDemand) && !daemon_responds
}

/// Should the editor route KB *reads* through the daemon (attach the LRU read
/// layer)? Only when the daemon actually hosts the primary KB (`primary_exists`),
/// OR when the editor started thin and therefore has no local mirror to fall back
/// on (`primary_thin`). A freshly spawned/empty daemon (e.g. on-demand first
/// launch) hosts nothing, so routing reads to it would shadow the local KB with
/// emptiness — in that case keep reads local. Pure + unit-tested.
pub fn should_attach_daemon_reads(primary_exists: bool, primary_thin: bool) -> bool {
    primary_exists || primary_thin
}

/// Resolve the `mae-daemon` binary: prefer the one sitting next to the running
/// editor (a release installs them side by side, so an on-demand daemon matches
/// the editor that spawns it — the version-pin precondition), then fall back to
/// `PATH`. Returns a bare `mae-daemon` when the exe path can't be determined.
pub fn resolve_daemon_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("mae-daemon");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("mae-daemon")
}

/// Does a daemon already answer on `socket`? A short connect + `daemon/status`
/// round-trip, bounded so a wedged socket can't hang startup. Mirrors the Phase
/// D3 host probe; kept here so the spawn path has a single readiness predicate.
pub fn daemon_responds(socket: &Path, timeout: Duration) -> bool {
    let mut client = mae_mcp::daemon_client::DaemonClient::new(socket);
    client.set_timeout(timeout);
    if client.connect().is_err() {
        return false;
    }
    client.call("daemon/status", serde_json::json!({})).is_ok()
}

/// Spawn a co-located `mae-daemon` and wait (bounded) until it answers on
/// `socket`. The child is detached (its own KB persistence + collab listeners
/// outlive nothing here — we just need it up); on readiness-timeout we return an
/// error and the caller falls back to the in-process KB. Synchronous: called from
/// the editor's (sync) startup before the daemon attach.
pub fn spawn_and_wait_ready(socket: &Path, ready_timeout: Duration) -> Result<(), String> {
    let binary = resolve_daemon_binary();
    // Inherit stderr (the daemon logs there); silence stdout. No `start` arg —
    // bare `mae-daemon` brings up the KB Unix socket + collab listeners.
    let mut cmd = std::process::Command::new(&binary);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not launch {}: {e}", binary.display()))?;
    let pid = child.id();
    info!(pid, binary = %binary.display(), "spawned on-demand mae-daemon; awaiting readiness");

    // Poll until the daemon answers or we exhaust the budget. 150ms cadence keeps
    // startup snappy without hammering the socket.
    let deadline = Instant::now() + ready_timeout;
    let poll = Duration::from_millis(150);
    while Instant::now() < deadline {
        if daemon_responds(socket, Duration::from_millis(500)) {
            info!(pid, "on-demand mae-daemon is ready");
            return Ok(());
        }
        std::thread::sleep(poll);
    }
    warn!(
        pid,
        timeout_ms = ready_timeout.as_millis(),
        "on-demand mae-daemon did not become ready in time"
    );
    Err(format!(
        "mae-daemon (pid {pid}) did not answer on {} within {:?}",
        socket.display(),
        ready_timeout
    ))
}

/// Startup entry point: for `on-demand`, ensure a daemon is running — attach to a
/// live one, else spawn + wait. Returns `true` when a daemon is available
/// afterward (so the caller can proceed to attach), `false` when none is (caller
/// uses the in-process floor). `off`/`shared` make no spawn decision here:
/// `shared` is handled by the normal attach path, `off` never wants one.
pub fn ensure_on_demand_daemon(mode: DaemonMode, socket: &Path) -> bool {
    let responds = daemon_responds(socket, Duration::from_millis(750));
    if !should_spawn(mode, responds) {
        return responds;
    }
    match spawn_and_wait_ready(socket, Duration::from_secs(5)) {
        Ok(()) => true,
        Err(e) => {
            warn!(error = %e, "on-demand daemon unavailable; falling back to in-process KB");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_spawn_only_on_demand_when_absent() {
        // on-demand + nothing listening → spawn.
        assert!(should_spawn(DaemonMode::OnDemand, false));
        // on-demand + already up → attach, don't spawn.
        assert!(!should_spawn(DaemonMode::OnDemand, true));
        // shared never spawns (attaches to an externally-managed daemon).
        assert!(!should_spawn(DaemonMode::Shared, false));
        assert!(!should_spawn(DaemonMode::Shared, true));
        // off is the in-process floor.
        assert!(!should_spawn(DaemonMode::Off, false));
        assert!(!should_spawn(DaemonMode::Off, true));
    }

    #[test]
    fn attach_reads_only_when_hosted_or_thin() {
        // Daemon hosts the primary → route reads through it.
        assert!(should_attach_daemon_reads(true, false));
        // Thin startup (no local mirror) → must use the daemon even if the probe
        // momentarily says no.
        assert!(should_attach_daemon_reads(false, true));
        // Fresh/empty daemon + a local mirror present → keep reads local (the fix:
        // don't shadow the local KB with an empty daemon).
        assert!(!should_attach_daemon_reads(false, false));
        // Both → attach.
        assert!(should_attach_daemon_reads(true, true));
    }

    #[test]
    fn resolve_binary_is_absolute_or_bare() {
        // Either a real sibling path (absolute) or the bare PATH fallback.
        let b = resolve_daemon_binary();
        assert!(
            b.is_absolute() || b == Path::new("mae-daemon"),
            "unexpected daemon binary path: {}",
            b.display()
        );
    }
}
