//! Adversarial regression test for the process-leak defect fixed alongside
//! this file: several `tests/*.rs` files that spawn a real `mae --headless`
//! subprocess left it unguarded (or defeated an existing guard) for part or
//! all of the test, so a panicking assertion -- or in one file's case, even
//! a SUCCESSFUL run -- leaked the process. Ten orphaned instances were
//! enough to exhaust `fs.inotify.max_user_instances` on this machine,
//! self-amplifying the failure (a freshly spawned instance couldn't start
//! its watchers, never bound its socket, failed its own test, and leaked
//! ANOTHER child).
//!
//! This test does not merely assert the shared `HeadlessGuard` type has the
//! right shape -- it forces the actual failure path (a panic while the
//! guard is alive) via `std::panic::catch_unwind` and then checks, from
//! OUTSIDE that unwind, that the real spawned PID is genuinely gone. That
//! is the honestly-provable half of "does cleanup survive a panic":
//! everything from the spawn through the panicking assertion runs inside
//! ONE process (this test binary), so the assertion after `catch_unwind`
//! observes the real, physical process table, not a mocked one.
//!
//! **What this does NOT prove, and why:** the incident that motivated this
//! fix was a child that outlived the TEST BINARY itself (found alive days
//! later), not just the test function -- e.g. `cargo test` hard-killed,
//! OOM-killed, or aborted without unwinding, none of which run any `Drop`
//! impl and are exactly what `headless_test_support::arm_pdeathsig`'s
//! `PR_SET_PDEATHSIG` backstop exists for. Verifying THAT mechanism
//! honestly requires an external observer: a process tree at least three
//! levels deep (this test binary -> a wrapper that arms `pre_exec` and
//! spawns `mae --headless` -> the headless instance itself), a hard SIGKILL
//! of the middle process from a FOURTH, still-alive process, and a delayed
//! check that the grandchild died too -- a single `cargo test` process
//! cannot SIGKILL itself and then go on to make an assertion. That would
//! need a dedicated helper binary and a shell-level driver script, which is
//! more test infrastructure than this fix's scope justifies; the mechanism
//! itself (`prctl(2)`'s `PR_SET_PDEATHSIG)` is a well-established, single
//! syscall with well-documented semantics, not novel code this session
//! wrote from scratch. Said plainly rather than writing something that
//! only looks like it covers this case.

#![cfg(target_os = "linux")]

use std::panic::AssertUnwindSafe;
use std::time::Duration;

mod headless_test_support;
use headless_test_support::spawn_isolated_headless;

/// Probes whether `pid` still exists via `kill(pid, 0)` (signal 0 sends
/// nothing, just checks permission/existence) -- does not require this
/// process to still hold the child in a `Child` handle, so it keeps working
/// even after the owning guard has been moved into (and dropped inside) the
/// `catch_unwind`'d closure below.
fn pid_exists(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn a_panic_while_the_guard_is_alive_still_kills_the_real_process() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".git")).unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    let (_socket_path, guard) =
        spawn_isolated_headless(&project_root, &xdg_config, &xdg_data, tmp.path(), None);
    let pid = guard.pid() as i32;
    assert!(
        pid_exists(pid),
        "sanity: the freshly spawned headless instance (pid {pid}) must exist \
         before the forced-panic path even starts"
    );

    // Force the exact failure path this defect needed: a panic while the
    // guard is alive and owns the child, unwinding straight through the
    // guard's scope. `guard` is moved INTO the closure so its `Drop` runs
    // as part of this unwind -- not after some later, hand-written cleanup
    // call that a real test (like the ones this session fixed) could skip
    // or bypass.
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _guard = guard;
        panic!("deliberately forcing the panic/unwind path");
    }));
    assert!(
        result.is_err(),
        "the deliberately-forced assertion must have actually panicked -- if it \
         didn't, this test is vacuous and proves nothing about the unwind path"
    );

    // Poll rather than assert instantly: SIGTERM -> exit -> reap is not
    // synchronous with the `Drop` call returning control here (the Drop
    // impl itself blocks on `wait_for_exit`, so this is generous, not
    // load-bearing, headroom for a loaded CI machine).
    let mut still_alive = true;
    for _ in 0..50 {
        if !pid_exists(pid) {
            still_alive = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !still_alive,
        "pid {pid} is STILL ALIVE after a panic unwound through its HeadlessGuard's \
         scope -- this is the exact process-leak defect this test exists to catch; \
         Drop either didn't run or didn't successfully terminate the process"
    );
}
