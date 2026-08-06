//! Whether the current thread may perform **real external effects** — git
//! mutations, writes to the user's config directory, and anything else that
//! outlives the process.
//!
//! # Why this exists
//!
//! `Editor` resolves its git root from `std::env::current_dir()` when no
//! project is open, and its config path from `$XDG_CONFIG_HOME`/`$HOME`. Both
//! are ambient. A test that constructs `Editor::new()` therefore inherits the
//! *developer's own* repository and home directory, and any command it
//! dispatches acts on them for real.
//!
//! That was not hypothetical. `commands::tests::all_builtin_commands_dispatch`
//! dispatches every registered builtin to prove each has a dispatch arm; on a
//! contributor's machine a single `cargo test -p mae-core` ran `git stash
//! push`, `git reset HEAD -- .`, `git add .`, `git fetch --all`, `git pull` and
//! `git push` against their working tree, and wrote `init.scm` into their real
//! config directory. Uncommitted work disappeared mid-session with no
//! indication of the cause — the stash entry looked like something external had
//! run.
//!
//! It survived because **the damage only lands on a developer's machine.** CI
//! checkouts are disposable, `git push` fails on absent credentials, and the
//! test asserts only that a dispatch arm exists — never that any command
//! succeeded. So CI was permanently, misleadingly green.
//!
//! # The rule
//!
//! In a test build, external effects are **blocked by default**. A test that
//! genuinely needs one opts in explicitly with [`with_external_effects`], which
//! is also the marker saying "this test has arranged an isolated target".
//! Production builds are unaffected: [`external_effects_blocked`] is a constant
//! `false` outside `cfg(test)` unless the env var below is set.
//!
//! # Why this is its own crate
//!
//! The guard is needed in two places that are **siblings, not stacked**:
//! `mae-core` (git root, config writes, `Editor::mae_data_dir`) and `mae-mcp`
//! (`identity::default_collab_dir`, the parent of the collab identity key and
//! the per-KB content keys). `mae-mcp` is only a *dev*-dependency of
//! `mae-core`, so neither can host the module for the other, and putting a
//! second copy of the test-binary detection in each is exactly the drift
//! principle #15 forbids — the copy that drifts is the one that stops
//! guarding. Hence a dependency-free leaf crate that both depend on, which the
//! daemon workspace also picks up through `mae-mcp`.
//!
//! `mae_core::effect_sandbox` re-exports this, so callers above keep their
//! natural spelling.
//!
//! @stability: stable
//!
//! @ai-caution: [test-safety] Do not "fix" a failing test by wrapping it in
//! [`with_external_effects`] to make the refusal go away. The opt-in asserts
//! that the effect has somewhere safe to land — an isolated tmp dir, an
//! isolated `XDG_CONFIG_HOME`. Opting in without arranging that isolation
//! re-creates exactly the defect above, and the next contributor loses work.

use std::cell::Cell;

thread_local! {
    /// Per-thread rather than global: `cargo test` runs each test on its own
    /// thread, so one test's deliberate opt-in must not silently license a
    /// concurrently-running test's accident.
    static EFFECTS_ALLOWED: Cell<bool> = const { Cell::new(false) };
}

/// Opt this crate's guard in for other crates' test binaries, where
/// `cfg!(test)` is false because `mae-core` itself was compiled as a
/// dependency.
///
/// Read once and cached: `std::env::var` is not safe to race against a
/// concurrent `set_var`, and the value is not meant to change mid-process.
fn env_blocks_effects() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| std::env::var_os("MAE_BLOCK_EXTERNAL_EFFECTS").is_some())
}

/// Is this process a cargo-built **test** binary?
///
/// The call-site `cfg!(test)` the macro captures is not enough on its own: a
/// guard that lives in `mae-core` but is *called* from `mae-core` code sees
/// `cfg!(test) == false` whenever the test driving it belongs to another crate.
/// That is not a corner case — `cargo test -p mae-ai --lib` reaching
/// `Editor::mae_data_dir` is exactly how the contributor's real
/// `kb-registry.toml` got overwritten.
///
/// Cargo places test, integration-test and bench executables in
/// `target/<profile>/deps/`, while `cargo run` and installed binaries never
/// live there (`target/debug/mae`, `~/.local/bin/mae`). Testing for a parent
/// directory named `deps` therefore separates the two without an env var and
/// without cooperation from the caller.
///
/// Deliberately conservative in the safe direction: if `current_exe()` fails we
/// answer `false`, because wrongly reporting "this is a test" would disable
/// real git and real config writes for a user running the actual editor.
fn running_as_cargo_test_binary() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                let parent = exe.parent()?.file_name()?.to_owned();
                Some(parent == *"deps")
            })
            .unwrap_or(false)
    })
}

/// Are real external effects blocked on this thread right now?
///
/// `caller_is_test_build` must be the **calling crate's** `cfg!(test)`. It is
/// a parameter rather than an internal `cfg!(test)` because a `cfg!(test)`
/// written here is false whenever `mae-core` is compiled as a dependency — so
/// `mae-ai`'s and `mae`'s test binaries would silently get no protection at
/// all, which is precisely the surface that reaches git through
/// `tool_impls::git::run_git`. Prefer the
/// [`external_effects_blocked!`](crate::external_effects_blocked) macro, which
/// fills this in correctly by construction.
pub fn external_effects_blocked_for(caller_is_test_build: bool) -> bool {
    if !(caller_is_test_build || running_as_cargo_test_binary() || env_blocks_effects()) {
        return false;
    }
    !EFFECTS_ALLOWED.with(|c| c.get())
}

