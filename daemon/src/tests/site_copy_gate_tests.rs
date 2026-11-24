//! Who may have a whole KB's content at once (ADR-119 D7, Amendment 3 G2):
//! the live site's content index, which holds every page's text. A
//! query-only member (ADR-067) is refused it and nothing else: pages one at
//! a time, the explorer and tag listings (titles, as `kb/query.titles` gives
//! them) are theirs. A member with full replication gets everything; a
//! stranger gets nothing.
//!
//! Membership here is the real signed op-log of an anchored KB, not the
//! unsigned member list, because replication policy exists only there: the
//! owner's genesis, then one signed `Admit` per member, each freshly keyed.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mae_daemon::doc_store::DocStore;
use mae_daemon::site::{self, SitePage, View};
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};
use mae_sync::membership::{MembershipAction, ReplicationPolicy};

use super::kb_query_tests::fresh_doc_store;

const BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");
const LIMITS: site::SiteLimits = site::SiteLimits {
    max_scan_nodes: 500,
    max_body_bytes: 65_536,
};
const KB: &str = "kb-gate";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The KB's owner and what it needs to sign more ops.
struct Owner {
    fp: String,
    secret: [u8; 32],
    pubkey: [u8; 32],
}

/// Append a signed op giving `subject` `role` with `replication`, and store it.
async fn admit(
    store: &DocStore,
    owner: &Owner,
    action: MembershipAction,
    subject: &str,
    replication: ReplicationPolicy,
) {
    let name = format!("kbc:{KB}");
    let (state, _) = store.encode_state_and_sv(&name).await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    if action == MembershipAction::Admit {
        coll.upsert_member(subject, subject, Role::Viewer);
    }
    let mut op = coll.build_membership_op(
        KB,
        action,
        subject,
        Some(Role::Viewer),
        false,
        &owner.fp,
        now(),
        None,
        coll.epoch_of(subject),
    );
    op.replication = replication;
    let sig = op.sign(&owner.secret);
    let update = coll.append_signed_op(&op, &sig, &owner.pubkey);
    store.apply_update(&name, &update, None, BY).await.unwrap();
}

/// An anchored plaintext KB: `a` tagged `t`, `b` untagged; the owner's
/// signed genesis and nothing else yet.
async fn seed(store: &DocStore) -> Owner {
    let id = Arc::new(Identity::generate("owner"));
    let owner = Owner {
        fp: id.fingerprint(),
        secret: id.secret_bytes(),
        pubkey: id.public().to_bytes(),
    };
    store.set_signer(Arc::clone(&id));
    let mut coll = KbCollectionDoc::new_owned(KB, &owner.fp, "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    let genesis = coll.build_membership_op(
        KB,
        MembershipAction::Admit,
        &owner.fp,
        Some(Role::Owner),
        true,
        &owner.fp,
        now(),
        None,
        0,
    );
    let sig = genesis.sign(&owner.secret);
    coll.append_signed_op(&genesis, &sig, &owner.pubkey);
    let nodes = [("a", "#+filetags: :t:\nA.\n"), ("b", "B.\n")];
    for (n, _) in nodes {
        let _ = coll.add_node(n, n);
    }
    store
        .share_doc(&format!("kbc:{KB}"), &coll.encode_state(), BY)
        .await
        .unwrap();
    for (n, body) in nodes {
        let doc = KbNodeDoc::new(n, n, body, &[]);
        store
            .share_doc(&mae_sync::kb_node_doc_name(KB, n), &doc.encode_state(), BY)
            .await
            .unwrap();
    }
    store.set_kb_anchor(KB, owner.pubkey).await.unwrap();
    owner
}

async fn view(store: &DocStore, v: View, who: &str) -> SitePage {
    site::view(store, KB, &v, Some(who), LIMITS, &Default::default(), None).await
}

/// Every view any reader of the KB gets, query-only or not.
fn reader_views() -> [View; 6] {
    [
        View::Page("a".into()),
        View::Page("b".into()),
        View::Tags,
        View::Tag("t".into()),
        View::Explorer,
        View::Home(None),
    ]
}

/// Whether `who` gets the content index, checked as a copy: page b's text.
async fn gets_the_content(store: &DocStore, who: &str) -> bool {
    match view(store, View::Index, who).await {
        SitePage::Index(all) => {
            assert!(all.iter().any(|e| e.text.contains("B.")), "{all:?}");
            true
        }
        SitePage::NotFound => false,
        other => panic!("{who}: {other:?}"),
    }
}

#[tokio::test]
async fn a_query_only_member_reads_everything_but_the_content_index() {
    let store = fresh_doc_store().await;
    let owner = seed(&store).await;
    let full = Identity::generate("full").fingerprint();
    let query_only = Identity::generate("query-only").fingerprint();
    let stranger = Identity::generate("stranger").fingerprint();
    admit(
        &store,
        &owner,
        MembershipAction::Admit,
        &full,
        ReplicationPolicy::Full,
    )
    .await;
    admit(
        &store,
        &owner,
        MembershipAction::Admit,
        &query_only,
        ReplicationPolicy::QueryOnly,
    )
    .await;

    assert!(gets_the_content(&store, &owner.fp).await);
    assert!(gets_the_content(&store, &full).await);
    assert!(!gets_the_content(&store, &query_only).await);
    assert!(!gets_the_content(&store, &stranger).await);
    // The rest: the query-only member gets exactly what a full member does
    // (pages, their explorer and tag links, the listings), a stranger nothing.
    for v in reader_views() {
        let full_answer = view(&store, v.clone(), &full).await;
        assert_ne!(full_answer, SitePage::NotFound, "{v:?}");
        assert_eq!(
            view(&store, v.clone(), &query_only).await,
            full_answer,
            "{v:?}"
        );
        assert_eq!(
            view(&store, v.clone(), &stranger).await,
            SitePage::NotFound,
            "{v:?}"
        );
    }
}

/// A full member restricted to query-only loses the content index at once,
/// and regains it when the restriction is lifted: the gate reads the op-log
/// on every request, never a decision made earlier.
#[tokio::test]
async fn restricting_a_member_takes_the_content_away_and_lifting_it_gives_it_back() {
    let store = fresh_doc_store().await;
    let owner = seed(&store).await;
    let member = Identity::generate("member").fingerprint();
    admit(
        &store,
        &owner,
        MembershipAction::Admit,
        &member,
        ReplicationPolicy::Full,
    )
    .await;
    assert!(gets_the_content(&store, &member).await);
    admit(
        &store,
        &owner,
        MembershipAction::SetRole,
        &member,
        ReplicationPolicy::QueryOnly,
    )
    .await;
    assert!(!gets_the_content(&store, &member).await);
    assert!(matches!(
        view(&store, View::Explorer, &member).await,
        SitePage::Explorer(_)
    ));
    admit(
        &store,
        &owner,
        MembershipAction::SetRole,
        &member,
        ReplicationPolicy::Full,
    )
    .await;
    assert!(gets_the_content(&store, &member).await);
}
