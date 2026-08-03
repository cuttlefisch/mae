//! Shared scaffolding for `tests/*.rs` files that spawn a real `mae
//! --headless` subprocess (ADR-055). NOT itself a test target: Cargo's
//! `tests/*.rs` auto-discovery only globs direct children of `tests/`, so
//! `tests/headless_test_support/mod.rs` is never picked up as its own
//! integration-test binary -- same precedent as
//! `collab_tcp_e2e_support/mod.rs` and `kb_graph_validation_support/`.
//!
//! **Why this exists.** Before this consolidation, `isolated_env`/
//! `send_sigterm`/`wait_for_exit`/`socket_is_live`/`wait_for_socket_live`
//! and a `HeadlessGuard` Drop-guard type were hand-copied, near-verbatim,
//! into 7 separate `tests/*.rs` files. Three of those independently drifted
//! into a real orphaned-process leak (found via an inotify-instance
//! exhaustion incident: 10 orphaned `mae --headless` processes had eaten
//! the machine's `fs.inotify.max_user_instances`, so a freshly spawned
//! headless instance couldn't start its file watchers, never bound its
//! socket, failed the test, and leaked ANOTHER child -- self-amplifying):
//!
//!   - Two files (`headless_idle_cpu_e2e.rs`, `headless_soak_shaped_e2e.rs`)
//!     had **no Drop guard at all** -- a bare `Child`, killed only on the
//!     happy path at the very end of the test function. Any panic before
//!     that point (including the very first `assert!` right after spawn)
//!     leaked the process.
//!   - One file (`guidance_delivery_e2e.rs`) HAD a correctly-shaped
//!     `HeadlessGuard`, but every test defeated it with
//!     `drop(guard.child.take())` -- `Child::drop` sends no signal, so that
//!     "cleanup" was a silent no-op on every SUCCESSFUL run, not just
//!     failures.
//!   - One file (`headless_kb_convergence_e2e.rs`) constructed its guard
//!     type only at the END of a helper function, AFTER several fallible
//!     operations (a sock-live wait, a connect, an MCP handshake) had
//!     already run against the bare, unguarded `Child`.
//!
//! Per CLAUDE.md principle #8 (shared computation, no duplicated logic) and
//! #15 (bugs are drift signals -- fix duplicated logic by consolidation, not
//! a third/fourth/fifth parallel reimplementation): this module is the one
//! place the spawn/guard/teardown logic lives now. `HeadlessGuard::shutdown`
//! is the ONLY teardown path -- both `Drop` and any test body that wants an
//! exit status call the exact same method, so there is no longer a distinct
//! "manual cleanup" spelling for a future test to accidentally get wrong.
//!
//! **Contract for callers:** construct `HeadlessGuard::new(child)`
//! immediately after `spawn()` returns, with nothing fallible in between.
//! Every fallible step after that (`wait_for_socket_live`, MCP handshakes,
//! assertions) is then covered by the guard's `Drop` impl no matter how the
//! test function exits (return, panic, or an early `?`).

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Isolated per-test `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`HOME` (never touches
/// a real user's config) + the death-signal backstop below.
pub fn isolated_env(cmd: &mut Command, xdg_config: &Path, xdg_data: &Path, home: &Path) {
    cmd.env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_DATA_HOME", xdg_data)
        .env("HOME", home)
        .env("SHELL", "/bin/sh")
        .env("MAE_SKIP_WIZARD", "1");
    arm_pdeathsig(cmd);
}

/// Belt-and-braces backstop for the case that actually bit us: a leaked
/// child that outlives the TEST BINARY itself, not just the test function --
/// e.g. the harness running `cargo test` is hard-killed or OOM-killed, or a
/// test aborts without unwinding. None of those run any `Drop` impl, so the
/// `HeadlessGuard` below cannot help; this is the only mechanism that can.
///
/// `PR_SET_PDEATHSIG` asks the kernel to deliver a signal to THIS child
/// specifically when its parent (the test binary process) dies, by any
/// means. Set inside `pre_exec` so it runs in the forked child, post-fork,
/// pre-exec -- the standard, only-correct place for it (this file already
/// has local precedent for `pre_exec`-based process setup in
/// `mcp_event_loop_integration.rs`).
///
/// Linux-only: `PR_SET_PDEATHSIG` has no direct macOS equivalent (the
/// nearest analog, `EVFILT_PROC`/`NOTE_EXIT` via kqueue, watches a *known*
/// pid from a *separate* watcher process -- a materially different
/// mechanism, not a drop-in substitute). This is not a silent cross-platform
/// gap: this entire support module -- and every one of its callers -- is
/// already `#![cfg(target_os = "linux")]`-gated for independent reasons
/// (real SIGTERM/`/proc` dependencies), so there is no macOS build of these
/// tests for this backstop to be missing from. Principle #13 is about not
/// silently degrading a mechanism that's supposed to run on both platforms;
/// this one was never supposed to run on macOS in the first place.
#[cfg(target_os = "linux")]
fn arm_pdeathsig(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent_pid = std::process::id();
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Close the fork/prctl race: if the parent already exited
            // between fork() and this prctl() call, the signal was never
            // armed in time to fire on that exit. Detect the reparent (to
            // init/a subreaper) and self-terminate instead of relying on a
            // signal that has already been missed.
            if libc::getppid() != parent_pid as libc::pid_t {
                libc::raise(libc::SIGKILL);
            }
            Ok(())
        });
    }
}

