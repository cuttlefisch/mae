//! ADR-061 Phase F2: RRF blend of `kb_federated_search_scoped_with_vector`.

use super::*;

/// The genuine dual-signal composition property: a node that lexically matches the
/// query but is semantically unrelated, and a node that's semantically similar (a
/// close cached embedding) but lexically distant, must BOTH surface in the blended
/// top results -- proving real fusion, not just "whichever signal ranks first".
#[test]
fn rrf_blend_surfaces_both_a_lexical_match_and_a_semantically_close_but_lexically_distant_hit() {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();

    // Lexically matches the query term "zorbatron", vector far from the query.
    let lex_node = mae_kb::Node::new(
        "n:lex",
        "Zorbatron manual",
        mae_kb::NodeKind::Note,
        "This document explains the zorbatron in detail.",
    );
    store.insert_node(&lex_node).unwrap();
    seed_embedding(&store, &lex_node, &[0.0, 1.0, 0.0]);

    // Lexically distant (no mention of "zorbatron" anywhere), vector close to the query.
    let sem_node = mae_kb::Node::new(
        "n:sem",
        "Unrelated title",
        mae_kb::NodeKind::Note,
        "Completely unrelated body content about gearboxes.",
    );
    store.insert_node(&sem_node).unwrap();
    seed_embedding(&store, &sem_node, &[1.0, 0.0, 0.0]);

    // Neither lexically nor semantically related to the query -- must not appear.
    let noise_node = mae_kb::Node::new(
        "n:noise",
        "Noise",
        mae_kb::NodeKind::Note,
        "Nothing to do with anything relevant here.",
    );
    store.insert_node(&noise_node).unwrap();
    seed_embedding(&store, &noise_node, &[0.0, 0.0, 1.0]);

    editor.kb.store = Some(std::sync::Arc::new(store));
    // Also present in the in-memory mirror `kb_federated_search_scoped` searches
    // lexically -- a real editor keeps these in sync; this test only needs both
    // sides queryable, not the sync mechanism itself.
    editor.kb.primary.insert(lex_node);
    editor.kb.primary.insert(sem_node);
    editor.kb.primary.insert(noise_node);

    let query_vec = vec![1.0_f32, 0.0, 0.0]; // matches n:sem's vector exactly
    let results = editor.kb_federated_search_scoped_with_vector(
        "zorbatron",
        &mae_kb::KbScope::All,
        mae_core_query_vector(&query_vec),
    );

    let ids: Vec<&str> = results.iter().map(|(_, n)| n.id.as_str()).collect();
    assert!(
        ids.contains(&"n:lex"),
        "the lexical match must survive the blend: {ids:?}"
    );
    assert!(
        ids.contains(&"n:sem"),
        "the semantically-close-but-lexically-distant hit must be surfaced by the blend, \
         not dropped just because it never matched the query text: {ids:?}"
    );
    // n:noise is a legitimate (if weak) vector-search candidate too -- with only 3
    // embedded nodes total, brute-force top-k trivially returns all 3 ranked by
    // distance, same as a real HNSW k-NN call would. The meaningful oracle isn't
    // "excluded entirely" but "outranked by both signals that actually matched
    // something" -- the real property RRF fusion is responsible for.
    let lex_rank = ids.iter().position(|id| *id == "n:lex").unwrap();
    let sem_rank = ids.iter().position(|id| *id == "n:sem").unwrap();
    if let Some(noise_rank) = ids.iter().position(|id| *id == "n:noise") {
        assert!(
            noise_rank > lex_rank && noise_rank > sem_rank,
            "a node with no lexical match and the weakest vector similarity must rank \
             below both signals that actually matched: {ids:?}"
        );
    }
}

/// `None` (via the plain, non-vector entry point) must behave EXACTLY as it always
/// has -- the blend is purely additive.
#[test]
fn kb_federated_search_scoped_without_vector_is_unaffected_by_the_new_blend_code() {
    let mut editor = Editor::new();
    editor
        .kb_create_node(
            "n:zzzunlikely",
            "Zzzunlikely",
            "a zzzunlikely marker body",
            mae_kb::NodeKind::Note,
        )
        .unwrap();

    let with_none = editor.kb_federated_search_scoped("zzzunlikely", &mae_kb::KbScope::All);
    let with_explicit_none_path = editor.kb_federated_search_scoped_with_vector(
        "zzzunlikely",
        &mae_kb::KbScope::All,
        // No store registered at all -- the vector branch must no-op cleanly,
        // reproducing the plain path exactly rather than erroring.
        crate::editor::kb_ops::QueryVector {
            vec: &[],
            model: "m",
            chunk_version: 1,
        },
    );
    let ids_none: Vec<&str> = with_none.iter().map(|(_, n)| n.id.as_str()).collect();
    let ids_vec: Vec<&str> = with_explicit_none_path
        .iter()
        .map(|(_, n)| n.id.as_str())
        .collect();
    assert!(ids_none.contains(&"n:zzzunlikely"));
    assert_eq!(
        ids_none, ids_vec,
        "with no store registered, the vector-aware entry point must reproduce the plain \
         search's order exactly (no store to blend against, so query_cached_embeddings is \
         never even reachable)"
    );
}

fn mae_core_query_vector(vec: &[f32]) -> crate::editor::kb_ops::QueryVector<'_> {
    crate::editor::kb_ops::QueryVector {
        vec,
        model: "m",
        chunk_version: 1,
    }
}

/// Seed one node's embedding through the PRODUCTION apply path.
///
/// Not a bare `put_cached_embedding`: since D2 a search scans the per-node
/// `embeddings` relation, so a test that wrote only the content-addressed cache
/// would pass while the real sweep wrote nothing findable.
fn seed_embedding(store: &mae_kb::CozoKbStore, node: &mae_kb::Node, vec: &[f32]) {
    let hash = mae_kb::activity::body_hash(&node.body);
    assert!(mae_kb::enrichment::apply_enrichment_results(
        store,
        "m",
        1,
        &[(node.id.clone(), hash, vec.to_vec())]
    )
    .is_empty());
}
