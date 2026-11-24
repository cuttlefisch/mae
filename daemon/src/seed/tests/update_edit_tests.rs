//! `kb seed --update` and edits outside the seed: the discard is bound to the
//! edits the dry run showed, a dry run during an update in progress judges
//! each node against that update, a returning node is judged against its
//! removal, and the plan must read the recorded repository.

use super::apply_tests::{fixture_files, fixture_repo, seeded, target, KB, T};
use super::git_tests::git_in;
use super::plan_opts;
use super::update_tests::{
    dry_run, edit_body, fresh, high_waters, node, release, remove, snapshot, update_to,
    update_with, v2, ALPHA, B,
};
use crate::seed::update::crash::{Step, AT};
use crate::seed::*;

const UNTOUCHED: &str = "0a000000-0000-4000-8000-000000000005";

/// The bodies kept in `id`'s history.
async fn history_bodies(store: &crate::doc_store::DocStore, id: &str) -> Vec<String> {
    store
        .history_list(KB, id)
        .await
        .unwrap()
        .iter()
        .filter_map(|v| v.content.as_ref())
        .map(|c| mae_sync::kb::history::decode_content(c).unwrap().body)
        .collect()
}

/// An edit made after the dry run is not discarded with that dry run's
/// token, whether it touches another node or the same node again; a fresh
/// dry run's token applies, and every edit stays in history. Kills: the
/// token not compared, or a bare `--discard-edits` taken for consent.
#[tokio::test]
async fn a_discard_is_bound_to_the_edits_the_dry_run_showed() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    edit_body(&store, ALPHA, "First edit.\n", "SHA256:editor").await;
    let rev = v2(&r);
    let first = dry_run(&store, &owner, &r, &rev).await.unwrap();
    let token = first.edits_hash.clone().expect("the edit needs a token");
    assert!(!update_would_apply(&first, None));
    assert!(update_would_apply(&first, Some(&token)));

    edit_body(&store, UNTOUCHED, "After the dry run.\n", "SHA256:late").await;
    let before = high_waters(&store).await;
    let stale = update_with(&store, &owner, &r, &rev, Some(&token)).await;
    assert_eq!(stale.unwrap_err().kind, ApplyRefusalKind::EditsChanged);
    let none = update_with(&store, &owner, &r, &rev, None).await;
    assert_eq!(none.unwrap_err().kind, ApplyRefusalKind::EditedOutsideSeed);
    assert_eq!(high_waters(&store).await, before);

    let second = dry_run(&store, &owner, &r, &rev).await.unwrap();
    let token = second.edits_hash.clone().unwrap();
    edit_body(&store, ALPHA, "Second edit.\n", "SHA256:editor").await;
    let before = high_waters(&store).await;
    let stale = update_with(&store, &owner, &r, &rev, Some(&token)).await;
    assert_eq!(stale.unwrap_err().kind, ApplyRefusalKind::EditsChanged);
    assert_eq!(high_waters(&store).await, before);
    assert!(store.seed_update(KB).await.unwrap().is_none());

    update_to(&store, &owner, &r, &rev, true).await.unwrap();
    assert_eq!(
        snapshot(&store).await,
        snapshot(&*fresh(&r, &rev).await).await
    );
    assert!(history_bodies(&store, ALPHA)
        .await
        .contains(&"Second edit.\n".into()));
    let kept = history_bodies(&store, UNTOUCHED).await;
    assert!(kept.contains(&"After the dry run.\n".into()), "{kept:?}");
}

