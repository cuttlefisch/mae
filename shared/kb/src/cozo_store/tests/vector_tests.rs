use super::*;

#[test]
fn store_and_search_embeddings() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("emb:1", "First", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("emb:2", "Second", NodeKind::Concept, ""))
        .unwrap();

    // Create synthetic 384-dim vectors (all-MiniLM-L6-v2 dimensionality)
    let mut v1 = vec![0.0f32; 384];
    v1[0] = 1.0; // point along dim 0
    let mut v2 = vec![0.0f32; 384];
    v2[1] = 1.0; // point along dim 1
    let mut query = vec![0.0f32; 384];
    query[0] = 0.9;
    query[1] = 0.1; // close to v1

    store.store_embedding("emb:1", "test-model", &v1).unwrap();
    store.store_embedding("emb:2", "test-model", &v2).unwrap();

    let hits = store.vector_search(&query, 2).unwrap();
    assert_eq!(hits.len(), 2);
    // emb:1 should be closer (lower cosine distance) to query
    assert_eq!(hits[0].id, "emb:1", "nearest neighbor should be emb:1");
    assert!(
        hits[0].distance < hits[1].distance,
        "emb:1 should have lower distance than emb:2"
    );
}

#[test]
fn graphrag_expands_neighbors() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "gr:1",
            "Vector Hit",
            NodeKind::Concept,
            "See [[gr:2]]",
        ))
        .unwrap();
    store
        .insert_node(&Node::new("gr:2", "Linked Neighbor", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("gr:3", "Unrelated", NodeKind::Concept, ""))
        .unwrap();

    // Embed only gr:1 — gr:2 should appear via graph expansion
    let mut v1 = vec![0.0f32; 384];
    v1[0] = 1.0;
    store.store_embedding("gr:1", "test-model", &v1).unwrap();

    // gr:3 is embedded far away
    let mut v3 = vec![0.0f32; 384];
    v3[383] = 1.0;
    store.store_embedding("gr:3", "test-model", &v3).unwrap();

    let mut query = vec![0.0f32; 384];
    query[0] = 1.0;

    let hits = store.graphrag_search(&query, 1).unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(ids.contains(&"gr:1"), "vector hit should be included");
    assert!(
        ids.contains(&"gr:2"),
        "graph neighbor should be included via expansion"
    );
}

// ADR-061 Phase B: content-addressed embedding cache.

#[test]
fn cached_embedding_survives_a_real_daemon_restart() {
    // Deliberately does NOT use `make_store()`'s tuple directly for the second
    // open -- this test's whole point is proving genuine on-disk persistence,
    // not in-memory memoization within one process lifetime. A naive
    // in-memory-only cache would pass a same-process check but fail this one,
    // so the store is fully dropped and reopened at the identical path.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_cozo");
    let vec = vec![0.1_f32, 0.2, -0.3, 0.4];
    {
        let store = crate::cozo_store::CozoKbStore::open(&path).unwrap();
        store
            .put_cached_embedding("hash-abc", "nomic-embed-text", 1, &vec)
            .unwrap();
        // store dropped here -- genuinely closes the on-disk engine.
    }
    let reopened = crate::cozo_store::CozoKbStore::open(&path).unwrap();
    let hit = reopened
        .get_cached_embedding("hash-abc", "nomic-embed-text", 1)
        .unwrap();
    assert_eq!(
        hit,
        Some(vec),
        "a cached embedding must survive the store being fully closed and reopened"
    );
}

#[test]
fn model_or_chunk_version_bump_invalidates_only_the_affected_entries() {
    let (_tmp, store) = make_store();
    let original = vec![1.0_f32, 0.0, 0.0];
    store
        .put_cached_embedding("hash-x", "model-a", 1, &original)
        .unwrap();

    // A different model, same content+chunk_version -> miss (never computed
    // under this key), and does NOT disturb the original entry.
    assert_eq!(
        store.get_cached_embedding("hash-x", "model-b", 1).unwrap(),
        None,
        "a different model_id must be a cache miss, not silently reuse model-a's vector"
    );
    // A bumped chunk_version, same content+model -> also a miss.
    assert_eq!(
        store.get_cached_embedding("hash-x", "model-a", 2).unwrap(),
        None,
        "a bumped chunk_version must be a cache miss, not silently reuse the old chunking's vector"
    );
    // The ORIGINAL key is untouched by either miss above.
    assert_eq!(
        store.get_cached_embedding("hash-x", "model-a", 1).unwrap(),
        Some(original.clone()),
        "the original (model, chunk_version) entry must remain exactly as cached"
    );

    // Now actually populate the bumped-chunk_version key with a genuinely
    // different vector, and confirm BOTH entries coexist independently --
    // proving a version bump invalidates exactly the affected entry (a fresh
    // recompute lands under its own key) without touching the other.
    let recomputed = vec![0.0_f32, 1.0, 0.0];
    store
        .put_cached_embedding("hash-x", "model-a", 2, &recomputed)
        .unwrap();
    assert_eq!(
        store.get_cached_embedding("hash-x", "model-a", 1).unwrap(),
        Some(original),
        "the old chunk_version's entry must still be exactly what it was"
    );
    assert_eq!(
        store.get_cached_embedding("hash-x", "model-a", 2).unwrap(),
        Some(recomputed),
        "the new chunk_version's entry must be the freshly-computed vector"
    );
}

#[test]
fn cache_lookup_is_a_clean_miss_for_never_cached_content() {
    let (_tmp, store) = make_store();
    assert_eq!(
        store
            .get_cached_embedding("never-seen", "any-model", 1)
            .unwrap(),
        None
    );
}
