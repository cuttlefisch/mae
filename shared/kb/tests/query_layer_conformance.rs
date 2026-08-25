//! Cross-implementation conformance for `KbQueryLayer`.
//!
//! MAE runs **three divergent implementations of "search"** behind one seam —
//! `search_ranked_pass`'s hand-rolled field weights, Cozo's `nodes:fts`, and the
//! daemon's unranked `title.contains()` capped at `max_scan_nodes`. ADR-035
//! named the consequence: *"that gap **is** the dual-mode tax surfacing."*
//! `remote_hub.rs` confesses it in its own comment — its synthetic scores are
//! *"NOT comparable in magnitude to a real BM25-style local score."*
//!
//! Nothing compared them until this file.
//!
//! # The contract, and why it needs the capability model
//!
//! > For every method an implementation **declares** it supports
//! > (`KbQueryLayer::capabilities`), its answers must agree with the reference.
//! > A method it does **not** declare is a *declared gap* — reported, never a
//! > failure.
//!
//! Without that split the suite is unwritable: `RemoteHubQueryLayer` genuinely
//! cannot answer twelve of seventeen methods, so it would fail forever and the
//! suite would end up `#[ignore]`d — the usual fate of a suite that cannot
//! express legitimate difference.
//!
//! Agreement is asserted on **observable properties**, not on byte-identical
//! output. Two backings may legitimately rank differently; they may not
//! legitimately disagree about whether a node exists.

use mae_kb::capabilities::QueryMethod;
use mae_kb::query::{InMemoryQueryLayer, KbQueryLayer};
use mae_kb::{KnowledgeBase, Node, NodeKind};

/// A corpus with enough structure that the properties below are non-trivial:
/// links in both directions, distinct namespaces, and a node whose title alone
/// would not match the body search term.
fn corpus() -> Vec<Node> {
    vec![
        Node::new(
            "concept:alpha",
            "Alpha",
            NodeKind::Note,
            "the alpha note, which links to [[concept:beta]] and mentions widgets",
        ),
        Node::new(
            "concept:beta",
            "Beta",
            NodeKind::Note,
            "the beta note, linking onward to [[concept:gamma]]",
        ),
        Node::new("concept:gamma", "Gamma", NodeKind::Note, "a leaf note"),
        Node::new(
            "task:one",
            "First task",
            NodeKind::Note,
            "unrelated, mentions widgets",
        ),
    ]
}

fn in_memory() -> InMemoryQueryLayer {
    let mut kb = KnowledgeBase::new();
    for n in corpus() {
        kb.insert(n);
    }
    InMemoryQueryLayer::new(kb)
}

