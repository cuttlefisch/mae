//! `kb/query.my_kbs`: the caller's own KBs and nothing else.
//!
//! Attacker model: a principal holding a valid token tries to learn which
//! other KBs a daemon hosts, or who else is in the ones it can see. Every
//! test here names a KB or principal the response must NOT contain and scans
//! the serialized response for it, the same proxy for "the raw HTTP body"
//! `kb_query_tests` uses (`handle_request` serializes `dispatch`'s value
//! unchanged).

use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::kb_query;
use mae_daemon::storage::SqliteBackend;
use mae_mcp::identity::Identity;
use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
use mae_sync::kb::{JoinPolicy, KbCollectionDoc, Role, TransportPolicy};
use serde_json::{json, Value};

use super::kb_query_tests::{fresh_doc_store, generous_limits};

const TEST_BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");

const CALLER: &str = "SHA256:caller-fp-7c1e";
const OTHER: &str = "SHA256:other-member-fp-93ad";
const OWNER: &str = "SHA256:owner-fp-0b55";

/// An owned, un-anchored KB stored at `kbc:{kb_id}`, named `name`, with
/// `members` in its locally-authoritative `member_roles`.
async fn put_kb(
    doc_store: &DocStore,
    kb_id: &str,
    name: &str,
    members: &[(&str, Role)],
    policy: JoinPolicy,
    transport: TransportPolicy,
) {
    let mut coll = KbCollectionDoc::new_owned(name, OWNER, "owner");
    coll.set_transport_policy(transport);
    let _ = coll.set_join_policy(policy);
    for (principal, role) in members {
        let _ = coll.upsert_member(principal, "member", *role);
    }
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state(), TEST_BY)
        .await
        .unwrap();
}

async fn my_kbs(doc_store: &DocStore, principal: Option<&str>) -> Value {
    kb_query::dispatch(
        "kb/query.my_kbs",
        &json!({}),
        doc_store,
        principal,
        generous_limits(),
    )
    .await
    .unwrap_or_else(|e| panic!("my_kbs refused: {e:?}"))
}

