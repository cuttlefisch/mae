//! ADVERSARIAL (#718 / ADR-105): two KBs may contain the SAME node id.
//!
//! This is the input choice that matters, and it is the reason the bug survived an
//! adversarial review. `collab_handler_cross_kb_node_isolation_tests` — written
//! specifically to attack cross-KB isolation — seeds `concept:a-own` in one KB and
//! `concept:b-secret` in the other. Deliberately *different* ids. That test proves
//! the authorization property and is structurally incapable of observing the
//! addressing collision underneath it, because two different ids never collide no
//! matter how the address is built.
//!
//! Every test here therefore uses the SAME node id in both KBs. Nothing forbids
//! that: node ids are human-authored (`concept:architecture`, `task:onboarding`),
//! and two tenants picking the same one is ordinary, not adversarial.
//!
//! The attacker here is mostly not an attacker at all — findings 1 and 2 are two
//! HONEST tenants interfering with each other. That is what makes this an
//! addressing defect rather than an authorization one.

use super::*;

/// Share `kb_id` as `who`, with `node_id` already carrying `body`.
///
/// Rolls its own share rather than reusing `share_kb_with_nodes`, which dispatches
/// ANONYMOUSLY — with no principal `kb_access` admits everyone, so an isolation test
/// built on it would be vacuous. Seeding content at share time (rather than adding an
/// empty manifest entry) matters too: `kb/node_update` carries a yrs *delta*, so a
/// node needs a base document for a later edit to apply onto.
async fn share_kb_with_node_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    who: &str,
    kb_id: &str,
    node_id: &str,
    body: &str,
    docs: &mut HashSet<String>,
) -> Vec<u8> {
    let node = make_test_node(node_id, "Architecture", body, &[]);
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb_id, "", who);
    coll.add_node(node_id, node_id);
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"kb/share",
        "params":{
            "kb_id": kb_id,
            "name": kb_id,
            "creator": who,
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [{"id": node_id, "state": update_to_base64(&node)}],
        }
    });
    let r = dispatch_as(store, bc, Some(who), Some(&fp(who)), msg, docs).await;
    assert!(r.error.is_none(), "{who} share failed: {:?}", r.error);
    node
}

/// Edit `node_id` in `kb_id` as `who`, as a delta from `base`.
#[allow(clippy::too_many_arguments)]
async fn edit_node(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    who: &str,
    kb_id: &str,
    node_id: &str,
    base: &[u8],
    body: &str,
    docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    // Author under the EPOCH-derived client id, as a real editor does
    // (`kb_ops::nodes` calls `derive_kb_client_id`). Authoring under the default
    // client id instead makes the ADR-023 fence reject the edit — a property of the
    // harness, not of the code under test.
    let cid = mae_sync::kb::derive_kb_client_id(&fp(who), 0);
    let mut doc =
        mae_sync::kb::KbNodeDoc::from_bytes_with_client_id(base, cid).expect("base node decodes");
    let update = doc.set_body(body);
    dispatch_as(
        store,
        bc,
        Some(who),
        Some(&fp(who)),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":node_id,
                      "update": update_to_base64(&update)}}),
        docs,
    )
    .await
}

/// FINDING 2 (the honest case, and the one no existing test could see): two tenants
/// who each use `concept:architecture` in their own KB must not share a document.
///
/// No edit is even needed — each tenant simply *shares* their own KB with their own
/// content under the same node id. If node docs are globally addressed, the second
/// share lands on the first's document.
#[tokio::test]
async fn two_tenants_using_the_same_node_id_do_not_share_content() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let node = "concept:architecture";
    let (mut a_docs, mut b_docs) = (HashSet::new(), HashSet::new());

    share_kb_with_node_as(
        &store,
        &bc,
        "alice",
        "kb-a",
        node,
        "ALICE-PRIVATE",
        &mut a_docs,
    )
    .await;
    share_kb_with_node_as(&store, &bc, "bob", "kb-b", node, "BOB-PRIVATE", &mut b_docs).await;

    // Each tenant reads their OWN KB, through their OWN membership.
    let a_body = read_node_body(&store, &bc, "alice", "kb-a", node, &mut a_docs).await;
    let b_body = read_node_body(&store, &bc, "bob", "kb-b", node, &mut b_docs).await;

    assert!(
        a_body.contains("ALICE-PRIVATE") && !a_body.contains("BOB-PRIVATE"),
        "alice must see only her own content, got: {a_body:?}"
    );
    assert!(
        b_body.contains("BOB-PRIVATE") && !b_body.contains("ALICE-PRIVATE"),
        "bob must see only his own content, got: {b_body:?}"
    );
}

