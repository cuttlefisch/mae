//! ADR-116 D2 through real traffic: members of a shared KB edit one node with
//! `sync/update`, as their clients would, and the daemon cuts a version at
//! each handoff. Each version's editors and WAL range are checked against the
//! editor record, which is written independently of history.

use super::*;
use mae_sync::kb::history::{verify_chain, HistoryEntry, VersionKind};
use mae_sync::kb::KbNodeDoc;

const KB: &str = "kbhist";
const NODE: &str = "n";

struct Env {
    store: Arc<DocStore>,
    bc: SharedBroadcaster,
    docs: HashSet<String>,
}

fn node_doc() -> String {
    mae_sync::kb_node_doc_name(KB, NODE)
}

/// A KB owned by alice, with bob and carol as editors (dave is no member).
async fn shared_kb() -> Env {
    let mut env = Env {
        store: test_doc_store(),
        bc: test_broadcaster(),
        docs: HashSet::new(),
    };
    let alice = fp("alice");
    kb_share_as(
        &env.store,
        &env.bc,
        Some("alice"),
        Some(&alice),
        KB,
        "alice",
        &mut env.docs,
    )
    .await;
    for member in [fp("bob"), fp("carol")] {
        let add = kb_member_msg("kb/add_member", KB, &member, Some("editor"));
        let r = dispatch_as(
            &env.store,
            &env.bc,
            Some("alice"),
            Some(&alice),
            add,
            &mut env.docs,
        )
        .await;
        assert!(r.error.is_none(), "admit: {:?}", r.error);
        let join = dispatch_as(
            &env.store,
            &env.bc,
            None,
            Some(&member),
            kb_join_msg(KB),
            &mut env.docs,
        )
        .await;
        assert!(join.error.is_none(), "join: {:?}", join.error);
    }
    env
}

/// `who` sends one `sync/update` made by `edit` on the node as the daemon holds
/// it (a new node, when it has none), with their own fenced client id.
async fn edit(
    env: &mut Env,
    who: &str,
    edit: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>,
) -> JsonRpcResponse {
    let cid = derive_kb_client_id(&fp(who), 0);
    let update = if env.store.has_durable_doc(&node_doc()).await {
        let state = env.store.encode_state(&node_doc()).await.unwrap();
        let mut node = KbNodeDoc::from_bytes_with_client_id(&state, cid).unwrap();
        edit(&mut node)
    } else {
        let mut node = KbNodeDoc::new_with_client_id(NODE, "", "", &[], cid);
        let _ = edit(&mut node);
        node.encode()
    };
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
        "params":{"doc":node_doc(),"kb_id":KB,"update":update_to_base64(&update)}});
    dispatch_as(
        &env.store,
        &env.bc,
        None,
        Some(&fp(who)),
        msg,
        &mut env.docs,
    )
    .await
}

/// `who` retitles the node; returns the WAL id the update landed at.
async fn retitle(env: &mut Env, who: &str, title: &str) -> u64 {
    let r = edit(env, who, |n| n.set_title(title)).await;
    assert!(r.error.is_none(), "{who} retitles {title:?}: {:?}", r.error);
    env.store.wal_high_water(&node_doc()).await.unwrap()
}

async fn history(env: &Env) -> Vec<HistoryEntry> {
    env.store.history_list(KB, NODE).await.unwrap()
}

fn titles(h: &[HistoryEntry]) -> Vec<String> {
    h.iter().map(|e| e.node().unwrap().unwrap().title).collect()
}

fn editor_sets(h: &[HistoryEntry]) -> Vec<Vec<String>> {
    h.iter()
        .map(|e| e.editors.iter().map(|ed| ed.principal.clone()).collect())
        .collect()
}

