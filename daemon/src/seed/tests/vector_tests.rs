//! The shared `org-literal-v1` cross-check vectors (`tests/data/`), which a
//! second implementation of the contract checks itself against too. Copied
//! verbatim; the hash pins the copy.

use serde_json::{json, Value};

use super::apply_tests::{fixture_repo, get, seeded, KB};
use crate::seed::*;

const VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/org-literal-v1.json"
));
const VECTORS_SHA256: &str = "609b3baa41b2c81db157116b49a26ce4ed3d462b49732957ea546ea2119d3abc";

fn vectors() -> Value {
    assert_eq!(
        crate::seed::plan::sha256_hex(VECTORS.as_bytes()),
        VECTORS_SHA256,
        "the vector file is not the pinned copy"
    );
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(v["formato"], json!(CONTRACT));
    v
}

fn list(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap()
}

/// The typed fields of a split, in the vector file's shape.
fn campos(f: &SplitFile) -> Value {
    let props: serde_json::Map<String, Value> = f
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    json!({
        "properties": props,
        "property_order": f.properties.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        "title": f.title.clone().unwrap_or_default(),
        "has_title_line": f.title.is_some(),
        "tags": f.filetags.clone().unwrap_or_default(),
        "has_filetags_line": f.filetags.is_some(),
    })
}

#[test]
fn every_vector_splits_and_normalises_as_recorded() {
    let v = vectors();
    assert_eq!(list(&v["vectores"]).len(), 12);
    for vec in list(&v["vectores"]) {
        let name = vec["nombre"].as_str().unwrap();
        let f = vec["f"].as_str().unwrap();
        let n = vec["n"].as_str().unwrap();
        let split = split(f).unwrap_or_else(|p| panic!("{name}: {p:?}"));
        assert_eq!(campos(&split), vec["campos"], "{name}");
        assert_eq!(json!(split.body), vec["body"], "{name}");
        assert_eq!(normalise(f).unwrap(), n, "{name}");
        assert_eq!(vec["canonico"], json!(f == n), "{name}");
    }
}

/// Each vector seeded from a real repository: `kb/query.get` gives its fields
/// and body, and export gives back `n`.
#[tokio::test]
async fn every_vector_seeds_reads_and_exports_as_recorded() {
    let v = vectors();
    let files: Vec<(String, String)> = list(&v["vectores"])
        .iter()
        .enumerate()
        .map(|(i, x)| (format!("v{i:02}.org"), x["f"].as_str().unwrap().to_string()))
        .collect();
    let refs: Vec<(&str, String)> = files.iter().map(|(p, t)| (p.as_str(), t.clone())).collect();
    let (store, owner, plan) = seeded(&fixture_repo(&refs)).await;
    assert_eq!(plan.nodes.len(), 12);
    let dir = tempfile::tempdir().unwrap();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    for (vec, (path, _)) in list(&v["vectores"]).iter().zip(&files) {
        let name = vec["nombre"].as_str().unwrap();
        let written = std::fs::read_to_string(dir.path().join(path)).unwrap();
        assert_eq!(written, vec["n"].as_str().unwrap(), "{name}");
        let id = vec["campos"]["properties"]["ID"].as_str().unwrap();
        let g = get(&store, &owner, id).await;
        for key in ["properties", "property_order", "title", "tags"] {
            assert_eq!(g[key], vec["campos"][key], "{name}: {key}");
        }
        for key in ["has_title_line", "has_filetags_line"] {
            assert_eq!(g["org_header"][key], vec["campos"][key], "{name}: {key}");
        }
        assert_eq!(g["body"], vec["body"], "{name}");
    }
}

#[test]
fn every_refusal_is_refused_with_its_kind() {
    let v = vectors();
    assert_eq!(list(&v["rechazos"]).len(), 1);
    for r in list(&v["rechazos"]) {
        let repo = fixture_repo(&[("r.org", r["f"].as_str().unwrap().to_string())]);
        let plan = repo.plan("v1");
        let kinds: Vec<&str> = plan.refusals.iter().map(|x| x.kind.as_str()).collect();
        assert!(
            kinds.contains(&r["motivo"].as_str().unwrap()),
            "{}: {kinds:?}",
            r["nombre"]
        );
        assert_eq!(plan.plan_hash(), None);
    }
}
