#![cfg(unix)]
//! A daemon started while an operator command holds the collab store must
//! EXIT, not serve with collab and `kb/query` silently off. The real binary,
//! against a real lock. Kills: degrading to "collab disabled" again.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn config(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("daemon.toml");
    std::fs::write(
        &path,
        format!(
            "socket = \"{d}/kb.sock\"\ndata_dir = \"{d}/data\"\n\n[collab]\nenabled = true\n\n\
             [collab.auth]\nmode = \"none\"\n",
            d = dir.display()
        ),
    )
    .unwrap();
    path
}

#[test]
fn a_daemon_exits_when_the_store_is_held() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let held = mae_daemon::store_lock::StoreLock::acquire(&tmp.path().join("data/collab")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(["--config", &cfg.display().to_string()])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the daemon kept running with the store held");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut err = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut err).unwrap();
    assert_eq!(status.code(), Some(75), "EX_TEMPFAIL; stderr: {err}");
    assert!(err.contains("in use by another process"), "{err}");
    drop(held);
}
