//! ADR-111 P1 — a registered remote hub is reachable from the editor.
//!
//! Before this, a `RemoteHub` registry row was inert in every shipped binary:
//! `mae-core`'s hub code was compiled out, `kb_search` never visited a hub, a
//! daemon-hosted primary shadowed the only layer that held one, and nothing
//! could register one. Turning it on also exposed a crash — see
//! `mae_kb::query_off_thread` — which `hub_layers_survive_rebuilds_inside_a_runtime`
//! pins at the editor level.
//!
//! The hub is a real HTTP server speaking the client's JSON-RPC, so these
//! tests exercise the production client, not a stand-in layer.

use super::*;
use mae_kb::federation::RemoteHubAuth;
use mae_kb::KbScope;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

/// A hub answering `kb/query.search` (substring match on title/body) and
/// `kb/query.get` from a fixed node set, for as long as the test runs.
fn spawn_hub(nodes: &[(&str, &str, &str)]) -> String {
    let nodes: Vec<(String, String, String)> = nodes
        .iter()
        .map(|(i, t, b)| (i.to_string(), t.to_string(), b.to_string()))
        .collect();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/", listener.local_addr().expect("addr"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = answer(stream, &nodes);
        }
    });
    url
}

fn answer(
    mut stream: std::net::TcpStream,
    nodes: &[(String, String, String)],
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let req: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let params = &req["params"];
    let result = match req["method"].as_str().unwrap_or("") {
        "kb/query.search" => {
            let q = params["query"].as_str().unwrap_or("").to_lowercase();
            let hits: Vec<_> = nodes
                .iter()
                .filter(|(_, t, b)| t.to_lowercase().contains(&q) || b.to_lowercase().contains(&q))
                .map(|(i, _, _)| serde_json::json!({"id": i, "score": 1.0}))
                .collect();
            serde_json::json!({"results": hits})
        }
        "kb/query.get" => {
            let id = params["node_id"].as_str().unwrap_or("");
            match nodes.iter().find(|(i, _, _)| i == id) {
                Some((_, t, b)) => serde_json::json!({"title": t, "body": b, "tags": []}),
                None => serde_json::Value::Null,
            }
        }
        _ => serde_json::Value::Null,
    };
    let payload =
        serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": result}).to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    )
}

/// A URL nothing listens on.
fn dead_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("bind")
        .port();
    format!("http://127.0.0.1:{port}/")
}

/// Register a hub row directly, as a hand-edited registry would (the command
/// refuses plain HTTP; the mock is plain HTTP). `echo` auth keeps the real
/// keystore out of it.
fn add_hub(editor: &mut Editor, name: &str, url: String) {
    editor
        .kb
        .registry
        .register_remote_hub(
            name.into(),
            url,
            "hub-kb".into(),
            RemoteHubAuth::Command("echo test-token".into()),
        )
        .expect("register hub row");
    editor.kb.rebuild_query_layer();
}

fn local_note(editor: &mut Editor, id: &str, body: &str) {
    editor
        .kb
        .primary
        .insert(mae_kb::Node::new(id, id, mae_kb::NodeKind::Note, body));
}

fn hits(editor: &Editor, q: &str, scope: &KbScope) -> Vec<(Option<String>, String)> {
    editor
        .kb_federated_search_scoped(q, scope)
        .into_iter()
        .map(|(inst, n)| (inst, n.id))
        .collect()
}

#[cfg(unix)]
#[test]
fn a_hub_hit_appears_labelled_and_obeys_every_scope() {
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);
    local_note(&mut editor, "note:local", "zebra crossing");
    let url = spawn_hub(&[("hub:far", "Zebra hub", "a zebra from afar")]);
    add_hub(&mut editor, "TeamHub", url);

    let hub_hit = (Some("TeamHub".to_string()), "hub:far".to_string());
    let all = hits(&editor, "zebra", &KbScope::All);
    assert!(all.contains(&hub_hit), "All includes the hub: {all:?}");
    assert!(
        all.iter().any(|(_, id)| id == "note:local"),
        "…and local notes"
    );

    for (scope, expect) in [
        (KbScope::RemoteOnly, true),
        (KbScope::Named("TeamHub".into()), true),
        (KbScope::LocalOnly, false),
        (KbScope::Named("SomethingElse".into()), false),
    ] {
        let got = hits(&editor, "zebra", &scope);
        assert_eq!(got.contains(&hub_hit), expect, "{scope:?}: {got:?}");
    }
    assert!(
        editor.kb.last_search_incomplete().is_empty(),
        "a hub that answered is not incomplete"
    );
}

