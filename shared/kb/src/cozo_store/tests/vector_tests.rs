use super::*;

/// The dimension the SHIPPED DEFAULT `ai_embedding_model` (`nomic-embed-text`)
/// actually emits.
///
/// These tests used to hand-pick **384** -- all-MiniLM-L6-v2's width, and the
/// width the `embeddings` relation was hardcoded to. That is a unicorn value
/// chosen around the defect (principle #14): at the default model's real width,
/// `store_embedding` returned
/// `Err(Storage("CozoDB: when executing against relation 'embeddings'"))`, so the
/// only vectors the relation ever accepted were ones no shipped configuration
/// produces. Every test in this file now uses the default model's width, so the
/// suite exercises the configuration users actually run.
const DEFAULT_MODEL_DIM: usize = 768;
const DEFAULT_MODEL: &str = "nomic-embed-text";

fn unit_vec(dim: usize, axis: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    v[axis] = 1.0;
    v
}

#[test]
fn store_and_search_embeddings_at_the_default_models_dimension() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("emb:1", "First", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("emb:2", "Second", NodeKind::Concept, ""))
        .unwrap();

    let v1 = unit_vec(DEFAULT_MODEL_DIM, 0);
    let v2 = unit_vec(DEFAULT_MODEL_DIM, 1);
    let mut query = vec![0.0f32; DEFAULT_MODEL_DIM];
    query[0] = 0.9;
    query[1] = 0.1; // close to v1

    store
        .store_embedding("emb:1", DEFAULT_MODEL, "h1", &v1)
        .unwrap();
    store
        .store_embedding("emb:2", DEFAULT_MODEL, "h2", &v2)
        .unwrap();

    let hits = store
        .vector_search_for_model(DEFAULT_MODEL, &query, 2)
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, "emb:1", "nearest neighbor should be emb:1");
    assert!(
        hits[0].distance < hits[1].distance,
        "emb:1 should have lower distance than emb:2"
    );
}

/// The regression this whole change exists for: a 768-dim vector -- what the
/// shipped default model emits -- must be storable and findable.
///
/// Before D2 this failed. The relation was created eagerly at `<F32; 384>`, so
/// the store rejected the only vectors a default install could ever produce, and
/// the error named neither the dimension nor the model.
#[test]
fn the_shipped_default_models_width_is_storable() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("d:1", "Doc", NodeKind::Concept, ""))
        .unwrap();
    let v = unit_vec(DEFAULT_MODEL_DIM, 7);
    store
        .store_embedding("d:1", DEFAULT_MODEL, "h", &v)
        .expect("the default embedding model's dimension must be storable");
    let hits = store.vector_search_for_model(DEFAULT_MODEL, &v, 1).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d:1");
}

/// Changing `ai_embedding_model` changes the vector width. The store must re-pin
/// rather than reject forever -- and must say what happened if it cannot.
#[test]
fn changing_the_model_re_pins_the_store_to_the_new_width() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("m:1", "N", NodeKind::Concept, ""))
        .unwrap();

    store
        .store_embedding("m:1", "all-minilm-l6-v2", "h384", &unit_vec(384, 0))
        .unwrap();
    assert_eq!(
        store
            .vector_search_for_model("all-minilm-l6-v2", &unit_vec(384, 0), 1)
            .unwrap()
            .len(),
        1
    );

    // Same store, a model with a different width.
    let v768 = unit_vec(DEFAULT_MODEL_DIM, 0);
    store
        .store_embedding("m:1", DEFAULT_MODEL, "h768", &v768)
        .expect("a width change must re-pin the relation, not fail permanently");
    let hits = store
        .vector_search_for_model(DEFAULT_MODEL, &v768, 1)
        .unwrap();
    assert_eq!(hits.len(), 1, "the new-width vector must be searchable");

    // The re-pin DROPS the old-width rows. That is the honest outcome and it is
    // asserted rather than left to chance: vectors from different models are not
    // comparable anyway, and `embedding_cache` still holds every vector ever
    // computed, so rebuilding costs a local rescan and no re-embedding calls.
    assert!(
        store
            .vector_search_for_model("all-minilm-l6-v2", &unit_vec(384, 0), 1)
            .unwrap()
            .is_empty(),
        "old-width rows must be gone after a re-pin, not silently mixed in"
    );
}

/// Vectors from two different models must never be mixed into one ranking:
/// their spaces are not comparable, so a search that ignored the pin would
/// return confidently-wrong neighbours.
#[test]
fn a_search_never_mixes_models() {
    let (_tmp, store) = make_store();
    for id in ["x:1", "x:2"] {
        store
            .insert_node(&Node::new(id, id, NodeKind::Concept, ""))
            .unwrap();
    }
    store
        .store_embedding("x:1", "model-a", "ha", &unit_vec(DEFAULT_MODEL_DIM, 0))
        .unwrap();
    store
        .store_embedding("x:2", "model-b", "hb", &unit_vec(DEFAULT_MODEL_DIM, 0))
        .unwrap();

    let hits = store
        .vector_search_for_model("model-a", &unit_vec(DEFAULT_MODEL_DIM, 0), 10)
        .unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["x:1"],
        "only the queried model's vectors may appear in its ranking"
    );
}

/// A store that has never been enriched has nothing to search -- and that is a
/// clean empty result, not an error about a missing relation.
#[test]
fn searching_a_never_enriched_store_is_empty_not_an_error() {
    let (_tmp, store) = make_store();
    let hits = store
        .vector_search_for_model(DEFAULT_MODEL, &unit_vec(DEFAULT_MODEL_DIM, 0), 5)
        .expect("an un-enriched store must not error");
    assert!(hits.is_empty());
}

#[test]
fn a_zero_length_embedding_is_refused() {
    let (_tmp, store) = make_store();
    assert!(
        store
            .store_embedding("z:1", DEFAULT_MODEL, "h", &[])
            .is_err(),
        "a zero-length vector would pin the relation to width 0"
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

    // Embed only gr:1 — gr:2 should appear via graph expansion.
    store
        .store_embedding("gr:1", DEFAULT_MODEL, "h1", &unit_vec(DEFAULT_MODEL_DIM, 0))
        .unwrap();
    // gr:3 is embedded far away.
    store
        .store_embedding(
            "gr:3",
            DEFAULT_MODEL,
            "h3",
            &unit_vec(DEFAULT_MODEL_DIM, DEFAULT_MODEL_DIM - 1),
        )
        .unwrap();

    let hits = store
        .graphrag_search(DEFAULT_MODEL, &unit_vec(DEFAULT_MODEL_DIM, 0), 1)
        .unwrap();
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
