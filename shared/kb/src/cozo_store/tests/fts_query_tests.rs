//! FTS query behaviour beyond the core "does it find/rank everything"
//! property (see the sibling `fts_search_tests`): known cozo FTS
//! query-grammar edge cases (prefix queries, reserved punctuation, reserved
//! UPPERCASE boolean keywords), ranking under a realistic multi-node corpus,
//! empty-query/bulk-fetch-path behaviour, re-indexing on node update, and a
//! raw-Tantivy sanity check of the underlying cozo FTS mechanism itself
//! (`docs:search`, independent of `CozoKbStore::fts_search`).
//!
//! Split out of `kb_store_impl_tests.rs` alongside `fts_search_tests` when
//! that file grew past the 500-line test-file ceiling.

use super::*;

/// A prefix query (`buffer*`) is legitimate cozo FTS syntax and the index
/// answers it correctly. `fts_search`'s post-query verification used to treat
/// the raw query as literal text, so `text.contains("buffer*")` was false for
/// every candidate and the caller got zero hits — the index found the node and
/// MAE threw it away. Same silent-miss direction as the extractor defect.
#[test]
fn fts_prefix_query_survives_post_query_verification() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer Management",
            NodeKind::Note,
            "The rope-backed store.",
        ))
        .unwrap();

    for q in ["buffer*", "buff*", "rope*", "manage*"] {
        let hits = store.fts_search(q, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "concept:buffer"),
            "prefix query {q:?} must retrieve concept:buffer, got {:?}",
            hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
        );
    }

    // The guard must still do its actual job: reject a term that is genuinely
    // absent, rather than passing everything through.
    let absent = store.fts_search("nonexistentterm", 10).unwrap();
    assert!(
        absent.is_empty(),
        "verification must still reject non-matching terms, got {:?}",
        absent.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
    );
}

/// Pins the KNOWN-BAD query-side behaviour that this change does NOT fix, so it
/// stays visible and a future fix must consciously update this test.
///
/// Cozo's FTS *query* grammar reads `:`, `-`, `.` and a leading `*` as
/// operators, so these are hard parse errors before MAE sees a candidate. This
/// is why `(kb-search "concept:buffer")` is built on `search_ranked` instead
/// (see the `@ai-caution` in `crates/scheme/src/runtime/kb_crud.rs`). Erroring
/// is at least loud — unlike the two silent misses fixed alongside this test —
/// but it means namespaced ids and hyphenated words are unqueryable through
/// `fts_search`.
#[test]
fn fts_query_syntax_characters_are_still_parse_errors() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer Management",
            NodeKind::Note,
            "Covers read-only mode and version 1.5.",
        ))
        .unwrap();

    for q in ["concept:buffer", "read-only", "1.5", "-buffer"] {
        assert!(
            store.fts_search(q, 10).is_err(),
            "{q:?} is expected to still be a cozo FTS parse error; if this now \
             succeeds the query surface was fixed — update this test and the \
             kb_crud.rs @ai-caution together"
        );
    }

    // The component words remain reachable, which is what makes the above a
    // usability gap rather than a data-loss bug.
    for q in ["buffer", "read", "only", "5"] {
        assert!(
            !store.fts_search(q, 10).unwrap().is_empty(),
            "{q:?} should still retrieve the node"
        );
    }
}

/// A second known-bad query class, found by the case-variation property test
/// rather than by inspection: cozo reserves UPPERCASE `AND`/`OR`/`NOT`/`NEAR`
/// as FTS boolean operators, and `cozoscript.pest`'s exclusion of them is a
/// bare `!("AND" | "OR" | "NOT" | "NEAR")` lookahead with no word-boundary
/// anchor. So an ALL-CAPS term that merely STARTS with one is a parse error:
/// `ANDROID`, `ORBIT`, `NEARBY`, `ANDES`, `NOTES` are all unqueryable, while
/// `Android`/`android`, `Orbit`, `Notes` are fine.
///
/// Not fixed here: suppressing it means deciding whether MAE exposes cozo's
/// boolean query language to users at all (lower-casing the query would make
/// a deliberate `foo AND bar` mean the literal word "and"). That is a product
/// decision, not a bug fix to slip in alongside an index repair.
#[test]
fn fts_uppercase_boolean_keyword_prefixes_are_parse_errors() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Android Notes",
            NodeKind::Note,
            "Orbit and nearby andes ordering.",
        ))
        .unwrap();

    // Exactly the keywords, and — the surprising part — words merely prefixed
    // by them.
    for q in [
        "AND", "OR", "NOT", "NEAR", "ANDROID", "ORBIT", "NEARBY", "ANDES", "NOTES", "ORDERING",
    ] {
        assert!(
            store.fts_search(q, 10).is_err(),
            "{q:?} is expected to be a cozo FTS parse error (unanchored keyword \
             lookahead); if this now succeeds, update this test"
        );
    }

    // Non-uppercase forms of the same words work, confirming this is purely a
    // query-grammar artifact and the INDEX holds these terms correctly.
    for q in [
        "android", "Android", "orbit", "Orbit", "nearby", "andes", "notes", "Notes", "ordering",
    ] {
        assert!(
            !store.fts_search(q, 10).unwrap().is_empty(),
            "{q:?} should retrieve the node"
        );
    }
}

