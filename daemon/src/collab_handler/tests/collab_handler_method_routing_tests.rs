//! ADVERSARIAL: a message is routed by what it PARSES to, not by how it is
//! spelled.
//!
//! JSON lets a method name be written with escapes (`"sync\/update"`,
//! `"sync/update"`). Every parser reads those as `sync/update`. The
//! session loop used to classify messages by searching the raw text, so an
//! escaped spelling missed the access-checked document handler and reached
//! the generic request handler's sync dispatch, which checked nothing. These
//! tests drive the real session loop, over a pipe, as a key that is not a
//! member of the KB, and assert on the document itself.

use super::*;
use tokio::io::BufReader;

const KB: &str = "kb-routing";
const NODE: &str = "concept:secret";
const OWNER: &str = "alice";
const STRANGER: &str = "mallory";

fn doc_name() -> String {
    format!("kbn:{KB}:{NODE}")
}

/// A KB owned by `OWNER` with one node holding real content.
async fn seed(store: &Arc<DocStore>, bc: &SharedBroadcaster) {
    let mut docs = HashSet::new();
    kb_share_as(
        store,
        bc,
        Some(OWNER),
        Some(&fp(OWNER)),
        KB,
        OWNER,
        &mut docs,
    )
    .await;
    let added = dispatch_as(
        store,
        bc,
        Some(OWNER),
        Some(&fp(OWNER)),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":KB,"node_id":NODE,"title":"secret"}}),
        &mut docs,
    )
    .await;
    assert!(added.error.is_none(), "seed failed: {:?}", added.error);
    let update = make_test_node(NODE, "secret", "ORIGINAL-PLAINTEXT", &[]);
    let wrote = dispatch_as(
        store,
        bc,
        Some(OWNER),
        Some(&fp(OWNER)),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"sync/update",
            "params":{"doc":doc_name(),"kb_id":KB,"update":update_to_base64(&update)}}),
        &mut docs,
    )
    .await;
    assert!(
        wrote.error.is_none(),
        "seed content failed: {:?}",
        wrote.error
    );
}

/// A live session for `who`, over an in-memory pipe, as an authenticated key.
struct Session {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    read: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
}

impl Session {
    fn start(store: &Arc<DocStore>, bc: &SharedBroadcaster, who: &str) -> Session {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (server_read, server_write) = tokio::io::split(server);
        let peer = mae_mcp::identity::PeerIdentity {
            label: who.to_string(),
            fingerprint: fp(who),
            pubkey: [7u8; 32],
        };
        let (store, bc) = (Arc::clone(store), Arc::clone(bc));
        tokio::spawn(async move {
            handle_client_authenticated(
                BufReader::new(server_read),
                server_write,
                peer,
                store,
                bc,
                std::time::Instant::now(),
                Transport::Hub,
                Arc::new(crate::artifact_store::NoArtifactStore),
                Arc::new(crate::quota::NoQuota),
                crate::kb_query::KbQueryLimits::default(),
                None,
            )
            .await;
        });
        let (read, write) = tokio::io::split(client);
        Session {
            write,
            read: BufReader::new(read),
        }
    }

    /// Send `raw` exactly as written and return the reply.
    async fn ask(&mut self, raw: &str) -> serde_json::Value {
        let line = format!("{raw}\n");
        tokio::io::AsyncWriteExt::write_all(&mut self.write, line.as_bytes())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut self.write)
            .await
            .unwrap();
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            mae_mcp::read_message(&mut self.read),
        )
        .await
        .expect("no reply within 5s")
        .unwrap()
        .unwrap();
        serde_json::from_str(&reply).unwrap()
    }
}

/// The same method, spelled four ways: plain, and three escapes JSON allows.
fn spellings(method: &str) -> Vec<String> {
    let first = method.chars().next().unwrap();
    vec![
        method.to_string(),
        method.replace('/', "\\/"),
        format!("\\u{:04x}{}", first as u32, &method[first.len_utf8()..]),
        method.replace('/', "\\u002f"),
    ]
}

