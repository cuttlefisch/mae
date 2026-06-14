//! Graded accuracy "dipstick" for MAE knowledge-base search.
//!
//! Numerical/statistical validation of search quality over a deterministic
//! corpus (the code-generated seed KB — concept/term/cmd/scheme/option nodes;
//! no filesystem/org dependence, so it's stable in CI). Reports top-1 accuracy,
//! recall@{3,5,10}, MRR, and nDCG@10 for BOTH the in-memory relevance ranker
//! (`KnowledgeBase::search_ranked`) and the CozoDB Tantivy FTS
//! (`KbStore::search`), and asserts regression FLOORS so quality can't silently
//! degrade. Companion `kb_search_perf.rs` covers latency/scale. Run the
//! comparison table with:
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
    // Broader coverage: scheme API, ex-command phrases, multi-word concepts.
    ("save and quit", "cmd:save-and-quit"),
    ("split vertical", "cmd:split-vertical"),
    ("collab architecture", "concept:collab-architecture"),
    ("display policy", "concept:display-policy"),
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

/// nDCG with a single binary-relevant document at `expected`. With one
/// relevant doc the ideal DCG is 1.0, so nDCG = 1/log2(rank+1) (rank 1-based),
/// 0 if absent. Reported alongside MRR because nDCG discounts deep hits more
/// sharply — a useful second lens on ranking quality.
fn ndcg(results: &[String], expected: &str) -> f64 {
    results
        .iter()
        .position(|id| id == expected)
        .map(|p| 1.0 / ((p as f64 + 2.0).log2()))
        .unwrap_or(0.0)
}

struct Metrics {
    top1: f64,
    recall_at_3: f64,
    recall_at_5: f64,
    recall_at_10: f64,
    mrr: f64,
    ndcg: f64,
}

fn grade<F: Fn(&str) -> Vec<String>>(search: F) -> Metrics {
    let n = GRADED.len() as f64;
    let mut top1 = 0.0;
    let mut recall3 = 0.0;
    let mut recall5 = 0.0;
    let mut recall10 = 0.0;
    let mut mrr = 0.0;
    let mut ndcg_sum = 0.0;
    for (q, expected) in GRADED {
        let results = search(q);
        if results.first().map(|s| s.as_str()) == Some(*expected) {
            top1 += 1.0;
        }
        if results.iter().take(3).any(|id| id == expected) {
            recall3 += 1.0;
        }
        if results.iter().take(5).any(|id| id == expected) {
            recall5 += 1.0;
        }
        if results.iter().take(10).any(|id| id == expected) {
            recall10 += 1.0;
        }
        mrr += reciprocal_rank(&results, expected);
        ndcg_sum += ndcg(&results, expected);
    }
    Metrics {
        top1: top1 / n,
        recall_at_3: recall3 / n,
        recall_at_5: recall5 / n,
        recall_at_10: recall10 / n,
        mrr: mrr / n,
        ndcg: ndcg_sum / n,
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
    println!("                     top-1   r@3    r@5    r@10   MRR     nDCG");
    println!(
        "in-memory ranked     {:>5.2}  {:>5.2}  {:>5.2}  {:>5.2}  {:>5.3}  {:>5.3}",
        ranked.top1,
        ranked.recall_at_3,
        ranked.recall_at_5,
        ranked.recall_at_10,
        ranked.mrr,
        ranked.ndcg
    );
    println!(
        "cozodb FTS (BM25)    {:>5.2}  {:>5.2}  {:>5.2}  {:>5.2}  {:>5.3}  {:>5.3}",
        fts.top1, fts.recall_at_3, fts.recall_at_5, fts.recall_at_10, fts.mrr, fts.ndcg
    );
    // Per-query breakdown for the ranker (diagnostics).
    for (q, expected) in GRADED {
        let r: Vec<String> = kb
            .search_ranked(q, 5)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let rr = reciprocal_rank(&r, expected);
        let f: Vec<String> = store
            .fts_search(q, 5)
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.id)
            .collect();
        println!(
            "  {:<20} rr={:.2}  want={}\n      ranked={:?}\n      fts   ={:?}",
            q, rr, expected, r, f
        );
    }

    // Regression FLOORS for the in-memory ranker (the production lexical path;
    // see kb_federated_search). After tuning (local-id matching + namespace
    // prior + whole-query phrase bonus) the observed scores on this set are
    // top1 1.00 / recall@3 1.00 / MRR 1.00. Floors are set with margin so the
    // graded set can grow with genuinely-harder/ambiguous queries (which may
    // legitimately lower the numbers) without false CI failures, while still
    // catching real regressions.
    //
    // CAVEAT (honesty): this is a small, hand-authored set, so 1.00 reflects the
    // ranker handling these canonical-lookup patterns — NOT a claim of perfect
    // search. Grow GRADED with harder cases over time; the value here is the
    // measurement framework + regression guard + the in-memory-vs-FTS contrast.
    //
    // FINDING (kept as a gated contrast, not a bug): CozoDB FTS/BM25 ranks by
    // term frequency across title+body, which BURIES the canonical short-title
    // node (e.g. "save" -> category/autosave, never cmd:save). The field-
    // weighted ranker is decisively better for canonical lookup — which is why
    // kb_federated_search routes to it, not FTS (revises plan assumption D1).
    // FTS remains the right tool for body/RAG recall (kb_search_context).
    assert!(
        ranked.top1 >= 0.85,
        "in-memory ranker top-1 regressed: {:.2}",
        ranked.top1
    );
    assert!(
        ranked.recall_at_3 >= 0.95,
        "in-memory ranker recall@3 regressed: {:.2}",
        ranked.recall_at_3
    );
    assert!(
        ranked.mrr >= 0.9,
        "in-memory ranker MRR regressed: {:.3}",
        ranked.mrr
    );
    assert!(
        ranked.ndcg >= 0.9,
        "in-memory ranker nDCG regressed: {:.3}",
        ranked.ndcg
    );
    // FTS is decisively worse for canonical lookup here — assert the contrast so
    // the routing decision (ranker, not FTS) stays justified by data.
    assert!(
        ranked.mrr > fts.mrr,
        "field-weighted ranker should beat raw BM25 for canonical lookup (ranker {:.3} vs fts {:.3})",
        ranked.mrr,
        fts.mrr
    );
}
