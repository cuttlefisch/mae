//! D7 / E10 on the stored chain: it verifies end to end under the daemon's
//! key, and someone with write access to `state.db` cannot alter any column
//! of any row, remove a row, swap two rows or forge a signature without the
//! export failing to verify. Removing the LAST rows is the honest limit: it
//! shows only against a head kept elsewhere (E10, "What it proves"), which is
//! tested as such. Keys are generated per test.

use mae_mcp::identity::Identity;
use serde_json::json;

use super::*;
use crate::review::chain;

/// A KB whose chain holds five events of four kinds.
async fn chained() -> Env {
    let env = env().await;
    enable(&env).await;
    call(
        &env,
        &env.owner,
        "kb/review.set_code",
        json!({"kb_id": KB, "node_id": "n1", "doc_code": "PRO-1"}),
    )
    .await
    .unwrap();
    let r = request(&env, &env.editor, &["n1", "n2"]).await.unwrap();
    call(
        &env,
        &env.editor,
        "kb/review.withdraw",
        json!({"kb_id": KB, "request_id": r["request_id"], "reason": "r"}),
    )
    .await
    .unwrap();
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "required_acts": ["reviewed", "approved"]}),
    )
    .await
    .unwrap();
    env
}

fn text(lines: &[Value]) -> String {
    lines.iter().map(|l| l.to_string() + "\n").collect()
}

/// Whether the export, as `state.db` now stands, fails to verify under the
/// daemon's key (an export that cannot even be built counts as failing).
async fn tamper_detected(env: &Env, against: Option<&Value>) -> bool {
    match crate::review::export_lines(&env.store, KB).await {
        Err(_) => true,
        Ok(lines) => {
            chain::verify_export(&text(&lines), &env.daemon.public().to_bytes(), against).is_err()
        }
    }
}

fn as_attacker(env: &Env, sql: &str) {
    env.backend.execute_for_test(
        "DROP TRIGGER IF EXISTS kb_review_events_no_update;
         DROP TRIGGER IF EXISTS kb_review_events_no_delete;",
    );
    env.backend.execute_for_test(sql);
}

const COLUMNS: [(&str, &str); 10] = [
    ("kb_id", "'kb-elsewhere'"),
    ("seq", "99"),
    ("kind", "'approve'"),
    ("at", "'2000-01-01T00:00:00.000Z'"),
    ("actor", "'{\"kind\":\"person\",\"principal\":\"someone-else\"}'"),
    ("payload", "'{}'"),
    ("prev_hash", "'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'"),
    ("entry_hash", "'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'"),
    ("sig", "'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=='"),
    ("member_sig", "'forged'"),
];

#[tokio::test]
async fn the_stored_chain_verifies_and_every_altered_column_of_every_row_fails() {
    let env = chained().await;
    assert_eq!(verified_events(&env).await, 5);
    // Every row but the last: the last row's kb_id or seq altered is its
    // removal, which the next test covers against a kept head.
    for seq in 1..=5 {
        for (col, val) in COLUMNS {
            if seq == 5 && matches!(col, "kb_id" | "seq") {
                continue;
            }
            let env = chained().await;
            as_attacker(
                &env,
                &format!("UPDATE kb_review_events SET {col} = {val} WHERE seq = {seq}"),
            );
            assert!(
                tamper_detected(&env, None).await,
                "row {seq}, column {col} altered and it verified"
            );
        }
    }
}

#[tokio::test]
async fn a_removed_row_and_two_swapped_rows_fail_and_a_cut_tail_fails_against_a_kept_head() {
    let env = chained().await;
    as_attacker(&env, "DELETE FROM kb_review_events WHERE seq = 3");
    assert!(tamper_detected(&env, None).await, "a removed row");

    let env = chained().await;
    as_attacker(
        &env,
        "UPDATE kb_review_events SET seq = -1 WHERE seq = 2;
         UPDATE kb_review_events SET seq = 2 WHERE seq = 3;
         UPDATE kb_review_events SET seq = 3 WHERE seq = -1;",
    );
    assert!(tamper_detected(&env, None).await, "two swapped rows");

    let env = chained().await;
    let kept = crate::review::export_lines(&env.store, KB)
        .await
        .unwrap()
        .pop()
        .unwrap();
    as_attacker(&env, "DELETE FROM kb_review_events WHERE seq >= 4");
    assert!(
        !tamper_detected(&env, None).await,
        "a cut tail re-headed verifies alone: the honest limit"
    );
    assert!(
        tamper_detected(&env, Some(&kept)).await,
        "against the kept head it does not"
    );
}

/// A row re-signed by another key (the hash is unchanged, only the signature
/// is the forger's) fails under the daemon's key, and the whole chain fails
/// under the forger's.
#[tokio::test]
async fn a_forged_signature_fails() {
    let env = chained().await;
    let forger = Identity::generate("forger");
    let row = env
        .store
        .review_read(|c| Ok(crate::review::store::events(c, KB, 2, 1)?.remove(0)))
        .await
        .unwrap();
    let raw = hex::decode(&row.entry_hash).unwrap();
    let forged = forger.sign(&[b"maereview-sig/v1\n".as_slice(), &raw].concat());
    let b64 = mae_sync::encoding::update_to_base64(&forged);
    as_attacker(
        &env,
        &format!("UPDATE kb_review_events SET sig = '{b64}' WHERE seq = 3"),
    );
    assert!(tamper_detected(&env, None).await);
    let lines = crate::review::export_lines(&env.store, KB).await.unwrap();
    assert!(chain::verify_export(&text(&lines), &forger.public().to_bytes(), None).is_err());
}

/// The log is append-only for the daemon's own code paths: an UPDATE or a
/// DELETE through a transaction is refused by the store, and the chain is
/// untouched.
#[tokio::test]
async fn the_event_log_and_frozen_sources_refuse_update_and_delete() {
    let env = chained().await;
    for sql in [
        "UPDATE kb_review_events SET kind = 'x' WHERE seq = 1",
        "DELETE FROM kb_review_events WHERE seq = 1",
        "UPDATE kb_review_sources SET content = x'00'",
        "DELETE FROM kb_review_sources",
    ] {
        let r = env.store.review_write(move |c| Ok(c.execute(sql)?)).await;
        assert!(r.is_err(), "{sql} was allowed");
    }
    assert_eq!(verified_events(&env).await, 5);
}
