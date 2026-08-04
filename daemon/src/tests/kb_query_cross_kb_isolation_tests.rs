//! ADVERSARIAL (#571): cross-KB isolation for `kb/query.get`.
//!
//! Split out of `kb_query_tests.rs`, which is scoped to ADR-053 Phase G's own
//! surface (encryption-aware branching, the Read gate, per-call caps). This
//! file pins a different property: the `DocStore` doc namespace is FLAT
//! (`kb:{node_id}`, no `kb_id` component), so authorizing a KB must not
//! authorize an arbitrary globally-addressed document.
//!
//! Reuses that file's seeding helpers rather than duplicating them.

use std::sync::Arc;

use mae_mcp::identity::Identity;
use mae_sync::kb::Role;
use serde_json::json;

use mae_daemon::kb_query;

use super::kb_query_tests::{fresh_doc_store, generous_limits, seed_e2e_kb, seed_unencrypted_kb};

/// ADVERSARIAL (#571): being authorized on KB-A must confer nothing on KB-B.
///
/// The attack is one substitution: `kb/query.get` gates on `kb_id` but fetches
/// `kb:{node_id}` from a FLAT, KB-unscoped doc namespace, so passing
/// `kb_id = <mine>, node_id = <someone else's>` reads a KB the caller was
/// never admitted to.
///
/// Table-driven over the FULL 2x2 encryption matrix, because the gated KB's
/// mode selects the response branch while the TARGET KB's mode decides what
/// actually leaks -- so three of the four cells fail differently and a test
/// covering only the obvious one would miss the worst:
///
///   A=plain, B=plain -> full title/body disclosure
///   A=plain, B=e2e   -> metadata only (no "node" map to decode)
///   A=e2e,   B=plain -> B's PLAINTEXT base64'd into `ciphertext_b64` and
///                       labelled `"encryption":"e2e"` (evades any plaintext
///                       string-scan of the response -- hence the decode below)
///   A=e2e,   B=e2e   -> real ciphertext under B's key, which A's member lacks
#[tokio::test]
async fn kb_query_get_cannot_read_a_node_belonging_to_another_kb() {
    for (case, (a_e2e, b_e2e)) in [(false, false), (false, true), (true, false), (true, true)]
        .into_iter()
        .enumerate()
    {
        // Per-case fixtures: distinct KB ids, node ids, identities and secret
        // markers, so nothing passes by colliding with another case's state.
        let kb_a = format!("kb-a-{case}");
        let kb_b = format!("kb-b-{case}");
        let node_a = format!("concept:a-own-{case}");
        let node_b = format!("concept:b-secret-{case}");
        let secret = format!("SECRET-MARKER-{case}-do-not-leak");
        let mallory = format!("oauth:mallory-{case}@example.com");
        let victim = format!("oauth:victim-{case}@example.com");

        let doc_store = fresh_doc_store().await;
        let owner_a = Arc::new(Identity::generate("owner-a"));
        let owner_b = Arc::new(Identity::generate("owner-b"));
        let mallory_id = Identity::generate("mallory");
        let victim_id = Identity::generate("victim");

        // KB-B first: the victim's KB, holding the secret. Mallory is admitted
        // NOWHERE in it.
        if b_e2e {
            seed_e2e_kb(
                &doc_store, &owner_b, &kb_b, &victim, &victim_id, &node_b, "b-title", &secret,
            )
            .await;
        } else {
            seed_unencrypted_kb(
                &doc_store,
                &owner_b,
                &kb_b,
                Some((&victim, Role::Viewer)),
                &node_b,
                "b-title",
                &secret,
                &[],
            )
            .await;
        }

        // KB-A: Mallory's own KB. Seeded second so the doc_store signer ends up
        // as A's owner -- the realistic posture for a caller operating on A.
        if a_e2e {
            seed_e2e_kb(
                &doc_store,
                &owner_a,
                &kb_a,
                &mallory,
                &mallory_id,
                &node_a,
                "a-title",
                "a-body",
            )
            .await;
        } else {
            seed_unencrypted_kb(
                &doc_store,
                &owner_a,
                &kb_a,
                Some((&mallory, Role::Viewer)),
                &node_a,
                "a-title",
                "a-body",
                &[],
            )
            .await;
        }

        // --- THE ATTACK: authorized on A, asking for B's node ---
        let attack = kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": kb_a, "node_id": node_b}),
            &doc_store,
            Some(&mallory),
            generous_limits(),
        )
        .await;

        assert!(
            attack.is_err(),
            "case {case} (a_e2e={a_e2e}, b_e2e={b_e2e}): a member of '{kb_a}' read \
             '{node_b}', which belongs to '{kb_b}'"
        );
        assert_eq!(
            attack.as_ref().unwrap_err().message,
            format!("node '{node_b}' is not in KB '{kb_a}'"),
            "case {case}: denial text must be exact and must not distinguish \
             'belongs to another KB' from 'does not exist'"
        );

        // Selective oracle: the secret must not appear ANYWHERE reachable --
        // neither in the serialized response, nor base64-decoded out of
        // `ciphertext_b64`. The decode is the A=e2e/B=plain cell: a plaintext
        // leak wrapped in base64 and mislabelled as ciphertext would sail past
        // a plain substring check.
        let serialized = format!("{attack:?}");
        assert!(
            !serialized.contains(&secret),
            "case {case}: secret leaked in the response payload"
        );
        if let Ok(v) = attack.as_ref() {
            if let Some(b64) = v.get("ciphertext_b64").and_then(|c| c.as_str()) {
                use base64::Engine as _;
                if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    assert!(
                        !raw
                            .windows(secret.len())
                            .any(|w| w == secret.as_bytes()),
                        "case {case}: secret leaked as base64 inside ciphertext_b64 \
                         while labelled encryption=e2e"
                    );
                }
            }
        }

        // The check must run BEFORE the doc store is touched: reading a node
        // also `get_or_create`s it, so a check placed after the fetch would
        // still let a caller materialize (and pre-squat) arbitrary node ids.
        // B's node genuinely exists, so probe with one that never did.
        let phantom = format!("concept:phantom-{case}");
        let _ = kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": kb_a, "node_id": phantom}),
            &doc_store,
            Some(&mallory),
            generous_limits(),
        )
        .await;
        assert!(
            !doc_store.has_doc(&format!("kb:{phantom}")).await,
            "case {case}: a refused fetch still materialized 'kb:{phantom}' -- the \
             scope check must precede encode_state_and_sv"
        );

        // POSITIVE CONTROL 1: the fix must not simply deny everything.
        let own = kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": kb_a, "node_id": node_a}),
            &doc_store,
            Some(&mallory),
            generous_limits(),
        )
        .await;
        assert!(
            own.is_ok(),
            "case {case}: Mallory can no longer read her OWN KB's node: {:?}",
            own.err()
        );

        // POSITIVE CONTROL 2: the victim is unharmed on their own KB.
        let legit = kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": kb_b, "node_id": node_b}),
            &doc_store,
            Some(&victim),
            generous_limits(),
        )
        .await;
        assert!(
            legit.is_ok(),
            "case {case}: the victim can no longer read their own node: {:?}",
            legit.err()
        );

        // SYMMETRY: the property must not be one-directional.
        let reverse = kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": kb_b, "node_id": node_a}),
            &doc_store,
            Some(&victim),
            generous_limits(),
        )
        .await;
        assert!(
            reverse.is_err(),
            "case {case}: the victim read Mallory's node -- the isolation is \
             one-directional, which means it is not isolation"
        );
    }
}
