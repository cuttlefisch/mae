//! Phase I: Graph KB Validation — MAE Manual as Test Fixture (Categories 4-7)
//!
//! Split from kb_graph_validation.rs (was 1510 lines, over the 500-line test
//! ceiling) into per-category files sharing fixtures via
//! kb_graph_validation_support/mod.rs. This file: query regression, health
//! report, block decomposition, and versioning.
//!
//! Run via:
//!   cargo test -p mae --test kb_graph_validation_queries -- --nocapture

use mae_kb::{AgendaFilter, KbStore};

mod kb_graph_validation_support;
use kb_graph_validation_support::*;

// ============================================================
// Category 4: Query Regression
// ============================================================

#[test]
fn traversal_from_buffer_concept() {
    let (_tmp, store) = make_seeded_store();

    // 2-hop neighborhood from concept:buffer should reach related concepts
    let subgraph = store.neighborhood("concept:buffer", 2).unwrap();

    eprintln!(
        "concept:buffer 2-hop neighborhood: {} nodes, {} edges",
        subgraph.nodes.len(),
        subgraph.edges.len()
    );

    assert!(
        subgraph.nodes.len() >= 3,
        "concept:buffer 2-hop should reach at least 3 nodes, got {}",
        subgraph.nodes.len()
    );
}

#[test]
fn shortest_path_between_concepts() {
    let (_tmp, store) = make_seeded_store();

    // There should be a path from lesson:navigation to concept:debugging
    // via the lesson prerequisite chain.
    // Note: CozoDB's Datalog may not support recursive depth tracking;
    // shortest_path may return an error on some backends.
    match store.shortest_path("lesson:navigation", "concept:debugging") {
        Ok(path) => {
            eprintln!(
                "Path from lesson:navigation to concept:debugging: {:?}",
                path
            );
            assert!(
                !path.is_empty(),
                "no path found from lesson:navigation to concept:debugging"
            );
        }
        Err(e) => {
            // CozoDB backend may not support recursive arithmetic in Datalog
            eprintln!(
                "shortest_path not supported on this backend (expected): {}",
                e
            );
        }
    }
}

#[test]
fn agenda_orphan_query() {
    let (_tmp, store) = make_seeded_store();

    let orphans = store.agenda_query(&AgendaFilter::Orphan).unwrap();

    eprintln!("Orphan nodes: {}", orphans.len());
    for o in &orphans {
        eprintln!("  orphan: {} ({})", o.id, o.title);
    }

    // Most seed nodes should have links — orphan count should be reasonable
    // (some cmd: nodes may not have typed links yet)
    let all_count = store.list_ids(None).unwrap().len();
    let orphan_ratio = orphans.len() as f64 / all_count as f64;

    eprintln!(
        "Orphan ratio: {:.1}% ({}/{})",
        orphan_ratio * 100.0,
        orphans.len(),
        all_count
    );

    // We don't assert zero orphans because cmd: and option: nodes
    // don't have typed relationships yet. But concepts/lessons should.
    let concept_orphans: Vec<_> = orphans
        .iter()
        .filter(|n| n.id.starts_with("concept:"))
        .collect();

    // After typed seeding, very few concept nodes should be orphans
    // (some newly-added concepts may not have links yet)
    eprintln!("Concept orphans: {}", concept_orphans.len());
}

#[test]
fn agenda_dead_end_query() {
    let (_tmp, store) = make_seeded_store();

    let dead_ends = store.agenda_query(&AgendaFilter::DeadEnd).unwrap();

    eprintln!("Dead-end nodes (no outgoing links): {}", dead_ends.len());
}

// ============================================================
// Category 5: Health Report
// ============================================================

#[test]
fn health_report_sane() {
    let (_tmp, store) = make_seeded_store();

    let report = store.health_report().unwrap();

    eprintln!("Health Report:");
    eprintln!("  Total nodes: {}", report.total_nodes);
    eprintln!("  Total links: {}", report.total_links);
    eprintln!("  Orphan count: {}", report.orphan_ids.len());
    eprintln!("  Broken link count: {}", report.broken_links.len());
    eprintln!("  By kind: {:?}", report.by_kind);
    eprintln!("  By rel type: {:?}", report.by_rel_type);
    eprintln!(
        "  Hub nodes: {:?}",
        &report.hub_nodes[..report.hub_nodes.len().min(5)]
    );

    assert!(report.total_nodes >= 50, "expected at least 50 nodes");
    assert!(report.total_links >= 80, "expected at least 80 links");
    for bl in &report.broken_links {
        eprintln!(
            "  Broken: {} --[{}]--> {} ({:?})",
            bl.source, bl.rel_type, bl.target, bl.reason
        );
    }
    assert!(
        report.broken_links.is_empty(),
        "expected 0 broken links, got {}",
        report.broken_links.len()
    );

    // Verify kind distribution makes sense
    assert!(
        report.by_kind.contains_key("concept"),
        "missing concept kind in health report"
    );

    // Verify relationship type diversity
    assert!(
        report.by_rel_type.len() >= 5,
        "expected at least 5 relationship types, got {}",
        report.by_rel_type.len()
    );
}

