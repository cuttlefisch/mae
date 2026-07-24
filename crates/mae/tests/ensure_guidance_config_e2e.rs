//! Real subprocess e2e for K3 of the post-ship quality pass:
//! `mae --ensure-guidance-config [--guidance-kb <name>]`.
//!
//! Setup (as opposed to ongoing use) shouldn't depend on an LLM correctly
//! guessing which of N MCP tools to call — this is a deterministic, one-shot
//! CLI primitive the "MAE for VS Code" extension's first-activation hook can
//! call directly (mirrors `--print-socket-path`'s shape), reusing the proven
//! `set_option`/`save_option_to_init` (`:set-save`) persistence path.
//!
//! Spawns the real compiled `mae` binary (`env!("CARGO_BIN_EXE_mae")`)
//! against an isolated `XDG_CONFIG_HOME`/`HOME` and inspects the real
//! `init.scm` it writes — no mocks, matching this session's established
//! real-subprocess e2e pattern (`headless_e2e.rs`, `mcp_tool_tiering_e2e.rs`).

use std::path::PathBuf;
use std::process::Command;

fn isolated_env() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();
    (tmp, project_root, xdg_config)
}

fn run_ensure_guidance_config(
    project_root: &std::path::Path,
    xdg_config: &std::path::Path,
    xdg_data: &std::path::Path,
    home: &std::path::Path,
    extra_args: &[&str],
) -> std::process::Output {
    let mae = env!("CARGO_BIN_EXE_mae");
    let mut cmd = Command::new(mae);
    cmd.arg("--ensure-guidance-config")
        .args(extra_args)
        .current_dir(project_root)
        .env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_DATA_HOME", xdg_data)
        .env("HOME", home)
        .env("SHELL", "/bin/sh")
        .env("MAE_SKIP_WIZARD", "1");
    cmd.output()
        .expect("failed to run `mae --ensure-guidance-config`")
}

fn read_init_scm(xdg_config: &std::path::Path) -> String {
    std::fs::read_to_string(xdg_config.join("mae").join("init.scm")).unwrap_or_default()
}

#[test]
fn fresh_env_with_guidance_kb_arg_sets_both_options() {
    let (tmp, project_root, xdg_config) = isolated_env();
    let xdg_data = tmp.path().join("data");

    let output = run_ensure_guidance_config(
        &project_root,
        &xdg_config,
        &xdg_data,
        tmp.path(),
        &["--guidance-kb", "TestKB"],
    );
    assert!(
        output.status.success(),
        "stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let content = read_init_scm(&xdg_config);
    assert!(
        content.contains(r#"(set-option! "ai_guidance_kb" "TestKB")"#),
        "expected ai_guidance_kb to be persisted, got: {content}"
    );
    assert!(
        content.contains(r#"(set-option! "ai_guidance_export_live_sync" "true")"#),
        "expected ai_guidance_export_live_sync to be persisted true, got: {content}"
    );
}

#[test]
fn fresh_env_without_guidance_kb_arg_leaves_guidance_kb_unset_but_still_enables_live_sync() {
    let (tmp, project_root, xdg_config) = isolated_env();
    let xdg_data = tmp.path().join("data");

    let output = run_ensure_guidance_config(&project_root, &xdg_config, &xdg_data, tmp.path(), &[]);
    assert!(output.status.success());

    let content = read_init_scm(&xdg_config);
    assert!(
        !content.contains("ai_guidance_kb"),
        "with no --guidance-kb and nothing already set, ai_guidance_kb must stay untouched, \
         got: {content}"
    );
    assert!(
        content.contains(r#"(set-option! "ai_guidance_export_live_sync" "true")"#),
        "ai_guidance_export_live_sync must still be enabled even with no guidance KB chosen, \
         got: {content}"
    );
}

#[test]
fn never_overwrites_an_already_explicit_ai_guidance_kb() {
    let (tmp, project_root, xdg_config) = isolated_env();
    let xdg_data = tmp.path().join("data");

    // Seed init.scm as if the shipped template (or a user) already set an
    // explicit guidance KB, before this flag ever runs.
    let mae_config_dir = xdg_config.join("mae");
    std::fs::create_dir_all(&mae_config_dir).unwrap();
    std::fs::write(
        mae_config_dir.join("init.scm"),
        "(set-option! \"ai_guidance_kb\" \"MaePractices\")\n",
    )
    .unwrap();

    let output = run_ensure_guidance_config(
        &project_root,
        &xdg_config,
        &xdg_data,
        tmp.path(),
        &["--guidance-kb", "SomeOtherKB"],
    );
    assert!(output.status.success());

    let content = read_init_scm(&xdg_config);
    assert!(
        content.contains(r#"(set-option! "ai_guidance_kb" "MaePractices")"#),
        "an already-explicit ai_guidance_kb must never be overwritten by --guidance-kb, \
         got: {content}"
    );
    assert!(
        !content.contains("SomeOtherKB"),
        "the --guidance-kb argument must be ignored once ai_guidance_kb is already set, \
         got: {content}"
    );
}

#[test]
fn running_twice_is_idempotent_and_makes_no_further_changes() {
    let (tmp, project_root, xdg_config) = isolated_env();
    let xdg_data = tmp.path().join("data");

    let first = run_ensure_guidance_config(
        &project_root,
        &xdg_config,
        &xdg_data,
        tmp.path(),
        &["--guidance-kb", "TestKB"],
    );
    assert!(first.status.success());
    let content_after_first = read_init_scm(&xdg_config);

    let second = run_ensure_guidance_config(
        &project_root,
        &xdg_config,
        &xdg_data,
        tmp.path(),
        &["--guidance-kb", "DifferentKB"],
    );
    assert!(second.status.success());
    let content_after_second = read_init_scm(&xdg_config);

    assert_eq!(
        content_after_first, content_after_second,
        "a second run must be a pure no-op once both options are already set"
    );
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("no changes made") || stdout.contains("already"),
        "expected the second run to report nothing changed, got: {stdout}"
    );
}
