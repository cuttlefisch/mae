//! Migration + cross-path agreement for the `nodes:fts` extractor repair.
//!
//! The extractor defect (see `NODES_FTS_DDL` in
//! `shared/kb/src/cozo_store/schema.rs`) welded a node's last title token onto
//! its first body token, so terms at that boundary retrieved nothing. Two
//! things have to be true for the fix to actually reach anyone:
//!
//!   1. a store whose index was built by the OLD definition must be repaired
//!      when it is next opened — a CozoDB FTS index is populated at
//!      `::fts create` and maintained incrementally, so changing the DDL alone
//!      only ever helps brand-new KBs; and
//!   2. `KbStore::fts_search` and `KnowledgeBase::search_ranked` — the two
//!      independent text-search paths behind the MCP tools, the Scheme
//!      primitives and the editor's own search — must agree about which nodes
//!      contain a term.
//!
//! Both are asserted here against the `DECISIONS_FOR_REVIEW.md` item-10
//! reproducer rather than a hand-picked term.

use super::*;
use crate::KnowledgeBase;

/// The reproducer node, plus a distractor so a "returns everything" regression
/// cannot pass by accident.
fn seed(store: &CozoKbStore) {
    store
        .insert_node(&Node::new(
            "n1",
            "Quantum Physics",
            NodeKind::Note,
            "Entanglement is spooky.",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n2",
            "Classical Mechanics",
            NodeKind::Note,
            "Newton was right.",
        ))
        .unwrap();
}

/// Every term of the reproducer node. `quantum` and `spooky` retrieved it even
/// while broken; `physics` and `entanglement` are the pair that did not.
const REPRODUCER_TERMS: &[&str] = &["quantum", "physics", "entanglement", "is", "spooky"];

/// An existing KB whose FTS index was built by the old, broken extractor is
/// repaired on the next open — no user action, no explicit reindex command.
///
/// The old index is reproduced exactly (same DDL text that shipped) rather than
/// simulated, and the store is genuinely closed and reopened, so this exercises
/// the real upgrade path an existing user takes.
#[test]
fn stale_fts_index_is_rebuilt_on_open() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("migrate_cozo");

    {
        let store = CozoKbStore::open(&path).unwrap();
        // Roll the index back to the broken definition and re-stamp the
        // version marker to a pre-fix value, i.e. exactly what is sitting on
        // an existing user's disk right now.
        store.run_mut("::fts drop nodes:fts").unwrap();
        store
            .run_mut(
                r#"::fts create nodes:fts {
                    extractor: title ++ ' ' ++ body,
                    tokenizer: Simple,
                    filters: [Lowercase]
                }"#,
            )
            .unwrap();
        store
            .run_mut(
                r#"?[key, val] <- [["fts_extractor_version", "1"]]
                   :put instance_meta {key => val}"#,
            )
            .unwrap();
        seed(&store);

        // Precondition: the old index really is broken, or this test proves
        // nothing about the migration.
        assert_eq!(
            store.fts_search("physics", 10).unwrap().len(),
            0,
            "expected the OLD extractor to miss 'physics'; if this fails the \
             bug being migrated away from no longer reproduces"
        );
        assert_eq!(store.fts_search("entanglement", 10).unwrap().len(), 0);
        // ...and the welded token is what the old index actually held.
        let words: Vec<String> = store
            .run_immut("?[word] := *nodes:fts{word}")
            .unwrap()
            .rows
            .iter()
            .filter_map(|r| r.first()?.get_str().map(str::to_string))
            .collect();
        assert!(
            words.iter().any(|w| w == "physicsentanglement"),
            "old index should hold the welded token, got {words:?}"
        );
    }

    // Reopen: `ensure_schema` -> `ensure_fts_index_current` must notice the
    // stale stamp and rebuild.
    let store = CozoKbStore::open(&path).unwrap();
    for term in REPRODUCER_TERMS {
        let hits = store.fts_search(term, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "n1"),
            "after migration, {term:?} must retrieve n1 (got {:?})",
            hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
        );
    }
    assert!(
        !store
            .run_immut("?[word] := *nodes:fts{word}")
            .unwrap()
            .rows
            .iter()
            .filter_map(|r| r.first()?.get_str())
            .any(|w| w == "physicsentanglement"),
        "the welded token must be gone after the rebuild"
    );

    // Idempotent: a second open must not rebuild again (the stamp now matches),
    // and must not regress what the first one repaired.
    drop(store);
    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(store.fts_search("physics", 10).unwrap().len(), 1);
    assert_eq!(store.fts_search("entanglement", 10).unwrap().len(), 1);
}