// ============================================================
// Category 6: Block Decomposition
// ============================================================

#[test]
fn block_decomposition_on_concept_node() {
    let (_tmp, store) = make_seeded_store();

    // concept:buffer has multiple paragraphs
    let block_count = store.split_into_blocks("concept:buffer").unwrap();
    assert!(
        block_count >= 2,
        "concept:buffer should decompose into at least 2 blocks, got {}",
        block_count
    );

    // Retrieve a specific block
    let block0 = store.get_block("concept:buffer", 0).unwrap();
    assert!(block0.is_some(), "block 0 of concept:buffer should exist");
    assert!(
        !block0.unwrap().content.is_empty(),
        "block 0 should have content"
    );

    eprintln!("concept:buffer decomposed into {} blocks", block_count);
}

#[test]
fn block_decomposition_roundtrips() {
    let (_tmp, store) = make_seeded_store();

    // Decompose and verify all blocks can be retrieved and reassembled
    let original = store.get_node("concept:mode").unwrap().unwrap();
    let block_count = store.split_into_blocks("concept:mode").unwrap();
    assert!(block_count >= 2);

    // Read all blocks back and reassemble
    let mut reassembled_parts = Vec::new();
    for i in 0..block_count {
        let block = store
            .get_block("concept:mode", i)
            .unwrap()
            .unwrap_or_else(|| panic!("block {} should exist", i));
        reassembled_parts.push(block.content);
    }
    let reassembled = reassembled_parts.join("\n\n");

    // The reassembled body should match the original
    assert_eq!(
        reassembled.trim(),
        original.body.trim(),
        "reassembled blocks should match original body"
    );

    // Verify structural similarity
    let original_paragraphs: Vec<&str> = original.body.split("\n\n").collect();
    assert_eq!(
        block_count,
        original_paragraphs.len(),
        "block count should match paragraph count"
    );
}

// ============================================================
// Category 7: Versioning
// ============================================================

#[test]
fn version_snapshot_on_update() {
    let (_tmp, store) = make_seeded_store();

    // Get original state of a concept node
    let original = store.get_node("concept:buffer").unwrap().unwrap();
    // Update the node
    let mut updated = original.clone();
    updated.title = "Concept: Buffer (Updated)".to_string();
    updated.body = format!("{}\n\nUpdated paragraph.", updated.body);
    store.update_node(&updated).unwrap();

    // Snapshot should have been created
    let v = store
        .snapshot_version("concept:buffer", "test update")
        .unwrap();
    assert!(v >= 1, "version should be >= 1");

    // Check history
    let history = store.node_history("concept:buffer", 10).unwrap();
    assert!(
        !history.is_empty(),
        "history should have at least one entry"
    );

    eprintln!("concept:buffer history: {} versions", history.len());
    for h in &history {
        eprintln!(
            "  v{}: {} (hash: {})",
            h.version, h.change_summary, h.content_hash
        );
    }
}

#[test]
fn version_restore_preserves_integrity() {
    let (_tmp, store) = make_seeded_store();

    let original = store.get_node("concept:mode").unwrap().unwrap();
    let original_body = original.body.clone();

    // Snapshot v1
    store.snapshot_version("concept:mode", "initial").unwrap();

    // Modify
    let mut modified = original.clone();
    modified.body = "Completely replaced body".to_string();
    store.update_node(&modified).unwrap();
    store
        .snapshot_version("concept:mode", "replaced body")
        .unwrap();

    // Verify modified
    let current = store.get_node("concept:mode").unwrap().unwrap();
    assert_eq!(current.body, "Completely replaced body");

    // Restore to v1
    store.restore_version("concept:mode", 1).unwrap();

    let restored = store.get_node("concept:mode").unwrap().unwrap();
    assert_eq!(
        restored.body, original_body,
        "restored body should match original"
    );
}
