//! `mae-daemon kb review export|verify` through the real binary (ADR-113
//! E10): a chain written by the review methods into a real `state.db` is
//! exported by one process and verified by another that is given no daemon
//! configuration at all, only the file and a key. A changed byte, the wrong
//! key, and a cut tail against a kept head each fail with exit 1; a chain made
//! by someone else's producer (the committed fixture) verifies.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mae_daemon::doc_store::DocStore;
use mae_daemon::kb_query::KbQueryLimits;
use mae_daemon::oauth_self_issue::{Credential, PrincipalKind, ValidatedPrincipal};
use mae_daemon::storage::SqliteBackend;
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};
use serde_json::{json, Value};

const KB: &str = "kb-chain";

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

/// Run `mae-daemon <args>` with `home` as every XDG root, killing it if it
/// has not exited in time (which means it fell through to serving).
fn daemon(home: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(args)
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("`mae-daemon {}` did not exit", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

fn person(p: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: p.to_string(),
        audience: vec!["https://mae.example/mcp".into()],
        expires_at: u64::MAX,
        issuer: "https://idp.example".into(),
        subject: "s".into(),
        credential: Credential::IdpBound,
        kind: PrincipalKind::Person,
        sign_in: None,
    }
}

/// Write a review chain into `collab/state.db`, signed by the identity the
/// daemon will load, through the review methods. Returns the daemon's key.
async fn write_chain(dir: &Path) -> String {
    let identity = Arc::new(Identity::load_or_generate(&dir.join("identity"), "daemon").unwrap());
    std::fs::create_dir_all(dir.join("collab")).unwrap();
    let backend = SqliteBackend::open(&dir.join("collab/state.db")).unwrap();
    let store = DocStore::new(Arc::new(backend), 500);
    store.set_signer(Arc::clone(&identity));
    let (owner, editor) = (
        Identity::generate("o").fingerprint(),
        Identity::generate("e").fingerprint(),
    );
    let mut coll = KbCollectionDoc::new_owned(KB, &owner, "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(&editor, "editor", Role::Editor);
    let _ = coll.add_node("n1", "n1");
    let by = mae_daemon::doc_editors::Attribution::Principal("seed");
    store
        .share_doc(&format!("kbc:{KB}"), &coll.encode_state(), by)
        .await
        .unwrap();
    let node = KbNodeDoc::new("n1", "Purchasing", "Buy carefully.", &[]);
    let version = node.version();
    store
        .share_doc(
            &mae_sync::kb_node_doc_name(KB, "n1"),
            &node.encode_state(),
            by,
        )
        .await
        .unwrap();
    let limits = KbQueryLimits {
        max_body_bytes: 65_536,
        max_scan_nodes: 50,
        max_search_results: 20,
    };
    let calls = [
        (
            &owner,
            "kb/review.set_policy",
            json!({"kb_id": KB, "review_enabled": true, "code_prefixes": ["PRO"]}),
        ),
        (
            &owner,
            "kb/review.set_code",
            json!({"kb_id": KB, "node_id": "n1", "doc_code": "PRO-1"}),
        ),
        (
            &editor,
            "kb/review.request",
            json!({"kb_id": KB, "nodes": [{"node_id": "n1", "version": version}], "reason": "first issue"}),
        ),
    ];
    for (who, m, p) in calls {
        mae_daemon::review::dispatch(m, &p, &store, &person(who), limits)
            .await
            .unwrap_or_else(|e| panic!("{m}: {e:?}"));
    }
    identity.public().encoded()
}

fn ok(o: &Output, what: &str) -> String {
    assert!(
        o.status.success(),
        "{what}: exit {:?}\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn refused(o: &Output, what: &str) {
    assert_eq!(
        o.status.code(),
        Some(1),
        "{what}: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("NOT VERIFIED"),
        "{what}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exported_chain_verifies_in_another_process_with_no_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let key = write_chain(tmp.path()).await;
    let export = tmp.path().join("chain.jsonl");
    let cfg_s = cfg.display().to_string();
    let out = ok(
        &daemon(
            tmp.path(),
            &[
                "kb",
                "review",
                "export",
                "--kb-id",
                KB,
                "--out",
                &export.display().to_string(),
                "--config",
                &cfg_s,
            ],
        ),
        "export",
    );
    let head = out.lines().last().unwrap().to_string();
    let head_file = tmp.path().join("kept-head.json");
    std::fs::write(&head_file, &head).unwrap();

    // The verifier runs where no daemon configuration exists at all.
    let bare = tempfile::tempdir().unwrap();
    let verify = |file: &Path, key: &str, against: Option<&Path>| {
        let mut args = vec![
            "kb".to_string(),
            "review".into(),
            "verify".into(),
            "--in".into(),
            file.display().to_string(),
            "--key".into(),
            key.to_string(),
            "--config".into(),
            bare.path().join("absent.toml").display().to_string(),
        ];
        if let Some(a) = against {
            args.extend(["--against".into(), a.display().to_string()]);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        daemon(bare.path(), &refs)
    };
    let text = ok(&verify(&export, &key, Some(&head_file)), "verify");
    assert!(
        text.contains("3 events") && text.ends_with("verified\n"),
        "{text}"
    );

    let original = std::fs::read(&export).unwrap();
    let mid = original.len() / 2;
    let mut changed = original.clone();
    changed[mid] = if changed[mid] == b'A' { b'B' } else { b'A' };
    let tampered = tmp.path().join("tampered.jsonl");
    std::fs::write(&tampered, &changed).unwrap();
    refused(&verify(&tampered, &key, None), "a changed byte");

    let other = Identity::generate("other").public().encoded();
    refused(&verify(&export, &other, None), "another key");

    // Cut the last event and re-use the old head line: the head no longer
    // names the last event.
    let lines: Vec<&str> = std::str::from_utf8(&original).unwrap().lines().collect();
    let cut = [&lines[..lines.len() - 2], &lines[lines.len() - 1..]]
        .concat()
        .join("\n");
    let cut_file = tmp.path().join("cut.jsonl");
    std::fs::write(&cut_file, cut).unwrap();
    refused(&verify(&cut_file, &key, Some(&head_file)), "a cut tail");
}

/// A chain from another producer, the committed fixture, verifies under its
/// test key through the binary: the format is E10's, not this daemon's.
#[test]
fn the_fixture_chain_verifies_through_the_binary() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../docs/verification/fixtures/review-api/events.json"
    ))
    .unwrap();
    let lines = fixture["export_jsonl"].as_array().unwrap();
    // The fixture's documented TEST key (seed bytes 0x01..0x20), never the
    // key its header claims.
    let seed: [u8; 32] = std::array::from_fn(|i| i as u8 + 1);
    let key = Identity::from_seed(&seed, "fixture").public().encoded();
    assert_eq!(lines[0]["daemon_key"], json!(key));
    let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("fixture.jsonl");
    std::fs::write(&file, text).unwrap();
    let out = daemon(
        tmp.path(),
        &[
            "kb",
            "review",
            "verify",
            "--in",
            &file.display().to_string(),
            "--key",
            &key,
        ],
    );
    let text = ok(&out, "fixture");
    assert!(text.contains("4 events"), "{text}");
}
