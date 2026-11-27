//! `kb/query.search` against a real seeded KB, and the fixture a browser
//! client builds against: `docs/verification/fixtures/kb-search/query_search.json`,
//! every case an answer this daemon gave (written with `MAE_BLESS_FIXTURES`
//! set), compared whole.

use std::path::Path;

use mae_mcp::identity::Identity;
use serde_json::{json, Value};

use super::apply_tests::{fixture_repo, seeded, KB};
use crate::kb_query::{self, KbQueryLimits};

const ACCENT: &str = "0c000000-0000-4000-8000-000000000001";
const NFD: &str = "0c000000-0000-4000-8000-000000000002";
const SHARP_S: &str = "0c000000-0000-4000-8000-000000000003";
const PLAIN: &str = "0c000000-0000-4000-8000-000000000004";
/// The highest id: a scan capped at two nodes never reaches it.
const LONG: &str = "0c000000-0000-4000-8000-000000000005";

fn page(id: &str, title: &str, body: &str) -> String {
    format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {title}\n{body}")
}

fn corpus() -> Vec<(&'static str, String)> {
    let filler = "Relleno. ".repeat(30); // 270 characters before the match
    vec![
        (
            "a.org",
            page(ACCENT, "Árbol de decisiones", "Raíz y ramas.\n"),
        ),
        // `a` + U+0301: the decomposed spelling macOS writes.
        (
            "b.org",
            page(NFD, "Notas", "Un a\u{301}rbol genealógico.\n"),
        ),
        ("c.org", page(SHARP_S, "Straße", "Ein Weg.\n")),
        ("d.org", page(PLAIN, "Plain", "Un arbol sin tilde.\n")),
        (
            "e.org",
            page(LONG, "Long", &format!("{filler}needle-word al final.\n")),
        ),
    ]
}

/// A `kb/query.search` call by `who`, as `{result}` or `{error: {kind}}`.
async fn search(
    store: &crate::doc_store::DocStore,
    who: &str,
    params: Value,
    limits: KbQueryLimits,
) -> Value {
    match kb_query::dispatch("kb/query.search", &params, store, Some(who), limits).await {
        Ok(result) => json!({ "result": result }),
        Err(e) => {
            let data = e.data.as_deref().cloned().unwrap_or(Value::Null);
            json!({ "error": { "kind": data["kind"] } })
        }
    }
}

fn ids(answer: &Value) -> Vec<&str> {
    answer["result"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn search_matches_by_canonical_caseless_key_and_the_fixture_holds() {
    let (store, owner, _plan) = seeded(&fixture_repo(&corpus())).await;
    let me = owner.fingerprint();
    let d = KbQueryLimits::default;
    let capped = KbQueryLimits {
        max_scan_nodes: 2,
        ..d()
    };
    let q = |query: &str| json!({ "kb_id": KB, "query": query });
    let mut cases: Vec<(&str, Value, KbQueryLimits, Value)> = Vec::new();
    for (name, params, limits) in [
        (
            "composed query, composed and decomposed text",
            q("árbol"),
            d(),
        ),
        ("case folds", q("ÁRBOL"), d()),
        ("accents count", q("arbol"), d()),
        ("full case folding", q("STRASSE"), d()),
        ("match past the excerpt", q("needle-word"), d()),
        (
            "limit",
            json!({ "kb_id": KB, "query": "árbol", "limit": 1 }),
            d(),
        ),
        ("scan cap reached", q("needle-word"), capped),
        ("missing query", json!({ "kb_id": KB }), d()),
    ] {
        let answer = search(&store, &me, params.clone(), limits).await;
        cases.push((name, params, limits, answer));
    }

    // The decomposed page is stored decomposed, or its case tests nothing.
    let stored = super::apply_tests::get(&store, &owner, NFD).await;
    assert!(
        stored["body"].as_str().unwrap().contains("a\u{301}"),
        "{stored}"
    );

    // The rules, asserted on the answers before they become the fixture.
    let at = |i: usize| &cases[i].3;
    assert_eq!(ids(at(0)), [ACCENT, NFD], "NFC query, NFC and NFD text");
    assert_eq!(at(1)["result"], at(0)["result"], "case-insensitive");
    assert_eq!(ids(at(2)), [PLAIN], "accent-sensitive");
    assert_eq!(ids(at(3)), [SHARP_S], "ß folds to ss");
    let long = &at(4)["result"]["results"][0];
    assert_eq!(long["id"], LONG);
    let excerpt = long["excerpt"].as_str().unwrap();
    assert_eq!(
        excerpt.chars().count(),
        200,
        "the body's first 200 characters"
    );
    assert!(!excerpt.contains("needle-word"), "not the match's context");
    assert_eq!(ids(at(5)), [ACCENT], "the first by node id");
    assert_eq!(
        at(5)["result"]["truncated"],
        false,
        "the limit, not the cap"
    );
    let cap = &at(6)["result"];
    assert_eq!(ids(at(6)), Vec::<&str>::new());
    assert_eq!(
        (cap["scanned"].as_u64(), cap["total"].as_u64()),
        (Some(2), Some(5))
    );
    assert_eq!(cap["truncated"], true, "the cap ended the scan");
    assert_eq!(at(7)["error"]["kind"], "invalid_params");

    // The same search answers the same, request after request.
    for _ in 0..5 {
        assert_eq!(&search(&store, &me, q("árbol"), d()).await, at(0));
    }

    // A non-member learns nothing: a refusal naming no node of the KB.
    let stranger = Identity::generate("stranger").fingerprint();
    let refused = search(&store, &stranger, q("árbol"), d()).await;
    let text = refused.to_string();
    for id in [ACCENT, NFD, SHARP_S, PLAIN, LONG] {
        assert!(!text.contains(id), "{text}");
    }
    assert!(refused.get("error").is_some(), "{refused}");

    let fixture = json!({
        "method": "kb/query.search",
        "rules": [
            "A node matches when its title or its body contains the query, both compared by Unicode canonical caseless matching (NFD, full case folding, NFC; no locale): case and normalization do not count, accents do.",
            "Results are in node-id order, not ranked.",
            "excerpt is the body's first 200 characters, not the text around the match.",
            "limit defaults to and is clamped to the operator's max_search_results, and to at least 1.",
            "The scan examines at most max_scan_nodes nodes, in node-id order. truncated is true only when that cap, not limit, ended the scan; scanned and total say how far it got.",
            "An E2E-encrypted KB is refused with kind e2e_unsupported before any node is read.",
            "A caller who cannot read the KB is refused, and the refusal names no node.",
        ],
        "cases": cases.iter().map(|(name, params, limits, answer)| {
            let mut c = json!({ "name": name, "params": params });
            if limits.max_scan_nodes != d().max_scan_nodes {
                c["max_scan_nodes"] = json!(limits.max_scan_nodes);
            }
            c.as_object_mut().unwrap().extend(answer.as_object().unwrap().clone());
            c
        }).collect::<Vec<_>>(),
        "refused_non_member": refused,
    });
    check_fixture(&fixture);
}

fn check_fixture(got: &Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/verification/fixtures/kb-search/query_search.json");
    if std::env::var_os("MAE_BLESS_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(got).unwrap() + "\n").unwrap();
    }
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        &fixture,
        got,
        "regenerate {} with MAE_BLESS_FIXTURES=1",
        path.display()
    );
}
