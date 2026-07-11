//! Regression test for the ROADMAP.md "Known Bugs → Pre-existing" entry
//! "Theme load failure is silent in headless mode": `--check-config` used to
//! exit 0 in headless/CI mode even when `config.toml` named a theme that
//! doesn't exist, because the fatal-error check only matched status strings
//! starting with `"Error in"` while `Editor::set_theme_by_name`'s failure
//! sets a differently-worded status message. `--check-config` now resolves
//! the theme directly via `Theme::load` (see
//! `crates/mae/src/cli.rs::handle_check_config`) instead of sniffing
//! `editor.status_msg`, so a bad theme name is a structurally-detected fatal
//! error — independent of how the failure happens to be worded.
//!
//! This drives the real compiled `mae` binary end-to-end (not the internal
//! function directly) so it also catches any future regression in how
//! `--check-config` wires the check into the process exit code.

use std::path::Path;
use std::process::{Command, Output};

/// Run `mae --check-config` with an isolated `XDG_CONFIG_HOME`/`HOME` so the
/// test never touches the real user's config.
fn run_check_config(xdg_config_home: &Path, home: &Path) -> Output {
    let exe = env!("CARGO_BIN_EXE_mae");
    Command::new(exe)
        .arg("--check-config")
        .env("XDG_CONFIG_HOME", xdg_config_home)
        .env("HOME", home)
        .env("MAE_SKIP_WIZARD", "1")
        .output()
        .expect("failed to spawn `mae --check-config`")
}

#[test]
fn check_config_exit_code_reflects_theme_validity() {
    let tmp = tempfile::tempdir().unwrap();

    // --- Negative case (the bug): a config.toml naming a theme that does not
    // exist must make --check-config fail loudly, not exit 0. Uses a random
    // (not cherry-picked) suffix so this can never accidentally collide with
    // a real bundled or future user theme name.
    let bad_config_home = tmp.path().join("bad");
    let bad_mae_dir = bad_config_home.join("mae");
    std::fs::create_dir_all(&bad_mae_dir).unwrap();
    let bogus_theme = format!(
        "definitely-not-a-real-theme-{}",
        std::process::id() // varies per test run, not a fixed "unicorn" value
    );
    std::fs::write(
        bad_mae_dir.join("config.toml"),
        format!("[editor]\ntheme = \"{bogus_theme}\"\n"),
    )
    .unwrap();

    let bad_output = run_check_config(&bad_config_home, tmp.path());
    assert!(
        !bad_output.status.success(),
        "expected `--check-config` to exit non-zero for an unresolvable theme name \
         ({bogus_theme:?}); stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&bad_output.stdout),
        String::from_utf8_lossy(&bad_output.stderr)
    );
    let bad_stderr = String::from_utf8_lossy(&bad_output.stderr);
    assert!(
        bad_stderr.contains(&bogus_theme) && bad_stderr.to_lowercase().contains("theme"),
        "stderr should name the offending theme so the failure is actionable: {bad_stderr}"
    );

    // --- Positive/control case: the SAME harness with no theme override must
    // still exit 0. This pins that the failure above is specific to the bad
    // theme name, not an artifact of the sandboxed XDG_CONFIG_HOME/HOME setup
    // (a selective oracle, not just "it failed somehow").
    let good_config_home = tmp.path().join("good");
    std::fs::create_dir_all(good_config_home.join("mae")).unwrap();
    let good_output = run_check_config(&good_config_home, tmp.path());
    assert!(
        good_output.status.success(),
        "expected `--check-config` to exit 0 with no theme override configured; \
         stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&good_output.stdout),
        String::from_utf8_lossy(&good_output.stderr)
    );
}
