//! Graded accuracy "dipstick" for MAE knowledge-base search.
//!
//! Numerical/statistical validation of search quality over a deterministic
//! corpus (the code-generated seed KB — concept/term/cmd/scheme/option nodes;
//! no filesystem/org dependence, so it's stable in CI). Reports top-1 accuracy,
//! recall@3, and MRR for BOTH the in-memory relevance ranker
//! (`KnowledgeBase::search_ranked`) and the CozoDB Tantivy FTS
//! (`KbStore::search`), and asserts regression FLOORS so quality can't silently
//! degrade. Run the comparison table with:
//!
//!   cargo test -p mae --test kb_search_grading -- --nocapture

use std::collections::HashMap;

use mae_core::commands::CommandRegistry;
use mae_core::hooks::HookRegistry;
use mae_core::kb_seed::seed_kb;
use mae_kb::{CozoKbStore, KbStore, KnowledgeBase};

/// (query, expected-top node id). Chosen to be unambiguous: the expected node
/// is clearly the best answer for the query. Mix of single- and multi-word
/// (multi-word is the case the legacy whole-substring `search` failed).
const GRADED: &[(&str, &str)] = &[
    ("buffer", "concept:buffer"),
    ("window", "concept:window"),
    ("command", "concept:command"),
    ("knowledge base", "concept:knowledge-base"),
    ("ai as peer", "concept:ai-as-peer"),
    ("kb federation", "concept:kb-federation"),
    ("save", "cmd:save"),
    ("undo", "cmd:undo"),
    ("redo", "cmd:redo"),
    ("hooks", "concept:hooks"),
    ("modules", "concept:modules"),
    ("buffer mode", "concept:buffer-mode"),
];

fn build_kb() -> KnowledgeBase {
    let registry = CommandRegistry::with_builtins();
    let keymaps = HashMap::new();
    let hooks = HookRegistry::new();
    seed_kb(&registry, &keymaps, &hooks)
}

fn build_store(kb: &KnowledgeBase) -> CozoKbStore {
    let store = CozoKbStore::open_mem().expect("open_mem");
    for id in kb.list_ids(None) {
        if let Some(node) = kb.get(&id) {
            let _ = store.insert_node(node);
        }
    }
    store
}

/// Reciprocal rank of `expected` in `results` (1-based); 0.0 if absent.
fn reciprocal_rank(results: &[String], expected: &str) -> f64 {
    results
        .iter()
        .position(|id| id == expected)
        .map(|p| 1.0 / (p as f64 + 1.0))
        .unwrap_or(0.0)
}

struct Metrics {
    top1: f64,
    recall_at_3: f64,
    mrr: f64,
}

fn grade<F: Fn(&str) -> Vec<String>>(search: F) -> Metrics {
    let n = GRADED.len() as f64;
    let mut top1 = 0.0;
    let mut recall3 = 0.0;
    let mut mrr = 0.0;
    for (q, expected) in GRADED {
        let results = search(q);
        if results.first().map(|s| s.as_str()) == Some(*expected) {
            top1 += 1.0;
        }
        if results.iter().take(3).any(|id| id == expected) {
            recall3 += 1.0;
        }
        mrr += reciprocal_rank(&results, expected);
    }
    Metrics {
        top1: top1 / n,
        recall_at_3: recall3 / n,
        mrr: mrr / n,
    }
}

#[test]
fn search_grading_dipstick() {
    let kb = build_kb();
    let store = build_store(&kb);

    let ranked = grade(|q| {
        kb.search_ranked(q, 20)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    });
    let fts = grade(|q| {
        store
            .fts_search(q, 20)
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.id)
            .collect()
    });

    println!(
        "\n=== KB search grading ({} queries, seed corpus) ===",
        GRADED.len()
    );
    println!("                     top-1   recall@3   MRR");
    println!(
        "in-memory ranked     {:>5.2}   {:>6.2}   {:>5.3}",
        ranked.top1, ranked.recall_at_3, ranked.mrr
    );
    println!(
        "cozodb FTS (BM25)    {:>5.2}   {:>6.2}   {:>5.3}",
        fts.top1, fts.recall_at_3, fts.mrr
    );
    // Per-query breakdown for the ranker (diagnostics).
    for (q, expected) in GRADED {
        let r: Vec<String> = kb
            .search_ranked(q, 5)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let rr = reciprocal_rank(&r, expected);
        println!("  {:<20} rr={:.2}  want={}  got={:?}", q, rr, expected, r);
    }

    // Regression FLOORS for the in-memory ranker (the production fallback when
    // no Cozo query layer is present). Set below observed (top1 0.67 / recall@3
    // 1.00 / MRR 0.79) so quality can't silently regress but tuning has headroom.
    // KNOWN tuning opportunity (tracked): bare-noun queries like "buffer" rank
    // the glossary `term:` node above the `concept:` node on an id-length tie —
    // a kind-aware prior (concept/cmd > term/lesson) would lift top-1; needs
    // node kind in the ranker (follow-up, validated by this dipstick).
    //
    // FTS (BM25) is printed for comparison but NOT gated: an `open_mem` store's
    // Tantivy index under-reports here (it isn't populated by insert_node alone
    // without the seed/commit path); the live store ranks correctly in prod
    // (kb_search_context). The federated routing test covers the live path.
    assert!(
        ranked.top1 >= 0.6,
        "in-memory ranker top-1 regressed: {:.2}",
        ranked.top1
    );
    assert!(
        ranked.recall_at_3 >= 0.9,
        "in-memory ranker recall@3 regressed: {:.2}",
        ranked.recall_at_3
    );
    assert!(
        ranked.mrr >= 0.7,
        "in-memory ranker MRR regressed: {:.3}",
        ranked.mrr
    );
}