/// The same on resume: a node written after the crash, then another after
/// the dry run of the resume, is refused with that dry run's token and
/// without one; the next dry run's token resumes it. Kills: a resume that
/// takes any token.
#[tokio::test]
async fn a_resumed_discard_is_bound_to_the_edits_since_the_crash() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let rev = v2(&r);
    AT.with(|c| c.set(Some(Step::BeforeNode(0))));
    let _ = update_to(&store, &owner, &r, &rev, false).await;
    AT.with(|c| c.set(None));
    assert!(store.seed_update(KB).await.unwrap().is_some());

    edit_body(&store, ALPHA, "Since the crash.\n", "SHA256:editor").await;
    let d = dry_run(&store, &owner, &r, &rev).await.unwrap();
    let ids: Vec<&str> = d.edited.iter().map(|e| e.node_id.as_str()).collect();
    assert_eq!(ids, [ALPHA]);
    let token = d.edits_hash.unwrap();
    edit_body(&store, UNTOUCHED, "Since the dry run.\n", "SHA256:late").await;
    let before = high_waters(&store).await;
    for (t, kind) in [
        (Some(token.as_str()), ApplyRefusalKind::EditsChanged),
        (None, ApplyRefusalKind::EditedOutsideSeed),
    ] {
        let err = update_with(&store, &owner, &r, &rev, t).await.unwrap_err();
        assert_eq!(err.kind, kind, "{err}");
    }
    assert_eq!(high_waters(&store).await, before);

    update_to(&store, &owner, &r, &rev, true).await.unwrap();
    assert_eq!(
        snapshot(&store).await,
        snapshot(&*fresh(&r, &rev).await).await
    );
}

/// A dry run while an update is cut short judges each node against that
/// update: a node it already wrote is not an edit, nor is one it has yet to
/// write. A node written after the update wrote it is listed, kept by the
/// resume (no token needed), and found by the next update. Kills: judging
/// against the rows of the last applied plan.
#[tokio::test]
async fn a_dry_run_mid_update_lists_no_false_edits() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let rev = v2(&r);
    let at = r
        .plan(&rev)
        .nodes
        .iter()
        .position(|n| n.file.id == B)
        .unwrap();
    AT.with(|c| c.set(Some(Step::BeforeNode(at))));
    let _ = update_to(&store, &owner, &r, &rev, false).await;
    AT.with(|c| c.set(None));
    let (_, nodes) = store.seed_update(KB).await.unwrap().unwrap();
    let alpha = nodes.iter().find(|n| n.row.node_id == ALPHA).unwrap();
    assert!(
        alpha.done,
        "ALPHA, whose body the update changes, is written"
    );
    assert!(node(&store, ALPHA).await.body.contains("revised"));

    let d = dry_run(&store, &owner, &r, &rev).await.unwrap();
    assert!(d.in_progress.is_some());
    assert!(
        d.edited.is_empty(),
        "no false edits mid-update: {:?}",
        d.edited
    );
    assert_eq!(d.edits_hash, None);
    assert!(update_would_apply(&d, None));

    edit_body(&store, ALPHA, "After the update wrote it.\n", "SHA256:e").await;
    let d = dry_run(&store, &owner, &r, &rev).await.unwrap();
    let listed: Vec<_> = d
        .edited
        .iter()
        .map(|e| (e.node_id.as_str(), e.discarded))
        .collect();
    assert_eq!(listed, [(ALPHA, false)]);
    assert_eq!(
        d.edits_hash, None,
        "the resume keeps it: nothing to discard"
    );
    update_to(&store, &owner, &r, &rev, false).await.unwrap();
    assert_eq!(
        node(&store, ALPHA).await.body,
        "After the update wrote it.\n"
    );

    let v3 = release(&r, 2, |r| {
        r.write("notes/more.org", &super::git_tests::note("more", "m\n"))
    });
    let next = dry_run(&store, &owner, &r, &v3).await.unwrap();
    let ids: Vec<&str> = next.edited.iter().map(|e| e.node_id.as_str()).collect();
    assert_eq!(ids, [ALPHA], "the next update finds the kept edit");
}