/// Alice creates the node; bob, carol and bob again take over. Each handoff
/// cuts the state the previous principal left, attributed to them; bob's
/// second edit in his own session cuts nothing. Kills a cut taken after the
/// update (wrong content), one attributed to the incoming principal, and one
/// cut on every update.
#[tokio::test]
async fn each_handoff_cuts_the_state_the_previous_session_left() {
    let mut env = shared_kb().await;
    let a = retitle(&mut env, "alice", "by alice").await;
    retitle(&mut env, "bob", "bob 1").await;
    let b = retitle(&mut env, "bob", "bob 2").await;
    let c = retitle(&mut env, "carol", "by carol").await;
    retitle(&mut env, "bob", "bob 3").await;
    let h = history(&env).await;
    assert_eq!(titles(&h), ["by alice", "bob 2", "by carol"]);
    assert_eq!(
        editor_sets(&h),
        [vec![fp("alice")], vec![fp("bob")], vec![fp("carol")]]
    );
    let ranges: Vec<(u64, u64)> = h.iter().map(|e| (e.wal_from, e.wal_to)).collect();
    assert_eq!(
        ranges,
        [(0, a), (a, b), (b, c)],
        "each covers its session's updates"
    );
    assert!(h.iter().all(|e| e.kind == VersionKind::Edit));
    let last = h.last().unwrap();
    assert!(verify_chain(KB, NODE, &h, Some((last.seq, last.entry_hash))).is_ok());
}

/// A long session by one principal cuts no version, however many updates.
#[tokio::test]
async fn a_long_single_principal_session_cuts_nothing() {
    let mut env = shared_kb().await;
    for i in 0..25 {
        retitle(&mut env, "alice", &format!("draft {i}")).await;
    }
    assert!(history(&env).await.is_empty());
    retitle(&mut env, "bob", "bob").await;
    assert_eq!(titles(&history(&env).await), ["draft 24"]);
}

/// A handoff that finds the content unchanged since the last version writes
/// nothing, and still starts a new session, so the handoff after it is cut.
/// Kills a no-op cut that writes, and a session left open after one.
#[tokio::test]
async fn an_unchanged_handoff_writes_nothing_but_starts_a_session() {
    let mut env = shared_kb().await;
    retitle(&mut env, "alice", "A").await;
    retitle(&mut env, "bob", "B").await;
    retitle(&mut env, "bob", "A").await;
    retitle(&mut env, "carol", "C").await;
    assert_eq!(
        titles(&history(&env).await),
        ["A"],
        "bob left it as the last version had it"
    );
    retitle(&mut env, "bob", "D").await;
    let h = history(&env).await;
    assert_eq!(titles(&h), ["A", "C"]);
    assert_eq!(
        editor_sets(&h)[1],
        vec![fp("bob"), fp("carol")],
        "everyone since the last version"
    );
}

/// A refused write cuts nothing: a non-member's, and a member's that would
/// leave a field mistyped. The next accepted write by that member is the
/// handoff.
#[tokio::test]
async fn a_refused_write_cuts_nothing() {
    let mut env = shared_kb().await;
    retitle(&mut env, "alice", "A").await;
    let r = edit(&mut env, "dave", |n| n.set_title("dave")).await;
    assert!(r.error.is_some(), "precondition: dave is refused");
    let r = edit(&mut env, "bob", mistype_title).await;
    assert!(
        r.error.is_some(),
        "precondition: a mistyped field is refused"
    );
    assert!(history(&env).await.is_empty());
    retitle(&mut env, "bob", "B").await;
    assert_eq!(titles(&history(&env).await), ["A"]);
}

/// An update, by `n`'s own client, setting the title to a plain string
/// instead of text.
fn mistype_title(n: &mut KbNodeDoc) -> Vec<u8> {
    use mae_sync::yrs::updates::decoder::Decode;
    use mae_sync::yrs::{Map, ReadTxn, Transact};
    let doc = mae_sync::yrs::Doc::with_client_id(n.client_id());
    let state = n.encode_state();
    doc.transact_mut()
        .apply_update(mae_sync::yrs::Update::decode_v1(&state).unwrap())
        .unwrap();
    let sv = doc.transact().state_vector();
    let root = doc.get_or_insert_map("node");
    root.insert(&mut doc.transact_mut(), "title", "plain");
    let out = doc.transact().encode_state_as_update_v1(&sv);
    out
}

