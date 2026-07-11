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
