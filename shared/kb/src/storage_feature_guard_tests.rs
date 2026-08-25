//! Every workspace member must request the same `mae-kb` storage features.
//!
//! ADR-108's Verification item 1. This is the check that would have caught the
//! binary's *accidental* sqlite: `crates/mae` declared `storage-sled` only, and
//! received sqlite purely through Cargo's feature unification with
//! `crates/core` — so its own default option (`kb_storage_engine = "sqlite"`)
//! depended on a feature it did not declare. Nothing failed, and nothing would
//! have warned if `crates/core` had stopped requesting it.
//!
//! `crates/ai` was the sharper case: `guidance.rs` calls
//! `open_with_engine(.., "sqlite")` directly while declaring only sled.

use std::path::{Path, PathBuf};

/// Repo root, from this file's location — `CARGO_MANIFEST_DIR` is `shared/kb`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("shared/kb has a grandparent")
        .to_path_buf()
}

/// The `features = [...]` list on a manifest's `mae-kb` dependency line, if any.
fn declared_storage_features(manifest: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("mae-kb =") && l.contains("features"))?;
    Some(
        line.split(['[', ']'])
            .nth(1)?
            .split(',')
            .map(|f| f.trim().trim_matches('"').to_string())
            .filter(|f| f.starts_with("storage-"))
            .collect(),
    )
}

/// Every crate that depends on `mae-kb` with an explicit feature list must ask
/// for the same storage backends — no crate may rely on another's request.
#[test]
fn every_consumer_declares_the_same_storage_features() {
    let root = repo_root();
    let mut seen: Vec<(String, Vec<String>)> = Vec::new();

    for rel in [
        "crates/ai/Cargo.toml",
        "crates/core/Cargo.toml",
        "crates/mae/Cargo.toml",
        "crates/scheme/Cargo.toml",
    ] {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let mut feats = declared_storage_features(&path)
            .unwrap_or_else(|| panic!("{rel}: no `mae-kb = {{ .. features = [..] }}` line found"));
        feats.sort();
        seen.push((rel.to_string(), feats));
    }
    assert!(!seen.is_empty(), "found no mae-kb consumers to check");

    let (first_name, first) = &seen[0];
    for (name, feats) in &seen[1..] {
        assert_eq!(
            feats, first,
            "{name} requests {feats:?} but {first_name} requests {first:?} — a crate that \
             relies on another's feature request works only by Cargo unification, and breaks \
             silently the moment that other crate changes (ADR-108 Verification 1)"
        );
    }

    // And the set must actually contain the backend the product defaults to.
    assert!(
        first.iter().any(|f| f == "storage-sqlite"),
        "no consumer declares storage-sqlite, but `kb_storage_engine` defaults to sqlite: {first:?}"
    );
}

/// sled must remain *available* — the one-time migration reads old stores — but
/// nothing may default to it any more.
#[test]
fn sled_is_retained_for_migration_but_is_no_longer_a_default() {
    let root = repo_root();
    let kb_manifest = std::fs::read_to_string(root.join("shared/kb/Cargo.toml")).unwrap();
    assert!(
        kb_manifest.contains(r#"default = ["storage-sqlite"]"#),
        "mae-kb's default feature must be sqlite (ADR-108 D2)"
    );
    assert!(
        kb_manifest.contains("storage-sled ="),
        "storage-sled must stay compiled so `migrate_sled_to_sqlite` can read an old store"
    );
}
