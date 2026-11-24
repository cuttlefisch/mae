#![cfg(unix)]
//! A site component directory that does not load refuses to start the daemon
//! (ADR-119 Amendment 4 H1, the live P-157), and `--check-config` reports the
//! same problem: the real binary, against real configuration files. Kills:
//! skipping a missing directory, or serving a site without its components.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// A daemon config with only `[oauth.site_components]` set: nothing else in
/// it can stop the daemon first.
fn config(dir: &Path, components: &Path) -> PathBuf {
    let path = dir.join("daemon.toml");
    std::fs::write(
        &path,
        format!(
            "socket = \"{d}/kb.sock\"\ndata_dir = \"{d}/data\"\n\n\
             [oauth.site_components]\n\"kb-docs\" = \"{c}\"\n",
            d = dir.display(),
            c = components.display()
        ),
    )
    .unwrap();
    path
}

fn daemon(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
    c.args(args)
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data-home"))
        .env_remove("MAE_DAEMON_CONFIG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

/// Run to exit, failing rather than hanging if the daemon starts serving.
fn run_to_exit(mut cmd: Command) -> Output {
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the daemon kept running with a component directory that does not load");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn a_missing_component_directory_refuses_to_start_the_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path(), &tmp.path().join("no-such-dir"));
    let cfg = cfg.display().to_string();
    let out = run_to_exit(daemon(tmp.path(), &["--config", &cfg]));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert!(err.contains("KB `kb-docs`"), "{err}");
    assert!(err.contains("does not exist"), "{err}");
    // Refused before anything was opened: no KB store was created.
    assert!(!tmp.path().join("data/daemon-kb.cozo").exists());

    let out = run_to_exit(daemon(tmp.path(), &["--check-config", "--config", &cfg]));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert!(err.contains("Site component configuration issues"), "{err}");
    assert!(err.contains("KB `kb-docs`"), "{err}");
}

/// The control: the same configuration naming the example components passes
/// `--check-config`, so the refusal above is the directory's, not the file's.
#[test]
fn the_example_components_pass_check_config() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/verification/fixtures/components/slots");
    let cfg = config(tmp.path(), &fixture);
    let out = run_to_exit(daemon(
        tmp.path(),
        &["--check-config", "--config", &cfg.display().to_string()],
    ));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("site components: 1 KB(s)"), "{stdout}");
    assert!(stdout.contains("Config OK"), "{stdout}");
}
