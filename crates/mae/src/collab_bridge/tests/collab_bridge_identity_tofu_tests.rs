//! Split from the monolithic `collab_bridge_tests.rs`: TOFU/prompting verifier, host-key policy, drain_collab_intent, handle_*_event, status/doctor lines.

use super::*;

fn tofu_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mae-tofu-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
#[test]
fn prompting_verifier_pinned_match_no_prompt() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("pin");
    let kh = dir.join("known_hosts");
    let server = Identity::generate("daemon").public();
    KnownHosts::load(&kh).pin("d:9473", &server).unwrap();
    // No receiver needed — a pinned match must NOT prompt.
    let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh,
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_millis(50),
    };
    assert!(
        v.verify("d:9473", &server),
        "pinned key must be accepted silently"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn prompting_verifier_changed_key_rejected() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("changed");
    let kh = dir.join("known_hosts");
    KnownHosts::load(&kh)
        .pin("d:9473", &Identity::generate("real").public())
        .unwrap();
    let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh,
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_millis(50),
    };
    // A DIFFERENT key for the same addr → abort (no prompt).
    assert!(!v.verify("d:9473", &Identity::generate("imposter").public()));
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn prompting_verifier_unknown_accept_pins() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("accept");
    let kh = dir.join("known_hosts");
    let server = Identity::generate("daemon").public();
    let server_bytes = server.to_bytes();
    let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh.clone(),
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_secs(5),
    };
    // verify() blocks until the (simulated) user answers via the event reply.
    let handle = std::thread::spawn(move || v.verify("d:9473", &server));
    match rx.blocking_recv().expect("prompt event") {
        CollabEvent::HostKeyPrompt {
            reply, fingerprint, ..
        } => {
            assert!(fingerprint.starts_with("SHA256:"));
            reply.send(true).unwrap();
        }
        other => panic!("expected HostKeyPrompt, got {other:?}"),
    }
    assert!(handle.join().unwrap(), "accepted host must verify");
    // ...and is now pinned.
    assert_eq!(
        KnownHosts::load(&kh).get("d:9473").unwrap().to_bytes(),
        server_bytes
    );
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn prompting_verifier_unknown_reject_not_pinned() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("reject");
    let kh = dir.join("known_hosts");
    let server = Identity::generate("daemon").public();
    let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh.clone(),
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_secs(5),
    };
    let handle = std::thread::spawn(move || v.verify("d:9473", &server));
    if let CollabEvent::HostKeyPrompt { reply, .. } = rx.blocking_recv().unwrap() {
        reply.send(false).unwrap();
    }
    assert!(!handle.join().unwrap(), "rejected host must not verify");
    assert!(
        KnownHosts::load(&kh).get("d:9473").is_none(),
        "rejected host must not be pinned"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
/// B-21 regression: a runtime `collab_host_key_policy` change is honored by the
/// SAME verifier instance at verify-time (the verifier/transport is built once
/// at collab-task setup and cached, so it must read the live policy cell).
#[test]
fn host_key_policy_change_honored_at_verify_time_b21() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("b21");
    let kh = dir.join("known_hosts");
    let policy = std::sync::Arc::new(std::sync::Mutex::new("accept-new".to_string()));
    let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh.clone(),
        evt_tx: tx,
        policy: policy.clone(),
        timeout: std::time::Duration::from_secs(5),
    };
    // accept-new: an unknown host is pinned WITHOUT prompting.
    let a = Identity::generate("daemon-a").public();
    assert!(v.verify("a:9473", &a), "accept-new pins unknown host");
    assert!(rx.try_recv().is_err(), "accept-new must NOT prompt");
    assert_eq!(
        KnownHosts::load(&kh).get("a:9473").unwrap().to_bytes(),
        a.to_bytes()
    );

    // Flip the LIVE policy to `prompt` — the SAME verifier must now ASK on a new
    // host instead of auto-pinning (the B-21 fix: no rebuild/relaunch needed).
    *policy.lock().unwrap() = "prompt".to_string();
    let b = Identity::generate("daemon-b").public();
    let b_bytes = b.to_bytes();
    let handle = std::thread::spawn(move || v.verify("b:9473", &b));
    match rx
        .blocking_recv()
        .expect("prompt event after runtime policy change")
    {
        CollabEvent::HostKeyPrompt {
            reply, fingerprint, ..
        } => {
            assert!(fingerprint.starts_with("SHA256:"));
            reply.send(false).unwrap(); // decline
        }
        other => panic!("expected HostKeyPrompt after policy→prompt, got {other:?}"),
    }
    assert!(!handle.join().unwrap(), "declined prompt → not verified");
    assert!(
        KnownHosts::load(&kh).get("b:9473").is_none(),
        "declined host must not be pinned"
    );
    let _ = b_bytes; // (only needed to move `b` into the thread)
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn drain_collab_intent_connect() {
    let mut editor = Editor::new();
    editor.collab.pending_intent = Some(CollabIntent::Connect {
        address: "127.0.0.1:9473".to_string(),
    });
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    assert!(editor.collab.pending_intent.is_none());
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, CollabCommand::Connect { .. }));
}
#[test]
fn drain_collab_intent_empty_is_noop() {
    let mut editor = Editor::new();
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    assert!(rx.try_recv().is_err());
}
#[test]
fn drain_collab_share_enables_sync() {
    let mut editor = Editor::new();
    let buf_name = editor.buffers[0].name.clone();
    editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
        buffer_name: buf_name.clone(),
    });
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    let cmd = rx.try_recv().unwrap();
    match cmd {
        CollabCommand::ShareBuffer {
            doc_id,
            state_bytes,
        } => {
            // Buffer with no file_path gets DocAddress::Shared, serialized as "shared:{name}".
            assert_eq!(doc_id, format!("shared:{}", buf_name));
            assert!(
                !state_bytes.is_empty(),
                "state bytes should be non-empty after enable_sync"
            );
        }
        other => panic!("expected ShareBuffer, got {:?}", other),
    }
    // Sync should now be enabled on the buffer.
    assert!(editor.buffers[0].sync_doc.is_some());
}
#[test]
fn drain_collab_list_docs() {
    let mut editor = Editor::new();
    editor.collab.pending_intent = Some(CollabIntent::ListDocs);
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    let cmd = rx.try_recv().unwrap();
    assert!(matches!(cmd, CollabCommand::ListDocs { for_join: false }));
}
#[test]
fn drain_collab_join_doc() {
    let mut editor = Editor::new();
    editor.collab.pending_intent = Some(CollabIntent::JoinDoc {
        doc_id: "test.org".to_string(),
    });
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    let cmd = rx.try_recv().unwrap();
    match cmd {
        CollabCommand::JoinDoc { doc_id } => assert_eq!(doc_id, "test.org"),
        other => panic!("expected JoinDoc, got {:?}", other),
    }
}
#[test]
fn handle_connected_event() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::Connected {
            address: "127.0.0.1:9473".to_string(),
            peer_count: 2,
        },
    );
    assert_eq!(
        editor.collab.status,
        CollabStatus::Connected { peer_count: 2 }
    );
}
#[test]
fn handle_disconnected_event() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    editor.collab.synced_buffers.insert("test.rs".to_string());
    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "test".to_string(),
        },
    );
    assert_eq!(editor.collab.status, CollabStatus::Disconnected);
    assert_eq!(editor.collab.synced_docs, 0);
    // UI tracking cleared, but per-buffer state depends on sync_doc presence.
    assert!(editor.collab.synced_buffers.is_empty());
}
#[test]
fn handle_buffer_shared_event() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferShared {
            doc_id: "main.rs".to_string(),
        },
    );
    assert!(editor.collab.synced_buffers.contains("main.rs"));
    assert_eq!(editor.collab.synced_docs, 1);
    assert!(editor.status_msg.contains("Shared: main.rs"));
}
#[test]
fn handle_doc_list_event_creates_buffer() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::DocList {
            documents: vec!["a.rs".to_string(), "b.rs".to_string()],
            for_join: false,
        },
    );
    let idx = editor.find_buffer_by_name("*Collab Docs*");
    assert!(idx.is_some());
    let buf = &editor.buffers[idx.unwrap()];
    assert!(buf.text().contains("a.rs"));
    assert!(buf.text().contains("b.rs"));
}
#[test]
fn handle_doc_list_for_join_opens_palette() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::DocList {
            documents: vec!["file1.org".to_string()],
            for_join: true,
        },
    );
    assert!(editor.command_palette.is_some());
    let palette = editor.command_palette.as_ref().unwrap();
    assert_eq!(palette.purpose, mae_core::PalettePurpose::CollabJoin);
    assert!(palette.entries.iter().any(|e| e.name == "file1.org"));
}
#[test]
fn handle_status_report_creates_buffer() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::StatusReport {
            lines: vec!["line1".to_string(), "line2".to_string()],
        },
    );
    let idx = editor.find_buffer_by_name("*Collab Status*");
    assert!(idx.is_some());
}
#[test]
fn handle_doctor_report_creates_buffer() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::DoctorReport {
            lines: vec!["ok".to_string()],
        },
    );
    let idx = editor.find_buffer_by_name("*Collab Doctor*");
    assert!(idx.is_some());
}
#[test]
fn status_lines_connected() {
    let lines = build_status_lines("127.0.0.1:9473", true, &["main.rs".to_string()]);
    assert!(lines.iter().any(|l| l.contains("Connected")));
    assert!(lines.iter().any(|l| l.contains("main.rs")));
}
#[test]
fn doctor_lines_disconnected() {
    let ctx = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: false,
        server_debug: None,
        ping_latency_ms: None,
        synced_info: vec![],
    };
    let lines = build_doctor_lines(&ctx);
    assert!(lines.iter().any(|l| l.contains("\u{2717}")));
    assert!(lines.iter().any(|l| l.contains("Troubleshooting")));
}
#[test]
fn doctor_lines_include_join_and_list() {
    let ctx = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: false,
        server_debug: None,
        ping_latency_ms: None,
        synced_info: vec![],
    };
    let lines = build_doctor_lines(&ctx);
    assert!(lines.iter().any(|l| l.contains("SPC C l")));
    assert!(lines.iter().any(|l| l.contains("SPC C j")));
}
#[test]
fn doctor_lines_show_server_stats() {
    // Matches actual $/debug response shape: doc_stats is a map keyed by name.
    let ctx = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: true,
        server_debug: Some(serde_json::json!({
            "documents": 1,
            "doc_stats": {
                "test.rs": {
                    "wal_seq": 42,
                    "update_count": 10,
                    "connected_clients": 2,
                    "idle_secs": 5
                }
            }
        })),
        ping_latency_ms: Some(3),
        synced_info: vec![],
    };
    let lines = build_doctor_lines(&ctx);
    assert!(lines.iter().any(|l| l.contains("test.rs")));
    assert!(lines.iter().any(|l| l.contains("wal:42")));
    assert!(lines.iter().any(|l| l.contains("clients:2")));
}
#[test]
fn doctor_lines_show_latency() {
    let ctx = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: true,
        server_debug: None,
        ping_latency_ms: Some(7),
        synced_info: vec![],
    };
    let lines = build_doctor_lines(&ctx);
    assert!(lines.iter().any(|l| l.contains("Ping: 7ms")));
}
#[test]
fn doctor_lines_show_synced_buffers() {
    let ctx = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: true,
        server_debug: None,
        ping_latency_ms: None,
        synced_info: vec![("doc-a".to_string(), 0), ("doc-b".to_string(), 3)],
    };
    let lines = build_doctor_lines(&ctx);
    assert!(lines
        .iter()
        .any(|l| l.contains("doc-a") && l.contains("up-to-date")));
    assert!(lines
        .iter()
        .any(|l| l.contains("doc-b") && l.contains("3 pending")));
}
#[test]
fn doctor_lines_disconnected_no_crash() {
    let ctx = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: false,
        server_debug: None,
        ping_latency_ms: None,
        synced_info: vec![],
    };
    let lines = build_doctor_lines(&ctx);
    assert!(!lines.is_empty());
    assert!(lines.iter().any(|l| l.contains("not reachable")));
}