pub fn send_sigterm(child: &Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

pub fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn socket_is_live(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

pub fn wait_for_socket_live(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if socket_is_live(path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// RAII guard that owns a spawned `mae --headless` `Child` from the moment
/// it's constructed. `shutdown()` is the single, idempotent teardown path --
/// `Drop` calls it, and a test body that needs the exit status (to assert
/// clean shutdown) calls the exact same method rather than reimplementing
/// SIGTERM/wait/escalate itself. There is deliberately no way to pull the
/// `Child` back out of the guard (no `take()`-like accessor) -- that
/// accessor is exactly what let `guidance_delivery_e2e.rs` bypass cleanup
/// via `drop(guard.child.take())` (dropping the bare `Child` sends no
/// signal at all).
pub struct HeadlessGuard {
    child: Option<Child>,
}

impl HeadlessGuard {
    /// Construct immediately after `spawn()` returns -- nothing fallible
    /// (an `assert!`, a `.unwrap()`, an `.expect()`) should run between the
    /// `spawn()` call and this constructor, or the child is unguarded for
    /// that window.
    pub fn new(child: Child) -> Self {
        HeadlessGuard { child: Some(child) }
    }

    pub fn pid(&self) -> u32 {
        self.child.as_ref().expect("guard already shut down").id()
    }

    /// Disarm this guard and hand back the raw `Child` for a caller that is
    /// itself about to become responsible for killing it -- e.g. wrapping it
    /// in another owning type that has its own `Drop` (this file's own
    /// `HeadlessInstance` in `headless_kb_convergence_e2e.rs` does exactly
    /// this: it needs to own the `Child` directly alongside an MCP stream so
    /// one combined `Drop` impl can clean up both).
    ///
    /// This is NOT the `drop(guard.child.take())` anti-pattern it replaces:
    /// that pulled the `Child` out through direct field access and then
    /// dropped the bare value with no signal sent at all, silently skipping
    /// cleanup. `into_child` leaves nothing behind for this guard to own (its
    /// own `Drop` becomes a no-op) and hands the child to a caller that MUST
    /// immediately take over ownership responsibility -- there is no window
    /// where the child is unowned by anything with a `Drop` impl, as long as
    /// the caller wraps it before doing anything else fallible.
    pub fn into_child(mut self) -> Child {
        self.child.take().expect("guard already shut down")
    }

    /// SIGTERM, wait up to `timeout`, escalate to SIGKILL if still alive
    /// after that. Always reaps the child before returning. Idempotent: a
    /// second call (including the one `Drop` makes) is a no-op once the
    /// child is already gone.
    pub fn shutdown(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let mut child = self.child.take()?;
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        send_sigterm(&child);
        match wait_for_exit(&mut child, timeout) {
            Some(status) => Some(status),
            None => {
                let _ = child.kill();
                child.wait().ok()
            }
        }
    }
}

impl Drop for HeadlessGuard {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(3));
    }
}

/// Boots a real isolated `mae --headless` instance for `project_root` and
/// returns the live socket path + a guard that has owned the child since
/// the instant `spawn()` returned. `stderr_log`, when given, captures
/// stderr to a file instead of `Stdio::null()` so a bind-failure message
/// has real signal to show instead of silence.
pub fn spawn_isolated_headless(
    project_root: &Path,
    xdg_config: &Path,
    xdg_data: &Path,
    home: &Path,
    stderr_log: Option<&Path>,
) -> (PathBuf, HeadlessGuard) {
    let mae = env!("CARGO_BIN_EXE_mae");

    let mut print_cmd = Command::new(mae);
    print_cmd
        .args(["--headless", "--print-socket-path"])
        .current_dir(project_root);
    isolated_env(&mut print_cmd, xdg_config, xdg_data, home);
    let print_output = print_cmd
        .output()
        .expect("failed to run `mae --headless --print-socket-path`");
    assert!(print_output.status.success());
    let socket_path = PathBuf::from(
        String::from_utf8_lossy(&print_output.stdout)
            .trim()
            .to_string(),
    );

    let mut spawn_cmd = Command::new(mae);
    spawn_cmd.args(["--headless"]).current_dir(project_root);
    match stderr_log {
        Some(path) => {
            let file = std::fs::File::create(path).expect("create stderr log file");
            spawn_cmd.stdout(Stdio::null()).stderr(file);
        }
        None => {
            spawn_cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    isolated_env(&mut spawn_cmd, xdg_config, xdg_data, home);
    let child = spawn_cmd.spawn().expect("failed to spawn `mae --headless`");
    // Guard constructed the instant spawn() returns -- nothing fallible ran
    // in between. Everything below (the socket-live wait, which is the most
    // failure-prone step in every one of these tests) is now covered by
    // `guard`'s `Drop` regardless of how this function's caller's test ends.
    let guard = HeadlessGuard::new(child);

    let bound = wait_for_socket_live(&socket_path, Duration::from_secs(30));
    if !bound {
        if let Some(path) = stderr_log {
            let log = std::fs::read_to_string(path)
                .unwrap_or_else(|e| format!("<failed to read stderr log: {e}>"));
            eprintln!("=== headless stderr ===\n{log}\n=== end ===");
        }
    }
    assert!(
        bound,
        "headless instance never bound its stable socket at {}",
        socket_path.display()
    );

    (socket_path, guard)
}
