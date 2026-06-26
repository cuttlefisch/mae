//! On-demand daemon spawn + readiness (ADR-035 `daemon_mode`).
//!
//! When `daemon_mode = on-demand` and no daemon is already listening, the editor
//! spawns + supervises a co-located `mae-daemon` (the `emacsclient -a ''` model):
//! the user gets persistence/collab without ceremony, and the editor owns the
//! lifecycle. `shared` never spawns (it attaches to an OS-supervised/remote
//! daemon); `off` is the in-process floor. This module owns the startup spawn +
//! readiness handshake AND session-long supervision (restart-on-crash with a
//! circuit-breaker, driven by the ~30s health-check tick).
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

/// Resolve the `mae-daemon` binary: an explicit `MAE_DAEMON_BIN` override first
/// (ops/packaging escape hatch + how integration tests point at the built daemon),
/// then the one sitting next to the running editor (a release installs them side
/// by side, so an on-demand daemon matches the editor that spawns it — the
/// version-pin precondition), then `PATH`. Returns a bare `mae-daemon` when the
/// exe path can't be determined.
pub fn resolve_daemon_binary() -> PathBuf {
    if let Some(bin) = std::env::var_os("MAE_DAEMON_BIN") {
        return PathBuf::from(bin);
    }
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

/// Spawn a co-located `mae-daemon`, detached, without waiting for readiness.
/// Returns its pid. The child outlives this process's attention (it has its own
/// KB persistence + listeners); we silence stdout and inherit stderr for its
/// logs. `bare mae-daemon` brings up the KB Unix socket + collab listeners.
/// Fast + non-blocking — safe to call from a UI tick (the supervision watchdog).
pub fn spawn_daemon_process() -> Result<u32, String> {
    let binary = resolve_daemon_binary();
    let mut cmd = std::process::Command::new(&binary);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not launch {}: {e}", binary.display()))?;
    Ok(child.id())
}

/// Spawn a co-located `mae-daemon` and wait (bounded) until it answers on
/// `socket`. On readiness-timeout we return an error and the caller falls back to
/// the in-process KB. Synchronous: called from the editor's (sync) startup before
/// the daemon attach.
pub fn spawn_and_wait_ready(socket: &Path, ready_timeout: Duration) -> Result<(), String> {
    let pid = spawn_daemon_process()?;
    info!(pid, "spawned on-demand mae-daemon; awaiting readiness");

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

// --- Session-long supervision (ADR-035 PR B2) --------------------------------
//
// The editor owns the `on-demand` daemon it spawned, so it restarts it if it
// dies mid-session — but with a circuit-breaker so a daemon that won't stay up
// can't respawn-loop. A periodic health-check tick (~30s, shared by the GUI +
// TUI loops) drives `supervise_daemon`. `shared`/`off` are never supervised:
// `shared` is OS-managed (systemd/launchd), `off` has no daemon.

/// Max consecutive failed-to-stay-up restarts before the breaker opens.
pub const MAX_DAEMON_RESTARTS: u32 = 5;

/// What the watchdog should do this tick, given the daemon's liveness + how many
/// restarts have already failed to stick. Pure + unit-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperviseAction {
    /// Not an on-demand daemon — nothing to supervise.
    NotOwned,
    /// Daemon is alive — reset the failure counter.
    Healthy,
    /// Daemon is down and within budget — re-spawn it.
    Restart,
    /// Daemon is down but the breaker is open — stop trying.
    CircuitOpen,
}

/// Decide the supervision action. `responds` is the daemon's current liveness;
/// `failures` is the consecutive-restart counter.
pub fn supervise_decision(mode: DaemonMode, responds: bool, failures: u32) -> SuperviseAction {
    if mode != DaemonMode::OnDemand {
        return SuperviseAction::NotOwned;
    }
    if responds {
        return SuperviseAction::Healthy;
    }
    if failures >= MAX_DAEMON_RESTARTS {
        return SuperviseAction::CircuitOpen;
    }
    SuperviseAction::Restart
}

/// Periodic supervision tick for an on-demand daemon (call from the ~30s
/// health-check in both the GUI and TUI loops). Probes liveness and, if the
/// daemon we own has died, re-spawns it (bounded by `MAX_DAEMON_RESTARTS`); the
/// existing collab reconnect loop re-establishes the session once it's back.
/// Best-effort + non-blocking: the probe is a fast local-socket connect and the
/// re-spawn is detached (no readiness wait on the UI thread).
pub fn supervise_daemon(editor: &mut mae_core::Editor) {
    let mode = editor.kb.daemon_mode;
    if mode != DaemonMode::OnDemand {
        return;
    }
    let socket = editor.kb.daemon_socket.clone();
    let responds = daemon_responds(&socket, Duration::from_millis(750));
    match supervise_decision(mode, responds, editor.kb.daemon_restart_failures) {
        SuperviseAction::NotOwned => {}
        SuperviseAction::Healthy => {
            // Stable again — clear the counter and any prior circuit-open notice.
            if editor.kb.daemon_restart_failures > 0 {
                editor.kb.daemon_restart_failures = 0;
            }
        }
        SuperviseAction::Restart => {
            editor.kb.daemon_restart_failures += 1;
            match spawn_daemon_process() {
                Ok(pid) => {
                    info!(
                        pid,
                        attempt = editor.kb.daemon_restart_failures,
                        "on-demand mae-daemon was down — re-spawned it"
                    );
                    editor.notify(
                        mae_core::notifications::Notification::info("collab", "Daemon restarted")
                            .key("daemon:connection")
                            .body("The on-demand daemon had stopped; restarted it."),
                    );
                }
                Err(e) => {
                    warn!(error = %e, "on-demand mae-daemon re-spawn failed");
                }
            }
        }
        SuperviseAction::CircuitOpen => {
            // Raise once (keyed); the editor keeps working on the in-process floor.
            editor.notify(
                mae_core::notifications::Notification::warning(
                    "collab",
                    "Daemon keeps stopping — auto-restart paused",
                )
                .key("daemon:supervise:circuit")
                .body(
                    "The on-demand daemon failed to stay up after several restarts; \
                     auto-restart is paused. The editor works locally; restart it \
                     manually with `mae setup-daemon` once resolved.",
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervise_decision_matrix() {
        use SuperviseAction::*;
        // Not on-demand → never supervised.
        assert_eq!(supervise_decision(DaemonMode::Off, false, 0), NotOwned);
        assert_eq!(supervise_decision(DaemonMode::Shared, false, 9), NotOwned);
        // On-demand + alive → healthy (resets).
        assert_eq!(supervise_decision(DaemonMode::OnDemand, true, 3), Healthy);
        // On-demand + dead + within budget → restart.
        assert_eq!(supervise_decision(DaemonMode::OnDemand, false, 0), Restart);
        assert_eq!(
            supervise_decision(DaemonMode::OnDemand, false, MAX_DAEMON_RESTARTS - 1),
            Restart
        );
        // On-demand + dead + budget exhausted → circuit open (stop respawning).
        assert_eq!(
            supervise_decision(DaemonMode::OnDemand, false, MAX_DAEMON_RESTARTS),
            CircuitOpen
        );
        assert_eq!(
            supervise_decision(DaemonMode::OnDemand, false, MAX_DAEMON_RESTARTS + 5),
            CircuitOpen
        );
    }

    #[test]
    fn supervise_daemon_is_inert_when_not_on_demand() {
        // Off mode: no probe, no spawn, no counter change.
        let mut ed = mae_core::Editor::new();
        ed.kb.daemon_mode = DaemonMode::Off;
        ed.kb.daemon_restart_failures = 2;
        supervise_daemon(&mut ed);
        assert_eq!(ed.kb.daemon_restart_failures, 2, "off mode is untouched");
    }

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
        // Either a real sibling path (absolute), a MAE_DAEMON_BIN override, or the
        // bare PATH fallback.
        let b = resolve_daemon_binary();
        assert!(
            b.is_absolute() || b == Path::new("mae-daemon"),
            "unexpected daemon binary path: {}",
            b.display()
        );
    }

    // --- Real-daemon test harness (ADR-035, #136) ------------------------------
    //
    // The pure decision fns above are necessary but not sufficient: the audited gap
    // was that NOTHING spawned a real daemon + attached + supervised it. The harness
    // below drives the actual spawn → readiness → status → restart → circuit path
    // against a REAL mae-daemon — RELIABLY. Three loose ends, all closed here:
    //   1. ISOLATION: a temp XDG_RUNTIME_DIR gives a unique Unix socket, and a
    //      collab-disabled config means the daemon never binds the FIXED TCP port
    //      9473 (which would collide with the user's daemon AND our own restarts).
    //   2. NO LEAKS: the supervisor spawns daemons DETACHED, and `daemon/shutdown`
    //      only closes the connection (it does not stop the process) — so cleanup
    //      cannot rely on an RPC or a Child handle. `DaemonTestEnv` REAPS every
    //      daemon under its temp XDG on Drop (RAII), so a panicking test never
    //      leaks a process.
    //   3. NO RACES: it holds a process-wide env lock for its lifetime.
    //
    // Linux-gated (the reaper scans /proc for the temp-XDG marker); the supervisor's
    // pure logic is covered cross-platform by the tests above. Skips cleanly when
    // the daemon binary isn't built, so `cargo test -p mae` stays green without it;
    // the CI Server-Client job builds the daemon + sets MAE_DAEMON_BIN to run it.

    #[cfg(target_os = "linux")]
    fn find_daemon_binary() -> Option<PathBuf> {
        if let Ok(bin) = std::env::var("MAE_DAEMON_BIN") {
            let p = PathBuf::from(bin);
            return p.exists().then_some(p);
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        [
            "daemon/target/debug/mae-daemon",
            "daemon/target/release/mae-daemon",
            "target/debug/mae-daemon",
            "target/release/mae-daemon",
        ]
        .into_iter()
        .map(|r| root.join(r))
        .find(|p| p.exists())
    }

    /// Poll `daemon_responds` until it matches `want` or the deadline passes.
    #[cfg(target_os = "linux")]
    fn wait_for(socket: &Path, want: bool, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            if daemon_responds(socket, Duration::from_millis(500)) == want {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    #[cfg(target_os = "linux")]
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A leak-proof, fully-isolated environment for real-daemon tests: temp XDG (a
    /// unique socket, collab disabled so no 9473 collision), `MAE_DAEMON_BIN` set,
    /// and a `/proc` reaper that kills every daemon under this temp XDG on Drop —
    /// so no test ever leaks a daemon, even on a panicking assertion. Holds the env
    /// lock for its lifetime so parallel tests can't race on the process env.
    #[cfg(target_os = "linux")]
    struct DaemonTestEnv {
        _tmp: tempfile::TempDir,
        runtime: PathBuf,
        socket: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(target_os = "linux")]
    impl DaemonTestEnv {
        /// `None` when the daemon binary isn't built — the caller skips cleanly.
        fn new() -> Option<Self> {
            let bin = find_daemon_binary()?;
            // Tolerate a poisoned lock (a prior test panicked) — we still serialize.
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let runtime = tmp.path().join("run");
            let config = tmp.path().join("config");
            std::fs::create_dir_all(&runtime).unwrap();
            let cfg_dir = config.join("mae");
            std::fs::create_dir_all(&cfg_dir).unwrap();
            std::fs::write(cfg_dir.join("daemon.toml"), "[collab]\nenabled = false\n").unwrap();
            std::env::set_var("XDG_RUNTIME_DIR", &runtime);
            std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
            std::env::set_var("XDG_CONFIG_HOME", &config);
            std::env::set_var("MAE_DAEMON_BIN", &bin);
            let socket = runtime.join("mae-daemon.sock");
            Some(Self {
                _tmp: tmp,
                runtime,
                socket,
                _lock: lock,
            })
        }

        fn socket(&self) -> &Path {
            &self.socket
        }

        /// Kill every process whose `XDG_RUNTIME_DIR` is this env's temp dir — the
        /// reliable reclaim for the supervisor's DETACHED spawns (there is no Child
        /// handle, and `daemon/shutdown` only closes the connection). Returns the
        /// number reaped, then waits for the socket to go quiet.
        fn reap(&self) -> usize {
            let marker = format!("XDG_RUNTIME_DIR={}", self.runtime.display());
            let mut killed = 0;
            if let Ok(entries) = std::fs::read_dir("/proc") {
                for e in entries.flatten() {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                        continue;
                    }
                    // /proc/<pid>/environ is NUL-separated KEY=VALUE.
                    if let Ok(environ) = std::fs::read(e.path().join("environ")) {
                        if environ.split(|&b| b == 0).any(|kv| kv == marker.as_bytes()) {
                            let _ = std::process::Command::new("kill")
                                .args(["-9", &name])
                                .status();
                            killed += 1;
                        }
                    }
                }
            }
            wait_for(&self.socket, false, Duration::from_secs(5));
            killed
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for DaemonTestEnv {
        fn drop(&mut self) {
            // RAII reclaim — runs even when the test body panicked (no leaked daemon).
            self.reap();
            std::env::remove_var("XDG_RUNTIME_DIR");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("MAE_DAEMON_BIN");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_lifecycle_e2e_spawn_supervise_circuit() {
        let Some(env) = DaemonTestEnv::new() else {
            eprintln!("SKIP daemon_lifecycle_e2e: mae-daemon not built (set MAE_DAEMON_BIN)");
            return;
        };
        let socket = env.socket().to_path_buf();
        // No catch_unwind needed — `env`'s Drop reaper reclaims on panic too.

        // 1. Nothing is listening on a fresh temp socket.
        assert!(
            !daemon_responds(&socket, Duration::from_millis(500)),
            "starts clean"
        );

        // 2. on-demand ensure spawns a REAL daemon + reaches readiness.
        assert!(
            ensure_on_demand_daemon(DaemonMode::OnDemand, &socket),
            "ensure_on_demand_daemon spawns + reaches readiness"
        );
        assert!(
            wait_for(&socket, true, Duration::from_secs(10)),
            "daemon up"
        );

        // 3. The real daemon/status answers + reports its version (the version-skew
        //    handshake's data source — end-to-end vs the #124 unit test).
        let mut c = mae_mcp::daemon_client::DaemonClient::new(&socket);
        c.set_timeout(Duration::from_secs(2));
        c.connect().expect("connect to the live daemon");
        let status = c.call("daemon/status", serde_json::json!({})).unwrap();
        assert!(
            status.get("version").and_then(|v| v.as_str()).is_some(),
            "daemon/status reports a version: {status}"
        );
        drop(c);

        // 4. Supervision while healthy resets a prior failure count.
        let mut editor = mae_core::Editor::new();
        editor.kb.daemon_mode = DaemonMode::OnDemand;
        editor.kb.daemon_socket = socket.clone();
        editor.kb.daemon_restart_failures = 3;
        supervise_daemon(&mut editor);
        assert_eq!(
            editor.kb.daemon_restart_failures, 0,
            "healthy ⇒ counter reset"
        );

        // 5. KILL it (reliably, via the reaper) → supervision RESTARTS it (the
        //    watchdog — the audited gap).
        assert!(env.reap() >= 1, "reaped the running daemon");
        assert!(
            wait_for(&socket, false, Duration::from_secs(5)),
            "daemon down after reap"
        );
        supervise_daemon(&mut editor);
        assert_eq!(
            editor.kb.daemon_restart_failures, 1,
            "down ⇒ one restart counted"
        );
        assert!(
            wait_for(&socket, true, Duration::from_secs(10)),
            "watchdog re-spawned the daemon"
        );

        // 6. Circuit-breaker: at the limit, a down daemon is NOT re-spawned.
        editor.kb.daemon_restart_failures = MAX_DAEMON_RESTARTS;
        env.reap();
        let notifs_before = editor.notifications.outstanding_count();
        supervise_daemon(&mut editor);
        assert_eq!(
            editor.kb.daemon_restart_failures, MAX_DAEMON_RESTARTS,
            "circuit open ⇒ no further restart attempt"
        );
        assert!(
            !daemon_responds(&socket, Duration::from_millis(500)),
            "circuit open ⇒ daemon stays down (not respawned)"
        );
        assert!(
            editor.notifications.outstanding_count() > notifs_before,
            "circuit open raises a sticky warning"
        );
        // `env` drops here → reaper reclaims any survivor + restores the env.
    }
}
