//! `mae-daemon backup …` through the real binary, as a restore rehearsal calls
//! it: separate processes for create, restore and verify, argv only, exit codes
//! and `key=number` stdout lines as the interface.
//!
//! Also guards the dispatch: an unrecognised subcommand falls through to
//! SERVING, so every form here must return on its own. A process still running
//! after the deadline is a failure, not a hang.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};

/// Every path the daemon could touch, inside `dir`. TOML literal strings, so a
/// Windows path's backslashes need no escaping.
fn config(dir: &Path) -> PathBuf {
    let d = dir.display();
    let path = dir.join("daemon.toml");
    std::fs::write(
        &path,
        format!(
            "socket = '{d}/kb.sock'\ndata_dir = '{d}/data'\n\n[collab]\nenabled = false\n\n\
             [collab.storage]\ndata_dir = '{d}/collab'\n\n[collab.auth]\n\
             identity_dir = '{d}/identity'\nauthorized_keys = '{d}/identity/authorized_keys'\n\
             keystore = '{d}/identity/trusted_keys'\n"
        ),
    )
    .unwrap();
    path
}

/// Run `mae-daemon <args> --config <cfg>`, killing it if it has not exited in
/// time (which means it fell through to serving).
fn daemon(dir: &Path, cfg: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(args)
        .args(["--config", &cfg.display().to_string()])
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!(
                "`mae-daemon {}` did not exit: it started serving",
                args.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn create_restore_verify_through_the_binary_report_the_inserted_count() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    std::fs::create_dir_all(tmp.path().join("data")).unwrap();
    let store =
        CozoKbStore::open_with_engine(tmp.path().join("data/daemon-kb.cozo"), "sqlite").unwrap();
    for i in 0..9 {
        let id = format!("e2e:{i}");
        store
            .insert_node(&Node::new(&id, &id, NodeKind::Note, "b"))
            .unwrap();
    }

    let archive = tmp.path().join("copy.tar");
    let out = daemon(
        tmp.path(),
        &cfg,
        &["backup", "create", &archive.display().to_string()],
    );
    assert!(
        out.status.success(),
        "create: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    drop(store);

    let dir = tmp.path().join("ensayo");
    let out = daemon(
        tmp.path(),
        &cfg,
        &[
            "backup",
            "restore",
            &archive.display().to_string(),
            &dir.display().to_string(),
        ],
    );
    assert!(
        out.status.success(),
        "restore: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = daemon(
        tmp.path(),
        &cfg,
        &["backup", "verify", &dir.display().to_string()],
    );
    assert!(
        out.status.success(),
        "verify: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        ["kb.daemon=9", "identity=0"],
        "no identity was configured"
    );
}

#[test]
fn every_malformed_backup_invocation_exits_without_serving() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    for args in [
        vec!["backup"],
        vec!["backup", "create"],
        vec!["backup", "restore", "only-one-arg"],
        vec!["backup", "bogus", "x"],
    ] {
        let out = daemon(tmp.path(), &cfg, &args);
        assert_eq!(out.status.code(), Some(2), "{args:?} is a usage error");
    }
    let missing = tmp.path().join("nothing-here");
    let out = daemon(
        tmp.path(),
        &cfg,
        &["backup", "verify", &missing.display().to_string()],
    );
    assert_eq!(out.status.code(), Some(1), "verifying nothing fails");
    assert!(stdout(&out).is_empty(), "and prints no key=number line");
}