/// A node removed by one update, edited while removed, then returned by the
/// next: the dry run lists it, the update refuses without the token, applies
/// with it, and keeps the edit in history. An unedited return needs no
/// token, and once returned it has no removal record. Kills: skipping the
/// returning-node check.
#[tokio::test]
async fn a_returning_node_edited_while_removed_is_an_outside_edit() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let b_text = fixture_files()[1].1.clone();
    let gone = release(&r, 1, |r| remove(r, "notes/no-title.org"));
    update_to(&store, &owner, &r, &gone, false).await.unwrap();
    assert_eq!(store.seed_removed(KB).await.unwrap().len(), 1);
    edit_body(&store, B, "Edited while removed.\n", "SHA256:editor").await;
    let back = release(&r, 2, |r| {
        r.write("notes/b.org", &b_text.replace("Plain body.", "Back."))
    });

    let d = dry_run(&store, &owner, &r, &back).await.unwrap();
    let listed: Vec<_> = d
        .edited
        .iter()
        .map(|e| (e.node_id.as_str(), &e.editors))
        .collect();
    assert_eq!(listed, [(B, &vec!["principal:SHA256:editor".to_string()])]);
    assert!(d.added.iter().any(|c| c.node_id == B && c.returns));
    let before = high_waters(&store).await;
    let err = update_to(&store, &owner, &r, &back, false)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::EditedOutsideSeed, "{err}");
    assert!(err.detail.contains(B), "{err}");
    assert_eq!(high_waters(&store).await, before);

    update_to(&store, &owner, &r, &back, true).await.unwrap();
    assert_eq!(
        snapshot(&store).await,
        snapshot(&*fresh(&r, &back).await).await
    );
    let kept = history_bodies(&store, B).await;
    assert!(kept.contains(&"Edited while removed.\n".into()), "{kept:?}");
    assert!(store.seed_removed(KB).await.unwrap().is_empty());

    let again = release(&r, 3, |r| remove(r, "notes/b.org"));
    update_to(&store, &owner, &r, &again, false).await.unwrap();
    let unedited = release(&r, 4, |r| r.write("notes/b.org", &b_text));
    update_to(&store, &owner, &r, &unedited, false)
        .await
        .unwrap();
}

/// A plan read from another repository is refused by the dry run and the
/// update, though its content (and so its plan hash) is the same: a clone.
/// Kills: no repository check.
#[tokio::test]
async fn a_plan_is_judged_by_its_history_not_its_path() {
    // A plan is judged by its history, not its repository's path: an
    // unrelated repository, and a history that holds the applied commit
    // without descending from it, are refused and write nothing; a clone of
    // the same history (a repository moved, or given by another path)
    // updates the KB.
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let rev = v2(&r);
    let fp = owner.fingerprint();
    let before = high_waters(&store).await;
    let refused = |plan: &SeedPlan| {
        let (store, plan, owner) = (store.clone(), plan.clone(), owner.clone());
        async move {
            let hash = plan.plan_hash().unwrap();
            let err = update_diff(&store, &plan, KB, &owner.fingerprint())
                .await
                .unwrap_err();
            assert_eq!(err.kind, ApplyRefusalKind::DifferentRepo, "{err}");
            let err = update(&store, &plan, &target(&owner, &hash), None)
                .await
                .unwrap_err();
            assert_eq!(err.kind, ApplyRefusalKind::DifferentRepo, "{err}");
            assert!(err.detail.contains("new kb-id"), "{err}");
        }
    };

    // Unrelated: a repository whose very first commit differs. (The fixture
    // commits deterministically, so the same files at the same time ARE the
    // same history, and are accepted, rightly.)
    let mut files = fixture_files();
    files[0].1.push_str("\nunrelated\n");
    let other = fixture_repo(&files);
    refused(&other.plan("HEAD")).await;

    // Orphan: a clone holds the applied commit, but this branch does not
    // descend from it.
    let clone = tempfile::tempdir().unwrap();
    let src = r.path().to_str().unwrap();
    git_in(clone.path(), &["clone", "-q", src, "."], T);
    git_in(clone.path(), &["checkout", "-q", "--orphan", "orphan"], T);
    git_in(clone.path(), &["commit", "-q", "-m", "orphan"], T + 2000);
    refused(&plan(&plan_opts(clone.path(), "orphan")).unwrap()).await;
    assert_eq!(high_waters(&store).await, before);

    // The same history at another path: accepted.
    git_in(clone.path(), &["checkout", "-q", &rev], T);
    let moved = plan(&plan_opts(clone.path(), &rev)).unwrap();
    let hash = moved.plan_hash().unwrap();
    assert_eq!(Some(&hash), r.plan(&rev).plan_hash().as_ref());
    assert!(update_diff(&store, &moved, KB, &fp).await.is_ok());
    update(&store, &moved, &target(&owner, &hash), None)
        .await
        .expect("a moved repository of the same history updates the KB");
}