/// Regression guard: scoping must not INTRODUCE epoch-fence interference.
///
/// **This test does not demonstrate the bug, and an earlier version of this comment
/// wrongly claimed it did.** While researching ADR-105 a tenant's own edit was seen
/// failing with `rebase required: … carries an op from stale-epoch client …`, and
/// that was attributed to two KBs sharing one document. It was not. The real cause
/// was the test harness authoring the edit under the DEFAULT yrs client id instead
/// of the epoch-derived one (`derive_kb_client_id`, what `kb_ops::nodes` uses) — the
/// ADR-023 fence was correctly rejecting a genuinely stale-epoch op.
///
/// Measured with the harness corrected: this passes both before and after the
/// scoping change, so it is evidence of nothing about the addressing bug. It is kept
/// as a one-directional guard — a tenant's own edit must keep working when another
/// tenant holds a same-named node — because that is a property scoping could
/// plausibly break, and it costs one test to know it doesn't.
#[tokio::test]
async fn a_tenants_own_edit_is_not_fenced_by_another_tenants_same_named_node() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let node = "concept:architecture";
    let (mut a_docs, mut b_docs) = (HashSet::new(), HashSet::new());

    let a_base =
        share_kb_with_node_as(&store, &bc, "alice", "kb-a", node, "alice v1", &mut a_docs).await;
    share_kb_with_node_as(&store, &bc, "bob", "kb-b", node, "bob v1", &mut b_docs).await;

    // Alice edits HER OWN node. Nothing about bob's KB may affect this.
    let ra = edit_node(
        &store,
        &bc,
        "alice",
        "kb-a",
        node,
        &a_base,
        "alice v2",
        &mut a_docs,
    )
    .await;
    let err = ra.error.map(|e| e.message).unwrap_or_default();
    assert!(
        err.is_empty(),
        "alice's edit to HER OWN kb was rejected because another tenant holds a \
         same-named node: {err}"
    );
    let a_body = read_node_body(&store, &bc, "alice", "kb-a", node, &mut a_docs).await;
    assert!(
        a_body.contains("alice v2"),
        "alice's edit must actually land, got: {a_body:?}"
    );
}

/// FINDING 1 (the authorization case #718 was filed as): a member of one KB must not
/// be able to write another KB's node of the same name.
///
/// Note the shape carefully — an earlier draft of this test had it wrong and passed
/// for the wrong reason. Mallory does NOT pass the victim's `kb_id`; `kb_access`
/// would deny that outright, since she has no membership there. She passes **her
/// own** `kb_id`, which she is fully authorized for, together with a node id that
/// also exists in the victim's KB. The write is authorized against `kb-a` and then
/// lands on a globally-addressed node document — which is precisely #718.
///
/// The oracle is the victim's CONTENT, not the response: the call legitimately
/// succeeds for Mallory's own KB, so a response-only assertion would see nothing
/// wrong at all.
#[tokio::test]
async fn writing_your_own_kbs_node_must_not_reach_another_kbs_node_of_the_same_name() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let node = "concept:architecture";
    let (mut m_docs, mut v_docs) = (HashSet::new(), HashSet::new());

    let m_base = share_kb_with_node_as(
        &store,
        &bc,
        "mallory",
        "kb-a",
        node,
        "mallory own",
        &mut m_docs,
    )
    .await;
    share_kb_with_node_as(
        &store,
        &bc,
        "victim",
        "kb-b",
        node,
        "VICTIM-ORIGINAL",
        &mut v_docs,
    )
    .await;

    // Authorized for kb-a, which she owns. Same node id as the victim's.
    let r = edit_node(
        &store,
        &bc,
        "mallory",
        "kb-a",
        node,
        &m_base,
        "MALLORY-WAS-HERE",
        &mut m_docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "mallory's write to her OWN kb is legitimate and must succeed: {:?}",
        r.error
    );

    let victim_body = read_node_body(&store, &bc, "victim", "kb-b", node, &mut v_docs).await;
    assert!(
        !victim_body.contains("MALLORY-WAS-HERE"),
        "a write authorized against kb-a reached kb-b's node: {victim_body:?}"
    );
    assert!(
        victim_body.contains("VICTIM-ORIGINAL"),
        "the victim's own content must survive intact, got: {victim_body:?}"
    );
}