/// A non-member's write to another KB's node is refused under every
/// spelling, and the node is byte-for-byte unchanged.
#[tokio::test]
async fn an_escaped_sync_update_from_a_non_member_changes_nothing() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;
    let before = store.encode_state(&doc_name()).await.unwrap();

    let forged = make_test_node(NODE, "secret", "WRITTEN-BY-A-STRANGER", &[]);
    let mut s = Session::start(&store, &bc, STRANGER);
    for (i, method) in spellings("sync/update").into_iter().enumerate() {
        let raw = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"doc":"{doc}","kb_id":"{KB}","update":"{u}"}}}}"#,
            id = i + 1,
            doc = doc_name(),
            u = update_to_base64(&forged),
        );
        let reply = s.ask(&raw).await;
        assert!(
            reply.get("error").is_some_and(|e| !e.is_null()),
            "{method}: a non-member's write was answered: {reply}"
        );
    }
    let after = store.encode_state(&doc_name()).await.unwrap();
    assert_eq!(before, after, "the node changed");
}

/// A non-member cannot read another KB's node under any spelling.
#[tokio::test]
async fn an_escaped_sync_read_from_a_non_member_returns_nothing() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;

    let mut s = Session::start(&store, &bc, STRANGER);
    let mut id = 0;
    for base in ["sync/full_state", "sync/state_vector"] {
        for method in spellings(base) {
            id += 1;
            let raw = format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"doc":"{doc}","kb_id":"{KB}"}}}}"#,
                doc = doc_name(),
            );
            let reply = s.ask(&raw).await;
            assert!(
                reply.get("error").is_some_and(|e| !e.is_null()),
                "{method}: a non-member's read was answered: {reply}"
            );
        }
    }
}

/// `sync/enable` (no document) is still answered on the generic path, and the
/// owner's own escaped write is served by the document handler, so routing by
/// the parsed method changes who is checked, not what works.
#[tokio::test]
async fn sync_enable_and_a_members_escaped_write_still_work() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;

    let mut s = Session::start(&store, &bc, OWNER);
    let reply = s
        .ask(r#"{"jsonrpc":"2.0","id":1,"method":"sync\/enable","params":{}}"#)
        .await;
    assert_eq!(reply["result"]["sync_enabled"], true, "{reply}");

    let edit = make_test_node(NODE, "secret", "EDITED-BY-THE-OWNER", &[]);
    let raw = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"sync\/update","params":{{"doc":"{doc}","kb_id":"{KB}","update":"{u}"}}}}"#,
        doc = doc_name(),
        u = update_to_base64(&edit),
    );
    let reply = s.ask(&raw).await;
    assert!(
        reply.get("error").is_none_or(|e| e.is_null()),
        "the owner's write was refused: {reply}"
    );
}

/// Classification reads the parsed message: an escaped `id` key still makes
/// it a request, and an `"id"` that is only a value is not a key.
#[test]
fn notification_status_comes_from_the_parsed_id_key() {
    let escaped_id = r#"{"jsonrpc":"2.0","id":3,"method":"sync/awareness","params":{}}"#;
    assert!(!Route::of(escaped_id).is_notification);
    let id_as_value = r#"{"jsonrpc":"2.0","method":"sync/awareness","params":{"note":"id"}}"#;
    assert!(Route::of(id_as_value).is_notification);
    for m in spellings("sync/update") {
        let raw = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{m}"}}"#);
        assert!(Route::of(&raw).is_doc, "{m} not routed to the doc handler");
    }
    assert!(!Route::of(r#"{"jsonrpc":"2.0","id":1,"method":"sync/enable"}"#).is_doc);
    assert!(!Route::of("not json").is_doc);
}

/// The generic tool path, which checks nothing, refuses every document sync
/// method on its own, so a routing mistake cannot reach the document store
/// through it (defence in depth). `sync/enable` touches no document.
#[tokio::test]
async fn the_unchecked_tool_path_refuses_every_document_sync_method() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;
    let before = store.encode_state(&doc_name()).await.unwrap();
    let forged = make_test_node(NODE, "secret", "VIA-THE-TOOL-PATH", &[]);
    let args = serde_json::json!({"doc": doc_name(), "update": update_to_base64(&forged)});
    for tool in [
        "__mcp_sync_update",
        "__mcp_sync_full_state",
        "__mcp_sync_state_vector",
    ] {
        let r = handle_sync_tool(tool, &args, &store, &bc).await;
        assert!(!r.success, "{tool} was served: {}", r.output);
        assert!(
            !r.output.contains("ORIGINAL-PLAINTEXT"),
            "{tool} leaked content"
        );
    }
    assert_eq!(store.encode_state(&doc_name()).await.unwrap(), before);
    assert!(
        handle_sync_tool("__mcp_sync_enable", &args, &store, &bc)
            .await
            .success
    );
}