/// Every implementation under test, by name. The reference is first.
fn layers() -> Vec<(&'static str, Box<dyn KbQueryLayer>)> {
    let mut v: Vec<(&'static str, Box<dyn KbQueryLayer>)> =
        vec![("InMemoryQueryLayer", Box::new(in_memory()))];

    // The Cozo-backed layer is the one that actually differs — its search is a
    // scored FTS index while the in-memory layer uses hand-rolled field weights.
    //
    // `mem`, not `sqlite`: conformance is about ANSWERS, not durability, so the
    // suite must not depend on which storage feature happens to be enabled —
    // otherwise it silently skips wherever that feature is off, which is exactly
    // how a conformance suite quietly stops conforming.
    let store = std::sync::Arc::new(
        mae_kb::CozoKbStore::open_with_engine(std::path::PathBuf::from(""), "mem")
            .expect("open in-memory cozo store"),
    );
    for n in corpus() {
        mae_kb::KbStore::insert_node(&*store, &n).expect("seed");
    }
    v.push((
        "CozoQueryLayer",
        Box::new(mae_kb::query::CozoQueryLayer::new(store)),
    ));
    v
}

/// **Existence is not a matter of opinion.** Ranking may differ between
/// backings; whether a node is in the corpus may not. This is the property that
/// would have caught a backing silently returning empty for a node it holds —
/// the shape `RemoteHubQueryLayer`'s give-up sites have today.
#[test]
fn every_layer_agrees_on_which_nodes_exist() {
    for (name, layer) in layers() {
        if !layer.capabilities().supports(QueryMethod::Contains) {
            continue;
        }
        for n in corpus() {
            assert!(
                layer.contains(&n.id),
                "{name} does not contain {} — existence must not vary by backing",
                n.id
            );
        }
        assert!(
            !layer.contains("concept:does-not-exist"),
            "{name} claims a node that was never inserted"
        );
    }
}

/// A declared method must not answer by shrugging. An empty result for a query
/// that demonstrably matches is the failure mode the capability model exists to
/// make visible: it is either a bug or an undeclared gap, and both are faults.
#[test]
fn a_declared_search_actually_searches() {
    for (name, layer) in layers() {
        if !layer.capabilities().supports(QueryMethod::Search) {
            continue;
        }
        let hits = layer.search("widgets", 10).unwrap_or_else(|e| {
            panic!("{name} declares Search but errored: {e}");
        });
        assert!(
            !hits.is_empty(),
            "{name} declares Search but returned nothing for a term present in two nodes — \
             either a bug, or Search should not be declared"
        );
    }
}

/// `get` and `contains` must not disagree with each other within one backing.
/// A backing that says "yes I have it" and then returns `None` is worse than
/// one that admits the gap.
#[test]
fn get_and_contains_never_contradict_each_other() {
    for (name, layer) in layers() {
        let caps = layer.capabilities();
        if !(caps.supports(QueryMethod::Get) && caps.supports(QueryMethod::Contains)) {
            continue;
        }
        for n in corpus() {
            assert_eq!(
                layer.contains(&n.id),
                layer.get(&n.id).is_some(),
                "{name}: contains() and get() disagree about {}",
                n.id
            );
        }
    }
}

/// The gaps a backing declares must be **stable and inspectable** — that is what
/// makes "shrink the declared-gap set to zero" a countable definition of network
/// parity (D1's gate) rather than a vague one.
#[test]
fn declared_gaps_are_inspectable_and_stable() {
    for (name, layer) in layers() {
        let first = layer.capabilities();
        let second = layer.capabilities();
        assert_eq!(
            first, second,
            "{name}: capabilities() is not stable across calls"
        );
        for m in first.gaps() {
            assert!(
                !first.supports(m),
                "{name}: {} reported as both gap and supported",
                m.name()
            );
        }
    }
}

/// The reference implementation must declare full support. If the *reference*
/// starts excusing itself, every comparison above silently weakens.
#[test]
fn the_reference_implementation_declares_no_gaps() {
    let (name, layer) = layers().remove(0);
    let gaps: Vec<&str> = layer
        .capabilities()
        .gaps()
        .iter()
        .map(|m| m.name())
        .collect();
    assert!(
        gaps.is_empty(),
        "{name} is the conformance reference and must support everything, but declares gaps: {gaps:?}"
    );
}

/// **The divergence test.** Ranking may legitimately differ between backings —
/// one uses a scored FTS index, the other hand-rolled field weights. The SET of
/// documents containing an unambiguous term may not.
///
/// This is the property ADR-035's "dual-mode tax" is really about: a user who
/// searches on the hub and on their laptop should not be told the term appears
/// in different documents.
#[test]
fn declared_search_implementations_agree_on_which_documents_match() {
    let all = layers();
    let (ref_name, reference) = &all[0];
    if !reference.capabilities().supports(QueryMethod::Search) {
        return;
    }
    let expected: std::collections::BTreeSet<String> = reference
        .search("widgets", 50)
        .expect("reference search")
        .into_iter()
        .map(|h| h.id)
        .collect();

    for (name, layer) in &all[1..] {
        if !layer.capabilities().supports(QueryMethod::Search) {
            continue;
        }
        let got: std::collections::BTreeSet<String> = layer
            .search("widgets", 50)
            .unwrap_or_else(|e| panic!("{name} declares Search but errored: {e}"))
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(
            got, expected,
            "{name} and {ref_name} disagree about which documents contain \"widgets\" — \
             ranking may differ between backings, membership may not"
        );
    }
}