/// Are real external effects blocked on this thread right now?
///
/// Expands at the call site so the calling crate's `cfg!(test)` is what
/// counts. True in any crate's test build, or when
/// `MAE_BLOCK_EXTERNAL_EFFECTS` is set — unless the current thread is inside
/// [`with_external_effects`].
#[macro_export]
macro_rules! external_effects_blocked {
    () => {
        $crate::external_effects_blocked_for(cfg!(test))
    };
}

/// Run `f` with external effects permitted on this thread.
///
/// For tests that deliberately exercise a real effect **against an isolated
/// target they have arranged themselves** — a `tempfile::TempDir` repo, an
/// `XDG_CONFIG_HOME` pointed at scratch space. Restores the previous value
/// even if `f` panics, so a failing test cannot leave the door open for
/// whatever runs next on the same thread.
pub fn with_external_effects<T>(f: impl FnOnce() -> T) -> T {
    let previous = EFFECTS_ALLOWED.with(|c| c.replace(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    EFFECTS_ALLOWED.with(|c| c.set(previous));
    match result {
        Ok(v) => v,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// The stand-in path handed to git when effects are blocked.
///
/// Deliberately a path that cannot exist, so the spawn fails immediately with
/// a legible error rather than running somewhere unexpected. Naming it in the
/// error is the point: a developer who sees this in a status message should be
/// able to tell at a glance that the sandbox refused, not that git is broken.
pub fn blocked_git_root() -> std::path::PathBuf {
    std::env::temp_dir().join("mae-effect-sandbox-no-such-git-root")
}

/// The one lock serialising tests that mutate process-global environment.
///
/// `std::env::set_var` is process-wide, so a test that redirects `HOME`,
/// `XDG_CONFIG_HOME` or `PATH` changes it for every other test running
/// concurrently in the same binary. Serialising is the usual mitigation, but
/// it only works if everyone takes the **same** lock — and this workspace had
/// grown twelve independent `static ENV_LOCK`s plus several tests taking none
/// at all, so a test holding one lock ran happily alongside a test holding
/// another while both rewrote the environment out from under each other and
/// under the ~3,000 tests taking no lock at all.
///
/// A single static here gives exactly one lock per process, shared by every
/// crate linked into that test binary.
///
/// Poison-tolerant: the guarded data is `()`, and propagating poisoning turns
/// one genuine failure into a cascade of `PoisonError`s that hides which test
/// actually broke.
///
/// @ai-caution: [test-safety] A lock only orders the tests that *take* it. The
/// rest of the binary still runs against the mutated global, so this makes env
/// mutation less bad, never safe. Prefer an injected path (`data_dir_override`,
/// an explicit argument) over mutating the environment at all; reach for this
/// only when the ambient resolution itself is the thing under test.
pub fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default must be *blocked*. If this ever inverts, every other guard
    /// in the crate silently stops guarding, so it is asserted directly rather
    /// than assumed by the tests that depend on it.
    #[test]
    fn effects_are_blocked_by_default_in_a_test_build() {
        assert!(crate::external_effects_blocked!());
    }

    /// The cross-crate half, asserted independently of `cfg!(test)`.
    ///
    /// Passing `false` simulates the case that actually bit: a guard sited in
    /// `mae-core` and called from `mae-core` code, while the test driving it
    /// lives in `mae-ai`. If only the call-site `cfg!(test)` were consulted
    /// this would be `false` — and `mae_data_dir` would hand back the
    /// contributor's real `~/.local/share/mae`.
    #[test]
    fn a_test_binary_is_detected_even_when_the_call_site_is_not_a_test_build() {
        assert!(
            running_as_cargo_test_binary(),
            "this IS a cargo test binary; detection is broken, so every guard \
             called from non-test crate code is disarmed"
        );
        assert!(
            external_effects_blocked_for(false),
            "a guard whose call site is not itself a test build was left unguarded"
        );
    }

    /// The opt-in must still win over runtime detection, or a test that has
    /// arranged isolation could never exercise a real effect.
    #[test]
    fn the_opt_in_overrides_runtime_test_binary_detection() {
        with_external_effects(|| {
            assert!(!external_effects_blocked_for(false));
        });
    }

    #[test]
    fn opting_in_permits_effects_only_within_the_scope() {
        assert!(crate::external_effects_blocked!());
        with_external_effects(|| {
            assert!(
                !crate::external_effects_blocked!(),
                "opt-in did not take effect"
            );
        });
        assert!(
            crate::external_effects_blocked!(),
            "the opt-in leaked past its scope"
        );
    }

    /// A panic inside the scope must not leave effects permitted for whatever
    /// runs next on this thread — the leak direction that matters, since it
    /// fails *open*.
    #[test]
    fn a_panic_inside_the_scope_still_restores_the_block() {
        let caught = std::panic::catch_unwind(|| {
            with_external_effects(|| panic!("boom"));
        });
        assert!(caught.is_err(), "the panic should have propagated");
        assert!(
            crate::external_effects_blocked!(),
            "a panicking opt-in left external effects permitted"
        );
    }

    /// Nesting must restore the *outer* state, not unconditionally re-block —
    /// otherwise an inner helper silently disarms its caller's opt-in.
    #[test]
    fn nested_scopes_restore_the_outer_value() {
        with_external_effects(|| {
            with_external_effects(|| {
                assert!(!crate::external_effects_blocked!());
            });
            assert!(
                !crate::external_effects_blocked!(),
                "the inner scope re-blocked its caller"
            );
        });
        assert!(crate::external_effects_blocked!());
    }
}
