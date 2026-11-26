//! ADR-113 E7 on seed removal: `kb seed --update` to a revision that drops a
//! page whose node holds a publication-review revision is refused before it
//! records its intent or writes anything, naming the node.

use super::apply_tests::{fixture_files, fixture_repo, seeded, target};
use super::update_tests::{high_waters, v2};
use crate::review::store::{insert_revision, Revision};
use crate::seed::*;

/// The fixture page v2 removes (`notes/empty-lines.org`).
const DROPPED: &str = "0a000000-0000-4000-8000-000000000003";

fn revision(node: &str) -> Revision {
    Revision {
        revision_id: "rv-1".into(),
        kb_id: super::apply_tests::KB.into(),
        node_id: node.into(),
        request_id: "rq-1".into(),
        doc_code: None,
        rev_no: None,
        source_sha256: "00".repeat(32),
        version: "00".repeat(16),
        cutoff_wal: 0,
        history_seq: 1,
        state: "in_force".into(),
        doc_type: None,
        template_id: None,
        template_sha256: None,
        approval_record: "act".into(),
        replaces: None,
        created_by: "SHA256:example".into(),
        created_by_kind: "person".into(),
        created_at: 0,
        approved_at: None,
        in_force_since: None,
        ended_at: None,
    }
}

/// Refused, naming the node, with nothing written and no update begun; the
/// same update with the revision on a node it keeps goes through. Kills: the
/// pre-check missing (the store's guard would refuse only at the manifest,
/// after the pages were written), and a pre-check that refuses every removal.
#[tokio::test]
async fn an_update_dropping_a_node_with_a_revision_is_refused_before_any_write() {
    for (node, refused) in [(DROPPED, true), (super::update_tests::ALPHA, false)] {
        let r = fixture_repo(&fixture_files());
        let (store, owner, _) = seeded(&r).await;
        let rev = revision(node);
        store
            .review_write(move |c| insert_revision(c, &rev))
            .await
            .unwrap();
        let tag = v2(&r);
        let plan = r.plan(&tag);
        let hash = plan.plan_hash().unwrap();
        let before = high_waters(&store).await;
        let out = update(&store, &plan, &target(&owner, &hash), None).await;
        if !refused {
            assert!(out.is_ok(), "{out:?}");
            continue;
        }
        let e = out.unwrap_err();
        assert_eq!(e.kind, ApplyRefusalKind::HasRevisions);
        assert!(e.detail.contains(DROPPED), "{}", e.detail);
        assert_eq!(high_waters(&store).await, before, "nothing written");
        assert!(store
            .seed_update(super::apply_tests::KB)
            .await
            .unwrap()
            .is_none());
    }
}