/// A store the migration CANNOT repair must still open.
///
/// `::fts create` binds against `nodes`, so a store whose `nodes` relation is on
/// disk at an older/short arity (the same real artifact
/// `load_all_tolerates_query_bind_failure` pins) fails the rebuild. That must
/// degrade to a warning, not an `Err` — propagating it would turn a store that
/// previously opened degraded into one that cannot be opened at all, which is
/// exactly the `kb_join`-abort + main-thread-stall failure that degradation was
/// introduced to prevent. The version stamp must also NOT advance, so the
/// repair is retried rather than recorded as done.
#[test]
fn failed_rebuild_degrades_instead_of_blocking_open() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("broken_cozo");

    {
        let store = CozoKbStore::open(&path).unwrap();
        // Replace `nodes` with a relation the current extractor cannot bind
        // (no `body` column). The index has to come off first — a relation
        // with indices attached cannot be replaced — and is then put back
        // over `title` alone, so an index still EXISTS on reopen. That is the
        // shape that matters: `ensure_schema`'s own `::fts create` short-
        // circuits on "already exists", so the only thing that touches this
        // broken relation is the migration's rebuild.
        store.run_mut("::fts drop nodes:fts").unwrap();
        store
            .run_mut(
                r#"?[id, title] <- [["bad", "x"]]
                   :replace nodes {id: String => title: String}"#,
            )
            .unwrap();
        store
            .run_mut(
                r#"::fts create nodes:fts {
                    extractor: title,
                    tokenizer: Simple,
                    filters: [Lowercase]
                }"#,
            )
            .unwrap();
        // Roll the stamp back so the migration attempts a rebuild on reopen.
        store
            .run_mut(
                r#"?[key, val] <- [["fts_extractor_version", "1"]]
                   :put instance_meta {key => val}"#,
            )
            .unwrap();
    }

    // Must open, not Err, and not panic.
    let store = CozoKbStore::open(&path).expect("a store the migration cannot repair must open");

    // The stamp must still be the pre-fix value — recording success here would
    // permanently skip the repair for this store.
    let stamped: Option<String> = store
        .run_immut(r#"?[val] := *instance_meta{key: "fts_extractor_version", val}"#)
        .unwrap()
        .rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.get_str())
        .map(str::to_string);
    assert_eq!(
        stamped.as_deref(),
        Some("1"),
        "a failed rebuild must not advance the version stamp"
    );
}

/// A store created fresh under the current DDL is correct without needing the
/// migration at all — the migration is a repair path, not a load-bearing part
/// of normal operation.
#[test]
fn fresh_store_needs_no_migration() {
    let tmp = tempfile::tempdir().unwrap();
    let store = CozoKbStore::open(tmp.path().join("fresh_cozo")).unwrap();
    seed(&store);
    for term in REPRODUCER_TERMS {
        assert!(
            store
                .fts_search(term, 10)
                .unwrap()
                .iter()
                .any(|h| h.id == "n1"),
            "{term:?} must retrieve n1 in a freshly created store"
        );
    }
}

/// The two independent search paths must agree on the reproducer.
///
/// `KnowledgeBase::search_ranked` scans an in-memory node collection and never
/// consults the FTS index, so it was never affected by the extractor defect —
/// which is precisely why an earlier pass rebuilt the Scheme `kb-search` on it
/// to avoid `fts_search`. That divergence (the MCP/store path missing nodes the
/// Scheme path returned) is what this test now pins closed.
#[test]
fn fts_search_and_search_ranked_agree_on_the_reproducer() {
    let tmp = tempfile::tempdir().unwrap();
    let store = CozoKbStore::open(tmp.path().join("agree_cozo")).unwrap();
    seed(&store);

    let mut kb = KnowledgeBase::new();
    for node in store.load_all().unwrap() {
        kb.insert(node);
    }

    let mut disagreements = Vec::new();
    for term in REPRODUCER_TERMS {
        let via_fts = store
            .fts_search(term, 50)
            .unwrap()
            .iter()
            .any(|h| h.id == "n1");
        let via_ranked = kb.search_ranked(term, 50).iter().any(|(id, _)| id == "n1");
        if via_fts != via_ranked {
            disagreements.push(format!(
                "{term:?}: fts_search={via_fts} search_ranked={via_ranked}"
            ));
        }
        assert!(
            via_ranked,
            "search_ranked must retrieve n1 for {term:?} (it scans nodes \
             directly and was never affected by the index defect)"
        );
    }
    assert!(
        disagreements.is_empty(),
        "the store path and the in-memory path disagree about which nodes \
         contain a term:\n  {}",
        disagreements.join("\n  ")
    );

    // Negative direction: neither path may invent a hit.
    for term in ["newton", "classical"] {
        assert!(
            !store
                .fts_search(term, 50)
                .unwrap()
                .iter()
                .any(|h| h.id == "n1"),
            "fts_search must not return n1 for {term:?}"
        );
        assert!(
            !kb.search_ranked(term, 50).iter().any(|(id, _)| id == "n1"),
            "search_ranked must not return n1 for {term:?}"
        );
    }
}
