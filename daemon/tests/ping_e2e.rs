#![cfg(unix)]
//! `mae-daemon ping` is the liveness probe: it succeeds against a serving
//! instance and fails, with a reason, against one that is not there. Both run
//! the real binary against a real instance.

use std::path::Path;
use std::process::Command;

fn config(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("daemon.toml");
    std::fs::write(
        &path,
        format!(
            "socket = \"{d}/kb.sock\"\ndata_dir = \"{d}/data\"\n\n[collab]\nenabled = false\n",
            d = dir.display()
        ),
    )
    .unwrap();
    path
}

fn ping(dir: &Path, cfg: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(["ping", "--config", &cfg.display().to_string()])
        .env("XDG_RUNTIME_DIR", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .output()
        .unwrap()
}

#[tokio::test]
async fn ping_succeeds_against_a_serving_instance_and_fails_without_one() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());

    // Nothing running yet: must fail, and say where it looked.
    let out = ping(tmp.path(), &cfg);
    assert!(
        !out.status.success(),
        "ping must fail with no daemon running"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kb.sock"), "names the socket it tried: {err}");

    let mut child = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(["--config", &cfg.display().to_string()])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let sock = tmp.path().join("kb.sock");
    let up = mae_mcp::ready::wait_until(|| async {
        tokio::net::UnixStream::connect(&sock).await.is_ok()
    })
    .await;

    let out = ping(tmp.path(), &cfg);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        up,
        "{}",
        mae_mcp::ready::timeout_message("mae-daemon KB socket")
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "ping against a serving instance: {stdout}"
    );
    assert!(stdout.starts_with("ok: mae-daemon "), "{stdout}");
}