/// A node document that does not read as a node (plain text here; E2E
/// ciphertext likewise) is never versioned, whoever edits it.
#[tokio::test]
async fn a_document_that_is_not_a_readable_node_is_not_versioned() {
    let mut env = shared_kb().await;
    for (who, text) in [("alice", "a"), ("bob", "b"), ("carol", "c")] {
        let msg = kb_node_update_msg_as(KB, &fp(who), 0, text);
        let r = dispatch_as(
            &env.store,
            &env.bc,
            None,
            Some(&fp(who)),
            msg,
            &mut env.docs,
        )
        .await;
        assert!(r.error.is_none(), "{who}: {:?}", r.error);
    }
    let h = env.store.history_list(KB, "concept:n").await.unwrap();
    assert!(h.is_empty(), "{h:?}");
}

async fn checkpoint(env: &mut Env, who: &str, node: &str, name: &str) -> JsonRpcResponse {
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/history.checkpoint",
        "params":{"kb_id":KB,"node_id":node,"name":name}});
    dispatch_as(
        &env.store,
        &env.bc,
        None,
        Some(&fp(who)),
        msg,
        &mut env.docs,
    )
    .await
}

fn error_kind(r: &JsonRpcResponse) -> String {
    let e = r.error.as_ref().expect("an error");
    e.data.as_deref().map_or(String::new(), |d| {
        d["kind"].as_str().unwrap_or("").to_string()
    })
}

/// A named version holds the node as it stands, is written even when the
/// content repeats the previous version, records who named it, and starts a
/// new session: the namer's next edit cuts nothing, the next handoff does.
#[tokio::test]
async fn a_named_version_is_cut_on_request_and_starts_a_session() {
    let mut env = shared_kb().await;
    retitle(&mut env, "alice", "A").await;
    retitle(&mut env, "bob", "B").await;
    let r = checkpoint(&mut env, "bob", NODE, "release 1").await;
    let v = r.result.as_ref().unwrap_or_else(|| panic!("{:?}", r.error));
    assert_eq!(
        (v["kind"].as_str(), v["name"].as_str(), v["seq"].as_u64()),
        (Some("named"), Some("release 1"), Some(2))
    );
    let r = checkpoint(&mut env, "bob", NODE, "again").await;
    assert!(
        r.error.is_none(),
        "a second name on the same content is written: {:?}",
        r.error
    );
    retitle(&mut env, "bob", "B2").await;
    retitle(&mut env, "carol", "C").await;
    let h = history(&env).await;
    assert_eq!(titles(&h), ["A", "B", "B", "B2"]);
    assert_eq!(
        h[1].summary.as_deref(),
        Some(format!("named by {}", fp("bob")).as_str())
    );
    assert_eq!(
        editor_sets(&h)[1],
        vec![fp("bob")],
        "naming does not make one an editor"
    );
    assert!(
        editor_sets(&h)[2].is_empty(),
        "no one edited between the two names"
    );
    let last = h.last().unwrap();
    assert!(verify_chain(KB, NODE, &h, Some((last.seq, last.entry_hash))).is_ok());
}

/// Naming needs `Edit`: a non-member and a viewer are refused and write
/// nothing. A bad name, a missing node and an E2E KB are refused too.
#[tokio::test]
async fn a_named_version_is_refused_where_it_must_be() {
    let mut env = shared_kb().await;
    let (alice, vic) = (fp("alice"), fp("vic"));
    let add = kb_member_msg("kb/add_member", KB, &vic, Some("viewer"));
    let r = dispatch_as(
        &env.store,
        &env.bc,
        Some("alice"),
        Some(&alice),
        add,
        &mut env.docs,
    )
    .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    retitle(&mut env, "alice", "A").await;
    for who in ["dave", "vic"] {
        assert!(
            checkpoint(&mut env, who, NODE, "n").await.error.is_some(),
            "{who}"
        );
    }
    for bad in ["", "   ", "a\nb", &"x".repeat(201)] {
        assert_eq!(
            error_kind(&checkpoint(&mut env, "alice", NODE, bad).await),
            "invalid_params",
            "{bad:?}"
        );
    }
    assert!(checkpoint(&mut env, "alice", NODE, &"x".repeat(200))
        .await
        .error
        .is_none());
    assert_eq!(
        error_kind(&checkpoint(&mut env, "alice", "missing", "n").await),
        "doc_not_found"
    );
    let kbc = format!("kbc:{KB}");
    let state = env.store.encode_state(&kbc).await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let up = coll.set_encryption(mae_sync::kb::Encryption::E2e);
    let by =
        crate::doc_editors::Attribution::Internal(crate::doc_editors::InternalWriter::Scheduler);
    env.store.apply_update(&kbc, &up, None, by).await.unwrap();
    assert_eq!(
        error_kind(&checkpoint(&mut env, "alice", NODE, "n").await),
        "e2e_unsupported"
    );
    assert_eq!(
        history(&env).await.len(),
        1,
        "only the one valid name was written"
    );
}