/// `(kb_id, role)` pairs, in response order.
fn ids_and_roles(v: &Value) -> Vec<(String, Option<String>)> {
    v["kbs"]
        .as_array()
        .expect("kbs array")
        .iter()
        .map(|k| {
            (
                k["kb_id"].as_str().unwrap().to_string(),
                k["role"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// Five KBs, one per way of NOT being visible, plus two the caller may read.
async fn five_kbs(doc_store: &DocStore) {
    use JoinPolicy::*;
    use TransportPolicy::*;
    let both = [(CALLER, Role::Editor), (OTHER, Role::Viewer)];
    put_kb(
        doc_store,
        "kb-a-viewer",
        "Alpha",
        &[(CALLER, Role::Viewer)],
        Restrictive,
        Hub,
    )
    .await;
    put_kb(doc_store, "kb-b-editor", "Beta", &both, Invite, Hub).await;
    put_kb(
        doc_store,
        "kb-c-private",
        "Gamma",
        &[(OTHER, Role::Editor)],
        Restrictive,
        Hub,
    )
    .await;
    // Permissive: the caller could auto-join, which is still not membership.
    put_kb(
        doc_store,
        "kb-d-permissive",
        "Delta",
        &[(OTHER, Role::Viewer)],
        Permissive,
        Hub,
    )
    .await;
    // A member, but the KB is not exposed over the hub this listener serves.
    put_kb(
        doc_store,
        "kb-e-p2p-only",
        "Epsilon",
        &[(CALLER, Role::Viewer)],
        Restrictive,
        P2p,
    )
    .await;
}

#[tokio::test]
async fn a_member_sees_exactly_the_kbs_it_may_read_with_its_own_role() {
    let doc_store = fresh_doc_store().await;
    five_kbs(&doc_store).await;

    let got = my_kbs(&doc_store, Some(CALLER)).await;
    assert_eq!(
        ids_and_roles(&got),
        vec![
            ("kb-a-viewer".into(), Some("viewer".into())),
            ("kb-b-editor".into(), Some("editor".into())),
        ],
        "{got}"
    );
    assert_eq!(got["kbs"][0]["title"], "Alpha");
    assert_eq!(got["kbs"][0]["encryption"], "none");

    let raw = got.to_string();
    for absent in [
        OTHER,
        OWNER,
        "kb-c-private",
        "Gamma",
        "kb-d-permissive",
        "Delta",
        "kb-e-p2p-only",
        "Epsilon",
    ] {
        assert!(!raw.contains(absent), "'{absent}' leaked into {raw}");
    }
}

#[tokio::test]
async fn every_listed_kb_is_one_capabilities_admits() {
    // The list and the per-KB gate must agree exactly, for every caller: a KB
    // listed but refused is a broken link, and one admitted but unlisted is a
    // KB the reader cannot find. Checked over every KB and principal,
    // including the permissive KB a non-member could join but not read.
    let doc_store = fresh_doc_store().await;
    five_kbs(&doc_store).await;
    let all = [
        "kb-a-viewer",
        "kb-b-editor",
        "kb-c-private",
        "kb-d-permissive",
        "kb-e-p2p-only",
    ];
    for who in [CALLER, OTHER, OWNER, "SHA256:stranger"] {
        let listed: Vec<String> = ids_and_roles(&my_kbs(&doc_store, Some(who)).await)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        for kb in all {
            let caps = kb_query::dispatch(
                "kb/query.capabilities",
                &json!({"kb_id": kb}),
                &doc_store,
                Some(who),
                generous_limits(),
            )
            .await;
            let is_listed = listed.iter().any(|l| l == kb);
            assert_eq!(
                is_listed,
                caps.is_ok(),
                "{who}: {kb} listed={is_listed} caps={caps:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_owner_sees_its_kbs_whatever_their_transport_and_a_stranger_sees_none() {
    let doc_store = fresh_doc_store().await;
    five_kbs(&doc_store).await;

    let owner = ids_and_roles(&my_kbs(&doc_store, Some(OWNER)).await);
    assert_eq!(owner.len(), 5, "{owner:?}");
    assert!(
        owner.iter().all(|(_, r)| r.as_deref() == Some("owner")),
        "{owner:?}"
    );

    let stranger = my_kbs(&doc_store, Some("SHA256:stranger")).await;
    assert_eq!(stranger, json!({"kbs": []}));
}

#[tokio::test]
async fn an_anchored_kb_reports_the_role_the_signed_log_grants_not_the_unsigned_copy() {
    // A joined (anchored) KB is authorized from its signed op-log. Its
    // unsigned `member_roles` is relay-writable; here it claims the caller is
    // an owner while the owner signed only a viewer admit. The listing must
    // show what the gate enforces.
    let doc_store = fresh_doc_store().await;
    let owner = Identity::generate("owner");
    let member = Identity::generate("member");
    let member_fp = member.fingerprint();
    let (owner_secret, owner_pub) = (owner.secret_bytes(), owner.public().to_bytes());

    let mut coll = KbCollectionDoc::new_owned("Signed KB", &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    let key = ContentKey::generate();
    let self_wrap = wrap_to_member(&key, &wrap_public_for(&owner_secret)).unwrap();
    coll.author_e2e_genesis(
        "kb-signed",
        &owner.fingerprint(),
        &owner_secret,
        &owner_pub,
        self_wrap,
        1000,
    );
    let member_wrap_pub = wrap_public_for(&member.secret_bytes());
    coll.author_member_admit(
        "kb-signed",
        &member_fp,
        &member.public().to_bytes(),
        &member_wrap_pub,
        Role::Viewer,
        "member",
        wrap_to_member(&key, &member_wrap_pub).unwrap(),
        &owner.fingerprint(),
        &owner_secret,
        &owner_pub,
        1001,
    );
    let _ = coll.upsert_member(&member_fp, "forged", Role::Owner);
    doc_store
        .share_doc("kbc:kb-signed", &coll.encode_state(), TEST_BY)
        .await
        .unwrap();
    doc_store
        .set_kb_anchor("kb-signed", owner_pub)
        .await
        .unwrap();

    let got = my_kbs(&doc_store, Some(&member_fp)).await;
    assert_eq!(
        ids_and_roles(&got),
        vec![("kb-signed".into(), Some("viewer".into()))],
        "{got}"
    );
    assert_eq!(got["kbs"][0]["encryption"], "e2e", "{got}");
}

#[tokio::test]
async fn the_list_survives_a_restart() {
    // `kb/list` reads `kb_metas`, which lives in memory and is filled only by
    // `kb/share`/`kb/register` on this process. A reader must not lose its KB
    // list when the daemon restarts, so `my_kbs` reads the stored set.
    let backend: Arc<dyn mae_daemon::storage::StorageBackend> =
        Arc::new(SqliteBackend::open_memory().unwrap());
    {
        let before = DocStore::new(Arc::clone(&backend), 500);
        put_kb(
            &before,
            "kb-durable",
            "Durable",
            &[(CALLER, Role::Viewer)],
            JoinPolicy::Restrictive,
            TransportPolicy::Hub,
        )
        .await;
    }
    let after = DocStore::new(backend, 500);
    assert_eq!(
        ids_and_roles(&my_kbs(&after, Some(CALLER)).await),
        vec![("kb-durable".into(), Some("viewer".into()))]
    );
}

#[tokio::test]
async fn an_unnamed_kb_has_a_null_title() {
    let doc_store = fresh_doc_store().await;
    put_kb(
        &doc_store,
        "kb-unnamed",
        "",
        &[(CALLER, Role::Viewer)],
        JoinPolicy::Restrictive,
        TransportPolicy::Hub,
    )
    .await;
    let got = my_kbs(&doc_store, Some(CALLER)).await;
    assert_eq!(got["kbs"][0]["kb_id"], "kb-unnamed", "{got}");
    assert!(got["kbs"][0]["title"].is_null(), "{got}");
}