/// The failure mode that matters most: a partial answer must not read as a
/// complete one.
#[cfg(unix)]
#[test]
fn an_unreachable_hub_leaves_local_results_and_is_reported_incomplete() {
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);
    local_note(&mut editor, "note:local", "zebra crossing");
    add_hub(&mut editor, "DownHub", dead_url());

    let got = hits(&editor, "zebra", &KbScope::All);
    assert_eq!(got, vec![(None, "note:local".to_string())]);
    assert_eq!(
        editor.kb.last_search_incomplete(),
        vec!["DownHub".to_string()]
    );

    // Out of scope, it was never asked — so it is not reported either.
    let _ = hits(&editor, "zebra", &KbScope::LocalOnly);
    assert!(editor.kb.last_search_incomplete().is_empty());
}

/// A daemon-hosted primary used to shadow the federated layer — and with it
/// every hub — for every non-search read (`kb_get`, links, graph …).
#[cfg(unix)]
#[test]
fn a_daemon_query_layer_does_not_hide_the_hub_from_reads() {
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);
    let url = spawn_hub(&[("hub:far", "Far", "body")]);
    add_hub(&mut editor, "TeamHub", url);
    let daemon = std::sync::Arc::new(mae_kb::InMemoryQueryLayer::new(mae_kb::KnowledgeBase::new()));
    editor.kb.set_daemon_query_layer(Some(daemon));

    let got = editor.kb.query_layer().and_then(|q| q.get("hub:far"));
    assert_eq!(got.map(|n| n.title), Some("Far".to_string()));
}

/// The editor's event loop runs inside `Runtime::block_on`. Rebuilding drops
/// the previous hub layers there; a bare blocking client panics on that drop.
#[cfg(unix)]
#[test]
fn hub_layers_survive_rebuilds_inside_a_runtime() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut editor = Editor::new();
            let _d = with_test_dirs(&mut editor);
            let url = spawn_hub(&[("hub:far", "Zebra", "")]);
            add_hub(&mut editor, "TeamHub", url);
            let _ = hits(&editor, "zebra", &KbScope::All);
            editor.kb.rebuild_query_layer(); // drops the first set of hub layers
            editor.kb.rebuild_query_layer();
            let again = hits(&editor, "zebra", &KbScope::All);
            assert!(again.iter().any(|(_, id)| id == "hub:far"));
        });
}

#[test]
fn register_hub_refuses_plain_http_and_accepts_https_with_keystore_auth() {
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);

    for bad in [
        "http://kb.example.org",
        "ftp://kb.example.org",
        "https://",
        "kb.example.org",
    ] {
        assert!(
            editor.kb_register_hub("Hub", bad, "kb", "key").is_err(),
            "{bad} must be refused"
        );
    }
    assert!(
        editor.kb.registry.find("Hub").is_none(),
        "no refused row persisted"
    );

    editor
        .kb_register_hub("Hub", "https://kb.example.org:8443/", "team", "hub-token")
        .expect("https is accepted");
    let inst = editor.kb.registry.find("Hub").expect("registered");
    let hub = inst.remote_hub.as_ref().expect("hub config");
    assert_eq!(hub.base_url, "https://kb.example.org:8443");
    assert_eq!(hub.auth, RemoteHubAuth::KeystoreKey("hub-token".into()));

    // Persisted, not only in memory: a fresh read of the registry file has it.
    let data_dir = editor.mae_data_dir().expect("data dir");
    let on_disk = mae_kb::federation::KbRegistry::load(&data_dir);
    assert!(on_disk.find("Hub").is_some());
}

#[test]
fn register_hub_refuses_a_name_a_local_kb_already_has() {
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);
    let dir = TempDir::new().unwrap();
    editor.kb_register("Notes", dir.path()).expect("local KB");

    let err = editor
        .kb_register_hub("notes", "https://kb.example.org", "team", "key")
        .expect_err("taken name");
    assert!(err.contains("already taken"), "{err}");
    assert_eq!(
        editor
            .kb
            .registry
            .instances
            .iter()
            .filter(|i| i.remote_hub.is_some())
            .count(),
        0
    );
}

#[test]
fn the_config_fields_the_command_writes_are_the_ones_the_layer_reads() {
    // Guards the seam between registration and rebuild: a row the command
    // writes must produce exactly one live hub layer.
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);
    assert!(editor.kb.remote_hubs().is_empty());
    editor
        .kb_register_hub("Hub", "https://kb.example.org", "team", "key")
        .unwrap();
    let names: Vec<&str> = editor
        .kb
        .remote_hubs()
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, vec!["Hub"]);
}