/// The owner's `kb/collection_op` with a collection update made by `edit`.
async fn collection_op(
    env: &mut Env,
    edit: impl FnOnce(&mut KbCollectionDoc) -> Vec<u8>,
) -> JsonRpcResponse {
    let state = env.store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let up = edit(&mut coll);
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/collection_op",
        "params":{"kb_id":KB,"update":update_to_base64(&up)}});
    let alice = fp("alice");
    dispatch_as(
        &env.store,
        &env.bc,
        Some("alice"),
        Some(&alice),
        msg,
        &mut env.docs,
    )
    .await
}

async fn is_e2e(env: &Env) -> bool {
    let state = env.store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    matches!(
        KbCollectionDoc::from_bytes(&state).unwrap().encryption(),
        mae_sync::kb::Encryption::E2e
    )
}

/// ADR-116 Amendment 1 (interim): switching a KB to E2E is refused while its
/// history holds plaintext content, and the collection is left as it was. A
/// collection op that does not switch is not refused, and a KB with no
/// history switches. Kills a check that refuses every collection op, one that
/// never refuses, and one that applies the switch before refusing it.
#[tokio::test]
async fn switching_to_e2e_is_refused_while_history_holds_plaintext() {
    let mut fresh = shared_kb().await;
    let r = collection_op(&mut fresh, |c| {
        c.set_encryption(mae_sync::kb::Encryption::E2e)
    })
    .await;
    assert!(
        r.error.is_none(),
        "no history: the switch is allowed: {:?}",
        r.error
    );
    assert!(is_e2e(&fresh).await);

    let mut env = shared_kb().await;
    retitle(&mut env, "alice", "A").await;
    retitle(&mut env, "bob", "B").await;
    assert_eq!(
        history(&env).await.len(),
        1,
        "precondition: history holds a version"
    );
    let r = collection_op(&mut env, |c| {
        c.set_encryption(mae_sync::kb::Encryption::E2e)
    })
    .await;
    assert_eq!(error_kind(&r), "history_holds_plaintext");
    assert!(!is_e2e(&env).await, "the refused switch was not applied");
    let r = collection_op(&mut env, |c| {
        c.upsert_member(&fp("erin"), "erin", SyncRole::Viewer)
    })
    .await;
    assert!(
        r.error.is_none(),
        "an op that does not switch is not refused: {:?}",
        r.error
    );
    // Already E2E (as a flip relayed from a mesh peer leaves it): later
    // collection ops are not switches, and are not refused.
    let kbc = format!("kbc:{KB}");
    let state = env.store.encode_state(&kbc).await.unwrap();
    let up = KbCollectionDoc::from_bytes(&state)
        .unwrap()
        .set_encryption(mae_sync::kb::Encryption::E2e);
    let by =
        crate::doc_editors::Attribution::Internal(crate::doc_editors::InternalWriter::Scheduler);
    env.store.apply_update(&kbc, &up, None, by).await.unwrap();
    let r = collection_op(&mut env, |c| {
        c.upsert_member(&fp("finn"), "finn", SyncRole::Viewer)
    })
    .await;
    assert!(
        r.error.is_none(),
        "an already-E2E KB is not refused: {:?}",
        r.error
    );
}
