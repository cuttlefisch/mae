//! Phase I: Graph KB Validation — MAE Manual as Test Fixture (Categories 1-3)
//!
//! Split from kb_graph_validation.rs (was 1510 lines, over the 500-line test
//! ceiling) into per-category files sharing fixtures via
//! kb_graph_validation_support/mod.rs. This file: schema conformance, graph
//! integrity, and type-system metadata.
//!
//! Run via:
//!   cargo test -p mae --test kb_graph_validation_schema -- --nocapture

use std::collections::{HashMap, HashSet};

use mae_kb::KbStore;

mod kb_graph_validation_support;
use kb_graph_validation_support::*;

// Category 1: Schema Conformance
// ============================================================

#[test]
fn seed_nodes_loaded_into_cozodb() {
    let (_tmp, store) = make_seeded_store();
    let all_ids = store.list_ids(None).unwrap();
    // The manual has 400+ nodes (concepts, lessons, commands, keys, scheme, options, categories)
    assert!(
        all_ids.len() >= 50,
        "expected at least 50 seed nodes, got {}",
        all_ids.len()
    );
    eprintln!("Total seed nodes loaded: {}", all_ids.len());
}

#[test]
fn node_kind_matches_namespace_prefix() {
    let (_tmp, store) = make_seeded_store();
    let all_ids = store.list_ids(None).unwrap();

    let prefix_kind_map: Vec<(&str, &str)> = vec![
        ("cmd:", "command"),
        ("concept:", "concept"),
        ("lesson:", "lesson"),
        ("key:", "key"),
        ("scheme:", "schemeapi"),
        ("option:", "concept"), // options use Concept (no Option kind in enum)
        ("category:", "category"),
        ("tutor:", "tutorial"),
        ("tutorial:", "tutorial"),
        ("guide:", "concept"),
        ("term:", "concept"),
    ];

    let mut mismatches = Vec::new();
    for id in &all_ids {
        if let Some(node) = store.get_node(id).unwrap() {
            let kind_str = format!("{:?}", node.kind).to_lowercase();
            for (prefix, expected_kind) in &prefix_kind_map {
                if id.starts_with(prefix) && kind_str != *expected_kind {
                    mismatches.push(format!(
                        "{}: expected kind '{}' for prefix '{}', got '{}'",
                        id, expected_kind, prefix, kind_str
                    ));
                }
            }
        }
    }

    if !mismatches.is_empty() {
        eprintln!("NodeKind mismatches (unexpected):");
        for m in &mismatches {
            eprintln!("  {}", m);
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} NodeKind mismatches found",
        mismatches.len()
    );
}

#[test]
fn all_seed_nodes_have_title() {
    let (_tmp, store) = make_seeded_store();
    let all_ids = store.list_ids(None).unwrap();

    let empty_titles: Vec<_> = all_ids
        .iter()
        .filter(|id| {
            store
                .get_node(id)
                .unwrap()
                .is_none_or(|n| n.title.is_empty())
        })
        .collect();

    assert!(
        empty_titles.is_empty(),
        "nodes with empty titles: {:?}",
        empty_titles
    );
}

#[test]
fn concept_nodes_have_body_content() {
    let (_tmp, store) = make_seeded_store();
    let concept_ids = store.list_ids(Some("concept:")).unwrap();

    let empty_body: Vec<_> = concept_ids
        .iter()
        .filter(|id| {
            store
                .get_node(id)
                .unwrap()
                .is_none_or(|n| n.body.trim().is_empty())
        })
        .collect();

    assert!(
        empty_body.is_empty(),
        "concept nodes with empty bodies: {:?}",
        empty_body
    );
}

// ============================================================
// Category 2: Graph Integrity
// ============================================================

#[test]
fn typed_relationships_seeded() {
    let (_tmp, store) = make_seeded_store();

    // Query all non-related_to links (CozoDB doesn't support count() in rules directly)
    let (_, rows) = store
        .raw_query("?[src, dst, rt] := *links{src, dst, rel_type: rt}, rt != 'related_to'")
        .unwrap();

    let total_typed = rows.len();

    // Count by type in Rust
    let mut by_type: HashMap<String, usize> = HashMap::new();
    for row in &rows {
        let rt = dv_str(&row[2]);
        *by_type.entry(rt).or_default() += 1;
    }

    eprintln!("Typed relationships by type:");
    for (rt, count) in &by_type {
        eprintln!("  {}: {}", rt, count);
    }

    // Only 6 code-generated relationships from seed_typed_relationships().
    // Content relationships are now inline typed links in org files,
    // parsed during import_org_dir_to_store().
    assert!(
        total_typed >= 6,
        "expected at least 6 typed relationships, got {}",
        total_typed
    );
}

#[test]
fn lesson_prerequisite_chain_complete() {
    let (_tmp, store) = make_seeded_store();

    // The lesson chain: navigation → modes → editing → files → ai → scheme → lsp → terminal → help → leader → debugging → observability
    let chain = [
        "lesson:navigation",
        "lesson:modes",
        "lesson:editing",
        "lesson:files",
        "lesson:ai",
        "lesson:scheme",
        "lesson:lsp",
        "lesson:terminal",
        "lesson:help",
        "lesson:leader",
        "lesson:debugging",
        "lesson:observability",
    ];

    for window in chain.windows(2) {
        let later = window[1];
        let earlier = window[0];
        let links = store.links_typed(later, "requires").unwrap();
        let has_prereq = links.iter().any(|l| l.dst == earlier);
        assert!(
            has_prereq,
            "'{}' should require '{}' but doesn't. Links: {:?}",
            later,
            earlier,
            links.iter().map(|l| &l.dst).collect::<Vec<_>>()
        );
    }
}