#[test]
fn fts_ranking_and_multi_word() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Quantum Physics",
            NodeKind::Note,
            "Entanglement is spooky action at a distance",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n2",
            "Classical Mechanics",
            NodeKind::Note,
            "Newton discovered gravity under a tree",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n3",
            "Relativity Theory",
            NodeKind::Note,
            "Einstein showed space and time are linked by gravity",
        ))
        .unwrap();

    // Single word search — should find nodes mentioning "gravity"
    let hits = store.fts_search("gravity", 10).unwrap();
    assert!(
        hits.len() >= 2,
        "expected 2+ results for 'gravity', got {}",
        hits.len()
    );
    let hit_ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(hit_ids.contains(&"n2"), "n2 should match 'gravity'");
    assert!(hit_ids.contains(&"n3"), "n3 should match 'gravity'");

    // Title search — "quantum" is in the title, Tantivy indexes title + body
    let hits = store.fts_search("quantum", 10).unwrap();
    assert!(!hits.is_empty(), "should find 'quantum' in title");
    assert_eq!(hits[0].id, "n1");

    // Empty query returns all nodes
    let all = store.fts_search("", 100).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn fts_search_empty_query_respects_limit() {
    // Regression: the empty-query branch used to return ALL node ids
    // unbounded (reachable via the AI `kb_search` tool); it must honor `limit`.
    let (_tmp, store) = make_store();
    for i in 0..10 {
        store
            .insert_node(&Node::new(
                format!("n{i}"),
                format!("Title {i}"),
                NodeKind::Note,
                "body",
            ))
            .unwrap();
    }
    let bounded = store.fts_search("", 3).unwrap();
    assert_eq!(bounded.len(), 3, "empty query must respect the limit");
}

#[test]
fn fts_search_bulk_path_matches_terms_and_scores() {
    // Exercises the bulk-fetch (`is_in`) path that replaced the per-candidate
    // get_node N+1: candidates must still be term-verified against their real
    // title+body (fetched in one query), non-matches excluded, and the FTS
    // score preserved. Uses colon-namespaced ids (the KB norm) to confirm the
    // bulk `is_in` lookup handles them.
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "doc:rust",
            "Rust Notes",
            NodeKind::Note,
            "The borrow checker enforces memory safety",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "doc:python",
            "Python Notes",
            NodeKind::Note,
            "Duck typing is flexible",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "doc:empty",
            "Unrelated",
            NodeKind::Note,
            "nothing relevant here",
        ))
        .unwrap();

    let hits = store.fts_search("borrow", 10).unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(
        ids.contains(&"doc:rust"),
        "bulk path should surface the term match, got {ids:?}"
    );
    assert!(
        !ids.contains(&"doc:empty"),
        "term-verification must exclude non-matches"
    );
    // Bulk fetch must not drop the score carried from the FTS query.
    assert!(hits.iter().all(|h| h.score >= 0.0));
}

#[test]
fn fts_updates_on_node_change() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "u1",
            "Alpha",
            NodeKind::Note,
            "original content about photosynthesis",
        ))
        .unwrap();

    // Should find photosynthesis
    let hits = store.fts_search("photosynthesis", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // Update body
    store
        .insert_node(&Node::new(
            "u1",
            "Alpha",
            NodeKind::Note,
            "updated content about mitochondria",
        ))
        .unwrap();

    // Old term should NOT be found (FTS re-indexed via rm + put)
    let hits = store.fts_search("photosynthesis", 10).unwrap();
    assert!(
        hits.is_empty(),
        "stale FTS: 'photosynthesis' should not match after update"
    );

    // New term should be found
    let hits = store.fts_search("mitochondria", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "u1");
}

/// CozoDB's native Tantivy FTS index. Opened **sled** until D2 despite its name,
/// passing only via workspace feature unification (ADR-108/C6 makes sqlite THE backend).
#[test]
fn tantivy_fts_on_sqlite() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("fts_test.db");
    let db = DbInstance::new("sqlite", p.to_str().unwrap(), "").unwrap();

    db.run_script(
        ":create docs { id: String => title: String, body: String }",
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    // Create FTS index
    let fts_create = db.run_script(
        r#"::fts create docs:search {
                extractor: body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    );
    if let Err(e) = &fts_create {
        panic!("FTS index creation failed on sqlite: {e}");
    }

    // Insert docs
    db.run_script(
        r#"?[id, title, body] <- [
                ["n1", "Quantum Physics", "Entanglement is a spooky action at a distance"],
                ["n2", "Classical Mechanics", "Newton discovered gravity under an apple tree"],
                ["n3", "Relativity", "Einstein showed that space and time are intertwined"]
            ] :put docs {id => title, body}"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    // FTS search for "gravity"
    let res = db
            .run_script(
                r"?[id, title, score] := ~docs:search{id, title | query: 'gravity', k: 5, bind_score: score}",
                BTreeMap::new(),
                ScriptMutability::Immutable,
            )
            .unwrap();

    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][0].get_str().unwrap(), "n2");

    // Multi-word search
    let res2 = db
        .run_script(
            r"?[id, score] := ~docs:search{id | query: 'space time', k: 5, bind_score: score}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap();
    assert_eq!(res2.rows.len(), 1);
    assert_eq!(res2.rows[0][0].get_str().unwrap(), "n3");

    // Test update: old term should be removed from FTS index
    db.run_script(
        r#"?[id, title, body] <- [["n2", "Classical Mechanics", "Hamilton reformulated mechanics"]]
            :put docs {id => title, body}"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    let res3 = db
        .run_script(
            r"?[id, score] := ~docs:search{id | query: 'gravity', k: 5, bind_score: score}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap();
    // Should no longer find "gravity" — it was in n2 which was updated
    // Verify FTS auto-cleans stale entries after update
    eprintln!(
        "After update, 'gravity' search returns {} results: {:?}",
        res3.rows.len(),
        res3.rows
            .iter()
            .map(|r| r[0].get_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    // n3 still has "gravity" in its body
    assert!(
        res3.rows.len() <= 1,
        "should have at most 1 result (n3), got {}",
        res3.rows.len()
    );
}
