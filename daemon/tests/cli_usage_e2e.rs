//! `mae-daemon`'s usage surface through the real binary (#955).
//!
//! Serving is the default, so before #955 `--help`, a mistyped subcommand or a
//! stray flag each started a full daemon: it opened the KB store and every
//! federated instance and ran any open-time migration. The attacker here is an
//! operator's typo, and what must fail is "it served".
//!
//! Every refused form must exit on its own, with nothing created in a fresh
//! instance directory. A process still running at the deadline started serving,
//! and is reported as that, not as a hang. The positive control proves the fix
//! did not refuse the one subcommand an existing caller depends on: the
//! editor's `collab-start` spawns `mae-daemon start`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// A config that keeps every path the daemon could touch inside `dir`, with
/// the collab listener off. TOML literal strings, so Windows backslashes need
/// no escaping.
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

fn spawn(dir: &Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(args)
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Run `mae-daemon <args>`, killing it if it has not exited in time. The
/// deadline is generous: a refused form exits in milliseconds, so only the
/// defect (it served) comes near it.
fn run(dir: &Path, args: &[&str]) -> Output {
    let mut child = spawn(dir, args);
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`mae-daemon {}` did not exit: it started serving",
                args.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

/// Every path under `dir`, relative, so "nothing was created" is checked
/// against the whole tree and not against the paths we thought to name.
fn tree(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            out.insert(p.strip_prefix(dir).unwrap().display().to_string());
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    out
}

/// Run `args` (with this instance's `--config` appended) in a fresh directory
/// and return the output, asserting that it touched nothing.
fn run_untouched(args: &[&str]) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let before = tree(tmp.path());
    let cfg_s = cfg.display().to_string();
    let mut full: Vec<&str> = args.to_vec();
    full.extend(["--config", cfg_s.as_str()]);
    let out = run(tmp.path(), &full);
    assert_eq!(
        tree(tmp.path()),
        before,
        "`mae-daemon {}` created files: it opened a store or a socket",
        args.join(" ")
    );
    out
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn help_prints_usage_and_opens_nothing() {
    for args in [
        &["--help"][..],
        &["-h"],
        &["doctor", "--help"],
        &["kb", "-h"],
    ] {
        let out = run_untouched(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "`{}`: {}",
            args.join(" "),
            text(&out.stderr)
        );
        let stdout = text(&out.stdout);
        assert!(
            stdout.starts_with("usage: mae-daemon"),
            "`{}` printed: {stdout}",
            args.join(" ")
        );
        // The usage names every subcommand an operator can type.
        for sub in [
            "doctor",
            "ping",
            "authorize",
            "token",
            "binding",
            "backup",
            "kb ",
        ] {
            assert!(stdout.contains(sub), "usage omits `{sub}`");
        }
    }
}

#[test]
fn a_mistyped_subcommand_is_a_usage_error_not_a_daemon() {
    for typo in ["dcotor", "kb-anchor", "serve-now", "Doctor"] {
        let out = run_untouched(&[typo]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`{typo}`: {}",
            text(&out.stderr)
        );
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains(&format!("unknown subcommand `{typo}`")),
            "`{typo}` stderr: {stderr}"
        );
        assert!(
            stderr.contains("usage: mae-daemon"),
            "no usage after the error: {stderr}"
        );
    }
}

#[test]
fn a_stray_argument_to_serve_is_a_usage_error() {
    let cases: &[(&[&str], &str)] = &[
        (&["--bogus"], "unknown flag `--bogus`"),
        (&["--bnid", "0.0.0.0:9473"], "unknown flag `--bnid`"),
        (&["start", "now"], "unexpected argument `now`"),
    ];
    for (args, want) in cases {
        let out = run_untouched(args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`{}`: {}",
            args.join(" "),
            text(&out.stderr)
        );
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains(want),
            "`{}` stderr: {stderr}",
            args.join(" ")
        );
    }
}

/// Positive control: `start` (the editor's `collab-start`) still serves. It is
/// serving once the KB socket answers to a connect; then it is killed.
#[cfg(unix)]
#[test]
fn start_still_serves() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let sock = tmp.path().join("kb.sock");
    let mut child = spawn(
        tmp.path(),
        &["start", "--config", &cfg.display().to_string()],
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let served = loop {
        if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            break true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let out = child.wait_with_output().unwrap();
            panic!(
                "`mae-daemon start` exited ({status}) instead of serving: {}",
                text(&out.stderr)
            );
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(served, "`mae-daemon start` never opened its KB socket");
}