#[test]
fn lessons_teach_concepts() {
    let (_tmp, store) = make_seeded_store();

    // Key lessons should teach their corresponding concepts
    let expected_teaches = [
        ("lesson:navigation", "concept:buffer"),
        ("lesson:modes", "concept:mode"),
        ("lesson:ai", "concept:ai-as-peer"),
        ("lesson:scheme", "concept:scheme-api"),
        ("lesson:terminal", "concept:terminal"),
        ("lesson:help", "concept:knowledge-base"),
        ("lesson:debugging", "concept:debugging"),
    ];

    for (lesson, concept) in &expected_teaches {
        let links = store.links_typed(lesson, "teaches").unwrap();
        let teaches_concept = links.iter().any(|l| l.dst == *concept);
        assert!(
            teaches_concept,
            "'{}' should teach '{}' but doesn't",
            lesson, concept
        );
    }
}

#[test]
fn no_broken_links_in_seed_relationships() {
    let (_tmp, store) = make_seeded_store();

    let (_, rows) = store
        .raw_query("?[src, dst, rt] := *links{src, dst, rel_type: rt}")
        .unwrap();

    let all_ids: HashSet<String> = store.list_ids(None).unwrap().into_iter().collect();

    let mut broken_typed = Vec::new();
    let mut broken_related = Vec::new();
    for row in &rows {
        let src = dv_str(&row[0]);
        let dst = dv_str(&row[1]);
        let rt = dv_str(&row[2]);
        let is_broken_src = !all_ids.contains(&src);
        let is_broken_dst = !all_ids.contains(&dst);
        if is_broken_src || is_broken_dst {
            let msg = if is_broken_src {
                format!("broken src: {} --[{}]--> {}", src, rt, dst)
            } else {
                format!("broken dst: {} --[{}]--> {}", src, rt, dst)
            };
            if rt == "related_to" {
                broken_related.push(msg);
            } else {
                broken_typed.push(msg);
            }
        }
    }

    // After fixing body text references, broken related_to links should be zero
    if !broken_related.is_empty() {
        eprintln!(
            "Broken related_to links (body text auto-extraction): {}",
            broken_related.len()
        );
        for b in &broken_related {
            eprintln!("  {}", b);
        }
    }
    assert!(
        broken_related.is_empty(),
        "{} broken related_to links found — fix body text references",
        broken_related.len()
    );

    // Typed relationships (seeded explicitly) should NEVER be broken
    if !broken_typed.is_empty() {
        eprintln!("Broken typed links:");
        for b in &broken_typed {
            eprintln!("  {}", b);
        }
    }
    assert!(
        broken_typed.is_empty(),
        "{} broken typed links found in seed relationships",
        broken_typed.len()
    );
}

#[test]
fn core_concepts_are_not_orphans() {
    let (_tmp, store) = make_seeded_store();

    // Core concepts should have at least one incoming or outgoing typed link
    let core_concepts = [
        "concept:buffer",
        "concept:mode",
        "concept:command",
        "concept:ai-as-peer",
        "concept:knowledge-base",
        "concept:scheme-api",
        "concept:debugging",
        "concept:terminal",
    ];

    for concept in &core_concepts {
        let from = store.links_from(concept).unwrap();
        let to = store.links_to(concept).unwrap();
        let total = from.len() + to.len();
        assert!(
            total > 0,
            "'{}' is an orphan (no incoming or outgoing links)",
            concept
        );
    }
}

// ============================================================
// Category 3: Type System Metadata
// ============================================================

#[test]
fn node_types_seeded() {
    let (_tmp, store) = make_seeded_store();

    let (_, rows) = store
        .raw_query("?[kind, label, prefix] := *node_types{kind, label, namespace_prefix: prefix}")
        .unwrap();

    // Debug: show raw format
    if let Some(first_row) = rows.first() {
        eprintln!("Raw first row: {:?}", first_row);
    }

    let kinds: HashSet<String> = rows
        .iter()
        .filter_map(|r| r.first().map(|s| dv_str(s)))
        .collect();

    eprintln!("Parsed kinds: {:?}", kinds);

    // All expected kinds should be present
    for expected in &[
        "index",
        "command",
        "concept",
        "key",
        "note",
        "project",
        "category",
        "lesson",
        "tutorial",
        "meta",
        "block",
        "scheme_api",
        "task",
        "view",
    ] {
        assert!(
            kinds.contains(*expected),
            "missing node type: '{}'",
            expected
        );
    }

    eprintln!("Node types: {:?}", kinds);
}

#[test]
fn rel_types_seeded_with_inverses() {
    let (_tmp, store) = make_seeded_store();

    let (_, rows) = store
        .raw_query("?[name, inverse] := *rel_types{name, inverse_name: inverse}")
        .unwrap();

    let rels: HashMap<String, String> = rows
        .iter()
        .filter_map(|r| {
            let name = dv_str(r.first()?);
            let inv = dv_str(r.get(1)?);
            Some((name, inv))
        })
        .collect();

    // Core relationship types
    for expected in &[
        "implements",
        "extends",
        "explains",
        "references",
        "part_of",
        "teaches",
        "requires",
        "contains",
        "categorized_under",
        "documents",
    ] {
        assert!(
            rels.contains_key(*expected),
            "missing relationship type: '{}'",
            expected
        );
        assert!(
            !rels[*expected].is_empty(),
            "relationship '{}' has no inverse",
            expected
        );
    }

    eprintln!("Relationship types with inverses: {}", rels.len());
}