/// ADVERSARIAL (ADR-105 D3): the collision the scoped address removed comes
/// straight back if a KB id may contain a colon.
///
/// `kbn:{kb_id}:{node_id}` splits on the FIRST colon, so KB `a:b` holding node `c`
/// and KB `a` holding node `b:c` both spell `kbn:a:b:c` — one document, two
/// tenants, which is #718 verbatim. Node ids legitimately contain colons
/// (`concept:architecture`), so the ambiguity can only be closed on the KB id
/// side, and `kb_id` arrives client-supplied on `kb/share`.
///
/// D3's `kb_id_is_addressable` shipped in Stage 2 and was called from NOWHERE.
/// This pins the enforcement, not the predicate.
#[tokio::test]
async fn a_colon_bearing_kb_id_is_refused_before_it_can_collide() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    // The pair that collides. Neither party is an attacker: both are ordinary
    // ids, and it is the ADDRESS that conflates them.
    let node = "c";
    let coll = {
        let mut c = mae_sync::kb::KbCollectionDoc::new_owned("a:b", "", "alice");
        c.add_node(node, node);
        c
    };
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"kb/share",
        "params":{
            "kb_id": "a:b",
            "name": "a:b",
            "creator": "alice",
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [{"id": node, "state": update_to_base64(
                &make_test_node(node, "Architecture", "ALICE-BODY", &[]))}],
        }
    });
    let r = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        msg,
        &mut docs,
    )
    .await;

    let err = r
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("a colon-bearing KB id must be refused, not shared"));
    assert!(
        err.message.contains("a:b") && err.message.contains(':'),
        "the refusal must name the offending id and say what is wrong with it, got: {}",
        err.message
    );

    // The document must not exist under ANY spelling: a refusal that still
    // materialized the doc would leave the collision in place.
    assert!(
        !store
            .has_doc(&mae_sync::kb_node_doc_name("a:b", node))
            .await,
        "refused share still created the node document"
    );
    assert!(
        !store.has_doc("kbc:a:b").await,
        "refused share still created the collection document"
    );
}

/// The non-vacuity control for the test above, and the property that makes D3
/// narrow rather than blanket: a colon in the NODE id is legitimate and common,
/// and must keep working. `a`/`b:c` is precisely the sibling that would have
/// collided with `a:b`/`c` — so this also proves the pair was a real collision
/// and not two ids that could never have met.
#[tokio::test]
async fn a_colon_in_the_node_id_stays_legal() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    share_kb_with_node_as(&store, &bc, "alice", "a", "b:c", "ALICE-BODY", &mut docs).await;

    assert!(
        store.has_doc(&mae_sync::kb_node_doc_name("a", "b:c")).await,
        "a node id containing ':' is ordinary (concept:architecture) and must share"
    );
}

/// An empty KB id addresses `kbn::{node_id}`, which `DocAddress::parse` rejects —
/// so it would be an unparseable name reaching the guards that fail CLOSED. Refuse
/// it at entry instead, where the caller gets an actionable error.
#[tokio::test]
async fn an_empty_kb_id_is_refused() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let coll = mae_sync::kb::KbCollectionDoc::new_owned("", "", "alice");
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"kb/share",
        "params":{
            "kb_id": "",
            "name": "",
            "creator": "alice",
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [],
        }
    });
    let r = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        msg,
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "an empty KB id must be refused at entry, not left to fail deeper"
    );
}
