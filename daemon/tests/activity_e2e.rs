#![cfg(unix)]
//! ADR-120 D13, the operator contract, against the real binary: the health
//! check from the first request, its recovery with no restart, the two stable
//! log lines, `activity status`'s exit codes and JSON, and `activity export`
//! and `verify` on what the running daemon wrote.

mod daemon_harness;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

struct Daemon {
    proc: daemon_harness::Process,
    config: PathBuf,
    log: PathBuf,
    collab_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

/// A daemon with the record on, its `activity.db` made unopenable first (a
/// directory in its place), so it starts unhealthy.
async fn spawn_with_blocked_record() -> Daemon {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let collab_dir = root.join("collab");
    std::fs::create_dir_all(collab_dir.join("activity.db")).unwrap();
    let cert = root.join("oauth.crt");
    let key = root.join("oauth.key");
    let cert_der = daemon_harness::write_self_signed_cert(&cert, &key);
    let config = root.join("daemon.toml");
    let log = root.join("daemon.log");
    let proc = daemon_harness::spawn_ready(|ports| {
        std::fs::write(
            &config,
            format!(
                r#"socket = "{root}/mae-daemon.sock"
data_dir = "{root}"

[collab]
enabled = false

[oauth]
enabled = true
bind = "127.0.0.1:{oauth}"
canonical_resource_uri = "https://mae.test"
jwks_url = "http://127.0.0.1:9/never"
issuer = "https://idp.test"
cert_path = "{cert}"
key_path = "{key}"

[activity]
enabled = true
commit_timeout_ms = 500
"#,
                root = root.display(),
                oauth = ports.oauth,
                cert = cert.display(),
                key = key.display(),
            ),
        )
        .unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
        cmd.args([
            "--config",
            config.to_str().unwrap(),
            "--data-dir",
            root.to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", &root);
        daemon_harness::Launch {
            cmd,
            status_socket: root.join("mae-daemon.sock"),
            log: log.clone(),
            collab: false,
            oauth: Some(daemon_harness::OauthProbe {
                cert_der: cert_der.clone(),
                ws_ticket: false,
            }),
        }
    })
    .await;
    Daemon {
        proc,
        config,
        log,
        collab_dir,
        _tmp: tmp,
    }
}

async fn health(d: &Daemon) -> (u16, Value) {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let r = client
        .get(format!("https://{}/api/health", d.proc.ports.oauth_addr()))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (
        status,
        serde_json::from_str(&r.text().await.unwrap()).unwrap(),
    )
}

fn cli(config: Option<&Path>, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
    cmd.args(args);
    if let Some(c) = config {
        cmd.args(["--config", c.to_str().unwrap()]);
    }
    cmd.output().unwrap()
}

fn status_json(d: &Daemon) -> (i32, Value) {
    let out = cli(Some(&d.config), &["activity", "status", "--json"]);
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), v)
}

fn log_has(d: &Daemon, level: &str, message: &str) -> bool {
    std::fs::read_to_string(&d.log)
        .unwrap_or_default()
        .lines()
        .any(|l| l.contains(level) && l.contains(message))
}

#[tokio::test]
async fn the_operator_contract_from_a_blocked_start_to_recovery() {
    let mut d = spawn_with_blocked_record().await;

    // Unhealthy from the first request, with nothing recorded first.
    assert_eq!(
        health(&d).await,
        (503, serde_json::json!({"status": "unavailable"}))
    );
    let (code, s) = status_json(&d);
    assert_eq!(code, 1, "unhealthy: {s}");
    assert_eq!(s["healthy"], false);
    for k in [
        "unavailable_total",
        "queue_depth",
        "last_commit_age_ms",
        "last_head_age_s",
    ] {
        assert!(s.get(k).is_some(), "status lacks {k}: {s}");
    }
    for c in ["long", "short"] {
        assert!(s["chains"][c].get("seq").is_some() && s["chains"][c].get("entry_hash").is_some());
    }
    assert!(
        log_has(&d, "ERROR", "activity_record_unavailable"),
        "{}",
        d.proc.log_tail()
    );

    // Cleared, it recovers by itself: no restart, no request.
    std::fs::remove_dir(d.collab_dir.join("activity.db")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while health(&d).await.0 != 200 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(health(&d).await, (200, serde_json::json!({"status": "ok"})));
    assert!(
        log_has(&d, "INFO", "activity_record_recovered"),
        "{}",
        d.proc.log_tail()
    );
    let (code, s) = status_json(&d);
    assert_eq!(code, 0, "{s}");
    assert_eq!(s["healthy"], true);
    assert!(
        s["chains"]["long"]["seq"].as_u64().unwrap() >= 1,
        "daemon_start: {s}"
    );
    assert_eq!(
        s["chains"]["long"]["entry_hash"].as_str().unwrap().len(),
        64
    );

    // The export of what it wrote verifies, offline and in place; a
    // `--no-proof` export does not.
    let out_file = d.collab_dir.join("export.jsonl");
    let out = cli(
        Some(&d.config),
        &["activity", "export", "--out", out_file.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let verify = cli(
        None,
        &["activity", "verify", "--in", out_file.to_str().unwrap()],
    );
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let text = std::fs::read_to_string(&out_file).unwrap();
    assert!(
        text.contains("\"event\""),
        "the export's own run is recorded first"
    );
    let stored = cli(Some(&d.config), &["activity", "verify"]);
    assert!(
        stored.status.success(),
        "{}",
        String::from_utf8_lossy(&stored.stderr)
    );
    let np = d.collab_dir.join("no-proof.jsonl");
    let out = cli(
        Some(&d.config),
        &[
            "activity",
            "export",
            "--kind",
            "read",
            "--no-proof",
            "--out",
            np.to_str().unwrap(),
        ],
    );
    assert!(out.status.success());
    let verify = cli(None, &["activity", "verify", "--in", np.to_str().unwrap()]);
    assert_eq!(verify.status.code(), Some(1));

    // A daemon that is not running: exit 2.
    d.proc.stop().await;
    assert_eq!(status_json(&d).0, 2);
}
