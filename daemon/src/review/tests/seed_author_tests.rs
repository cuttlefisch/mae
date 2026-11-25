//! ADR-113 A3 with `kb seed --author`: the seed's writes name no author, so
//! the operator's attestation, logged per write, stands in for one. The seed
//! is simulated with the calls it makes (a whole-document `restore_doc` on a
//! node's first write, `apply_update` after, then the log); the seed's own
//! tests pin that it makes them. Each test names the guard it kills.

use serde_json::json;

use super::act_tests::{approver, as_, policy, NOW};
use super::*;
use crate::doc_editors::{Attribution, InternalWriter};
use crate::review::store::{self, SeedWrite};

const SEED: Attribution<'static> = Attribution::Internal(InternalWriter::Seed);

/// How a simulated seed write is logged.
#[derive(Clone, Copy)]
enum Log<'a> {
    /// Logged, attested by this member.
    Attested(&'a str),
    /// Logged, with no attested author (`kb seed` without `--author`).
    Unattested,
    /// Not logged: the process stopped between the write and the log.
    Missed,
}

/// The seed writes `node`, replacing the whole document (`restore`, a
/// node's first write) or updating it, and logs it as `log` says.
async fn seed_write(env: &Env, node: &str, title: &str, restore: bool, log: Log<'_>) {
    let name = mae_sync::kb_node_doc_name(KB, node);
    let wal = if restore {
        let doc = KbNodeDoc::new(node, title, &format!("Seeded body of {node}."), &[]);
        let r = env
            .store
            .restore_doc(&name, &doc.encode_state(), SEED)
            .await;
        r.unwrap().wal_seq
    } else {
        let state = env.store.encode_state(&name).await.unwrap();
        let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 779).unwrap();
        let up = doc.set_title(title);
        let r = env.store.apply_update(&name, &up, None, SEED).await;
        r.unwrap().wal_seq
    };
    let author = match log {
        Log::Attested(a) => Some(a.to_string()),
        Log::Unattested => None,
        Log::Missed => return,
    };
    let node = node.to_string();
    env.store
        .review_write(move |c| {
            store::log_seed_write(
                c,
                &SeedWrite {
                    kb_id: KB,
                    node_id: &node,
                    wal_id: wal,
                    author: author.as_deref(),
                    operator: "tester",
                    rev: "0123abcd",
                    at: 0,
                },
            )
        })
        .await
        .unwrap();
}

/// The editor requests `node` at its current version; the revision id and
/// the request id. The editor is the requester, so an author (A1).
async fn request_one(env: &Env, node: &str) -> (String, String) {
    let r = request(env, &env.editor, &[node]).await.unwrap();
    (
        r["revisions"][0]["revision_id"]
            .as_str()
            .unwrap()
            .to_string(),
        r["request_id"].as_str().unwrap().to_string(),
    )
}

async fn approve(env: &Env, revision: &str) -> Result<Value, McpError> {
    let p = json!({"kb_id": KB, "revision_id": revision});
    as_(env, &approver(env), "kb/review.approve", p).await
}

/// A seed's first write resets the editor record's origin, so the record
/// alone cannot name an author; the attested restore at the origin does.
/// Kills: the origin's attestation ignored (n1 unknown), its author not
/// counted (n2 would pass), an unattested restore read as attested (n3).
#[tokio::test]
async fn an_attested_seed_restore_names_its_author() {
    let env = env().await;
    policy(&env, json!({})).await;
    let owner = env.owner.clone();
    seed_write(&env, "n1", "Seeded", true, Log::Attested(&owner)).await;
    let (rev, _) = request_one(&env, "n1").await;
    let a = approve(&env, &rev).await.unwrap();
    assert_eq!(a["revision_state"], json!("approved"));

    let viewer = env.viewer.clone();
    seed_write(&env, "n2", "Seeded", true, Log::Attested(&viewer)).await;
    let (rev, _) = request_one(&env, "n2").await;
    assert_eq!(err_kind(approve(&env, &rev).await), "author_cannot_approve");

    seed_write(&env, "n3", "Seeded", true, Log::Unattested).await;
    let (rev, _) = request_one(&env, "n3").await;
    assert_eq!(err_kind(approve(&env, &rev).await), "authors_unknown");
}

/// Every seed write must be in the log and attested. Kills: the log's count
/// not checked against the editor record (n1: an update the log missed), an
/// older attested write standing in for an unlogged restore at the origin
/// (n2), an unattested logged update ignored (n3).
#[tokio::test]
async fn a_seed_write_the_log_missed_or_did_not_attest_leaves_the_authors_unknown() {
    let env = env().await;
    policy(&env, json!({})).await;
    let owner = env.owner.clone();
    for n in NODES {
        seed_write(&env, n, "Seeded", true, Log::Attested(&owner)).await;
    }
    seed_write(&env, "n1", "Updated", false, Log::Missed).await;
    seed_write(&env, "n2", "Reseeded", true, Log::Missed).await;
    seed_write(&env, "n3", "Updated", false, Log::Unattested).await;
    for n in NODES {
        let (rev, _) = request_one(&env, n).await;
        assert_eq!(
            err_kind(approve(&env, &rev).await),
            "authors_unknown",
            "{n}"
        );
    }
}

/// `--update` adds authors after the last approval and never restarts the
/// window: an unattested write approved past (by acknowledging it) no longer
/// counts, and an attested update after it names its author. Kills: the seed
/// log read without the window (the old unattested write would refuse), and
/// an attested update's author not counted.
#[tokio::test]
async fn after_an_approval_only_later_seed_writes_count() {
    let env = env().await;
    policy(&env, json!({})).await;
    seed_write(&env, "n1", "Seeded", true, Log::Unattested).await;
    let (rev, rq) = request_one(&env, "n1").await;
    let p = json!({"kb_id": KB, "revision_id": rev, "acknowledge_unknown_authors": true,
        "override_reason": "Migrated from the paper register."});
    as_(&env, &approver(&env), "kb/review.approve", p)
        .await
        .unwrap();
    let f = json!({"kb_id": KB, "request_id": rq, "effective_at": NOW,
        "acknowledge_unknown_authors": true, "override_reason": "As approved."});
    as_(&env, &approver(&env), "kb/review.enter_into_force", f)
        .await
        .unwrap();

    let owner = env.owner.clone();
    seed_write(&env, "n1", "Second edition", false, Log::Attested(&owner)).await;
    let (rev, rq) = request_one(&env, "n1").await;
    assert_eq!(
        approve(&env, &rev).await.unwrap()["revision_state"],
        json!("approved")
    );
    let f = json!({"kb_id": KB, "request_id": rq, "effective_at": NOW});
    as_(&env, &approver(&env), "kb/review.enter_into_force", f)
        .await
        .unwrap();

    let viewer = env.viewer.clone();
    seed_write(&env, "n1", "Third edition", false, Log::Attested(&viewer)).await;
    let (rev, _) = request_one(&env, "n1").await;
    assert_eq!(err_kind(approve(&env, &rev).await), "author_cannot_approve");
}
