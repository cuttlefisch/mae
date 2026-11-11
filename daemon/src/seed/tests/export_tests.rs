//! Exporting a seeded KB: the round trip export(seed(F)) == N(F), the header
//! order rules, the self-check, and the target directory's safety.

use std::collections::HashMap;
use std::path::Path;

use mae_sync::kb::KbNodeDoc;

use super::apply_tests::{fixture_files, fixture_repo, seeded, KB};
use crate::doc_editors::Attribution;
use crate::doc_store::DocStore;
use crate::seed::*;

fn out_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

async fn edit(store: &DocStore, id: &str, f: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>) {
    let name = mae_sync::kb_node_doc_name(KB, id);
    let mut doc = KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
    let update = f(&mut doc);
    store
        .apply_update(&name, &update, None, Attribution::Principal("editor-1"))
        .await
        .unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

/// For every file: export(seed(F)) == N(F); for a canonical file, == F byte
/// for byte. Kills: losing the order, a line flag, or a byte of the body.
#[tokio::test]
async fn export_of_a_seed_is_the_normalised_file() {
    let files = fixture_files();
    let r = fixture_repo(&files);
    let (store, _, plan) = seeded(&r).await;
    let dir = out_dir();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert_eq!(report.written.len(), files.len());
    let mut canonical = 0;
    for (path, text) in &files {
        let n = normalise(text).unwrap();
        assert_eq!(read(dir.path(), path), n, "{path}");
        if n == *text {
            canonical += 1;
            assert_eq!(read(dir.path(), path).as_bytes(), text.as_bytes());
        }
    }
    assert_eq!(
        canonical,
        files.len() - 1,
        "the fixture has one non-canonical file"
    );
    assert_eq!(plan.nodes.iter().filter(|n| !n.canonical).count(), 1);
}

/// A property added after the seed goes after the ordered ones (sorted), and
/// one removed is skipped; both are reported. The result still reads back.
#[tokio::test]
async fn keys_outside_the_recorded_order_are_placed_and_reported() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    let alpha = "0a000000-0000-4000-8000-000000000001";
    edit(&store, alpha, |d| {
        let mut p = d.properties();
        p.remove("zeta");
        p.insert("NEW_B".into(), "2".into());
        p.insert("NEW_A".into(), "1".into());
        d.set_properties(&p)
    })
    .await;
    let dir = out_dir();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    let text = read(dir.path(), "notes/alpha.org");
    let drawer: Vec<&str> = text.lines().take_while(|l| *l != ":END:").collect();
    assert_eq!(
        drawer,
        [
            ":PROPERTIES:",
            &format!(":ID: {alpha}"),
            ":KIND: Concept",
            ":Alpha: first",
            ":mIxEd_Case: kept",
            ":beta:",
            ":NEW_A: 1",
            ":NEW_B: 2",
        ]
    );
    let notes = report.notes.join("\n");
    for key in ["zeta", "NEW_A", "NEW_B"] {
        assert!(notes.contains(key), "{key} must be reported: {notes}");
    }
}

/// A body that now starts with a header keyword, or a node whose `ID`
/// property no longer names it, would seed back differently: refused by name
/// and not written. Kills: writing without the read-back check.
#[tokio::test]
async fn a_node_that_would_seed_back_differently_is_refused() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    let no_title = "0a000000-0000-4000-8000-000000000002";
    edit(&store, no_title, |d| {
        d.set_body("#+title: Smuggled\nPlain body.\n")
    })
    .await;
    let unicode = "0a000000-0000-4000-8000-000000000004";
    edit(&store, unicode, |d| {
        let mut p = d.properties();
        p.insert("ID".into(), "something-else".into());
        d.set_properties(&p)
    })
    .await;
    let dir = out_dir();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    let refused: HashMap<&str, ExportRefusalKind> = report
        .refused
        .iter()
        .map(|r| (r.node_id.as_str(), r.kind))
        .collect();
    assert_eq!(
        refused.get(no_title),
        Some(&ExportRefusalKind::NotReproducible)
    );
    assert_eq!(
        refused.get(unicode),
        Some(&ExportRefusalKind::NotReproducible)
    );
    assert!(!dir.path().join("notes/no-title.org").exists());
    assert!(!dir.path().join("deep/dir/unicode.org").exists());
    assert_eq!(report.written.len(), 3);
}

/// A non-empty target is refused unless forced; forced, the KB's files are
/// written and other files are left alone.
#[tokio::test]
async fn a_non_empty_target_needs_force() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    let dir = out_dir();
    std::fs::write(dir.path().join("keep.txt"), "mine").unwrap();
    let err = export(&store, KB, dir.path(), false).await.unwrap_err();
    assert!(err.0.contains("not empty"), "{err}");
    assert!(!dir.path().join("notes").exists(), "refused before writing");
    let report = export(&store, KB, dir.path(), true).await.unwrap();
    assert!(report.refused.is_empty());
    assert_eq!(read(dir.path(), "keep.txt"), "mine");
}

/// A symlinked directory on the way, or a symlink at a file's own name, is
/// refused and never followed: nothing lands outside the target. Kills:
/// `create_dir_all` / a plain `write` in place of the checked walk.
#[tokio::test]
async fn symlinks_in_the_target_are_never_followed() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    let dir = out_dir();
    let outside = out_dir();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("deep")).unwrap();
    std::fs::create_dir(dir.path().join("notes")).unwrap();
    let victim = outside.path().join("victim.org");
    std::fs::write(&victim, "untouched").unwrap();
    std::os::unix::fs::symlink(&victim, dir.path().join("notes/alpha.org")).unwrap();
    let report = export(&store, KB, dir.path(), true).await.unwrap();
    let refused: Vec<(&str, ExportRefusalKind)> = report
        .refused
        .iter()
        .map(|r| (r.node_id.as_str(), r.kind))
        .collect();
    assert!(refused.contains(&(
        "0a000000-0000-4000-8000-000000000004",
        ExportRefusalKind::WriteFailed
    )));
    assert!(refused.contains(&(
        "0a000000-0000-4000-8000-000000000001",
        ExportRefusalKind::WriteFailed
    )));
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
}

/// A KB whose bodies are not literal org has no file to write back.
#[tokio::test]
async fn a_kb_without_the_literal_format_is_refused() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    let name = format!("kbc:{KB}");
    let mut coll =
        mae_sync::kb::KbCollectionDoc::from_bytes(&store.encode_state(&name).await.unwrap())
            .unwrap();
    let update = coll.set_body_format("");
    store
        .apply_update(&name, &update, None, Attribution::Principal("editor-1"))
        .await
        .unwrap();
    let err = export(&store, KB, out_dir().path(), false)
        .await
        .unwrap_err();
    assert!(err.0.contains("body format"), "{err}");
}
