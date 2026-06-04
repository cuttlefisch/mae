//! Scale test: validates CozoDB performance at roamnotes-scale loads
//! (2,500 nodes, 15,000 links).
//!
//! Thresholds are set for debug builds. Release builds are 5-10x faster.

use mae_kb::store::KbStore;
use mae_kb::{CozoKbStore, Node, NodeKind};
use std::time::Instant;

fn create_scale_store() -> (tempfile::TempDir, CozoKbStore) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("scale-test.cozo");
    let store = CozoKbStore::open(&db_path).unwrap();
    (dir, store)
}

fn populate_store(store: &CozoKbStore, node_count: usize, links_per_node: usize) {
    let kinds = [
        NodeKind::Concept,
        NodeKind::Lesson,
        NodeKind::Tutorial,
        NodeKind::Category,
        NodeKind::SchemeApi,
        NodeKind::Command,
    ];

    for i in 0..node_count {
        let kind = kinds[i % kinds.len()];
        let mut node = Node::new(
            format!("test-node-{i}"),
            format!("Test Node {i}: {}", lorem_title(i)),
            kind,
            lorem_body(i),
        );
        node.tags = vec![format!("tag-{}", i % 20), format!("group-{}", i % 5)];
        store.insert_node(&node).unwrap();
    }

    let rel_types = [
        "references",
        "teaches",
        "implements",
        "extends",
        "requires",
        "part_of",
        "categorizes",
        "related_to",
    ];
    for i in 0..node_count {
        for j in 0..links_per_node {
            let dst_idx = (i * 7 + j * 13 + 3) % node_count;
            if dst_idx == i {
                continue;
            }
            store
                .add_typed_link(
                    &format!("test-node-{i}"),
                    &format!("test-node-{dst_idx}"),
                    rel_types[(i + j) % rel_types.len()],
                    1.0,
                )
                .unwrap();
        }
    }
}

fn lorem_title(i: usize) -> &'static str {
    const TITLES: &[&str] = &[
        "Buffer Management",
        "Window System",
        "Modal Editing",
        "Syntax Highlighting",
        "LSP Integration",
        "Debug Adapter Protocol",
        "Knowledge Graph",
        "Collaborative Editing",
        "Scheme Runtime",
        "Configuration System",
    ];
    TITLES[i % TITLES.len()]
}

fn lorem_body(i: usize) -> String {
    format!(
        "This is the body of test node {i}. It contains information about {} \
         and relates to various concepts in the editor architecture. \
         The node demonstrates how content scales in the knowledge base \
         with realistic text lengths that mirror actual documentation.",
        lorem_title(i)
    )
}

fn assert_under_ms(label: &str, elapsed_ms: f64, threshold_ms: f64) {
    assert!(
        elapsed_ms < threshold_ms,
        "{label} took {elapsed_ms:.1}ms, expected < {threshold_ms}ms"
    );
}

#[test]
#[ignore] // Slow in debug builds (~40s). Run explicitly: cargo test -p mae-kb --test cozo_scale_test -- --ignored
fn scale_2500_nodes_15000_links() {
    let (_dir, store) = create_scale_store();

    // Phase 1: Bulk insert (2500 nodes + ~15000 typed links)
    let start = Instant::now();
    populate_store(&store, 2500, 6);
    let insert_ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Bulk insert (2500 nodes + links): {insert_ms:.0}ms");
    assert!(
        insert_ms < 300_000.0,
        "Bulk insert took too long: {insert_ms:.0}ms"
    );

    // Phase 2: Single node lookup
    let start = Instant::now();
    for i in (0..2500).step_by(100) {
        let node = store.get_node(&format!("test-node-{i}")).unwrap();
        assert!(node.is_some());
    }
    let get_ms = start.elapsed().as_secs_f64() * 1000.0 / 25.0;
    eprintln!("get_node (avg over 25 lookups): {get_ms:.2}ms");
    assert_under_ms("get_node", get_ms, 500.0);

    // Phase 3: Full-text search
    let start = Instant::now();
    let results = store.fts_search("Buffer Management", 50).unwrap();
    let search_ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "FTS search: {search_ms:.1}ms, {len} results",
        len = results.len()
    );
    assert!(!results.is_empty(), "FTS should find results");
    assert_under_ms("FTS search", search_ms, 5000.0);

    // Phase 4: Links from (one-hop traversal)
    let start = Instant::now();
    for i in (0..2500).step_by(250) {
        let links = store.links_from(&format!("test-node-{i}")).unwrap();
        assert!(!links.is_empty());
    }
    let links_ms = start.elapsed().as_secs_f64() * 1000.0 / 10.0;
    eprintln!("links_from (avg over 10 lookups): {links_ms:.2}ms");
    assert_under_ms("links_from", links_ms, 500.0);

    // Phase 5: Links to (reverse traversal)
    let start = Instant::now();
    for i in (0..2500).step_by(250) {
        let _links = store.links_to(&format!("test-node-{i}")).unwrap();
    }
    let backlinks_ms = start.elapsed().as_secs_f64() * 1000.0 / 10.0;
    eprintln!("links_to (avg over 10 lookups): {backlinks_ms:.2}ms");
    assert_under_ms("links_to", backlinks_ms, 500.0);

    // Phase 6: Neighborhood query (2-hop)
    let start = Instant::now();
    let neighborhood = store.neighborhood("test-node-0", 2).unwrap();
    let nbr_ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "neighborhood(2-hop): {nbr_ms:.1}ms, {} nodes",
        neighborhood.nodes.len()
    );
    assert!(!neighborhood.nodes.is_empty());
    assert_under_ms("neighborhood", nbr_ms, 60_000.0);

    // Phase 7: Datalog query (filter by tag)
    let start = Instant::now();
    let agenda_results = store.raw_query(
        r#"?[id, title] := *nodes{id, title, tags_json}, str_includes(tags_json, "tag-5")"#,
    );
    let agenda_ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Datalog query (tag filter): {agenda_ms:.1}ms");
    match &agenda_results {
        Ok((headers, rows)) => eprintln!("  headers: {headers:?}, {} rows", rows.len()),
        Err(e) => eprintln!("  ERROR: {e}"),
    }
    assert!(
        agenda_results.is_ok(),
        "Datalog query failed: {:?}",
        agenda_results.err()
    );
    assert_under_ms("datalog query", agenda_ms, 2000.0);

    // Phase 8: Load all nodes (startup simulation)
    let start = Instant::now();
    let all_nodes = store.load_all().unwrap();
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("load_all ({} nodes): {load_ms:.0}ms", all_nodes.len());
    assert_eq!(all_nodes.len(), 2500);
    assert_under_ms("load_all", load_ms, 30_000.0);

    eprintln!("\n--- Scale test summary ---");
    eprintln!("All queries within thresholds at 2,500 nodes / ~15,000 links");
}
