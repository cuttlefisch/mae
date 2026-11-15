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

/// The export's own guard, for a seed record that holds colliding paths
/// (written by an older planner, or edited): every colliding node is refused,
/// and a distinct one is not. Kills: dropping the guard, or first-wins.
#[test]
fn colliding_source_paths_are_all_refused_at_export() {
    let row = |id: &str, path: &str| crate::storage::SeedNodeRow {
        node_id: id.into(),
        path: path.into(),
        blob: String::new(),
        normalised_sha256: String::new(),
        created_at: 0,
        created_source: "git".into(),
        modified_at: 0,
        modified_source: "git".into(),
        seed_wal_id: None,
    };
    let rows: HashMap<String, crate::storage::SeedNodeRow> = [
        row("a", "N/A.org"),
        row("b", "n/a.org"),
        row("c", "n/caf\u{e9}.org"),
        row("d", "n/cafe\u{301}.org"),
        row("e", "n/other.org"),
    ]
    .into_iter()
    .map(|r| (r.node_id.clone(), r))
    .collect();
    let ids: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
    let got = crate::seed::export::colliding_nodes_for_test(&ids, &rows);
    let mut refused: Vec<&str> = got.keys().map(String::as_str).collect();
    refused.sort();
    assert_eq!(refused, ["a", "b", "c", "d"]);
}

/// `--force` over an existing file replaces it through a temporary name and
/// leaves no temporary file behind.
#[tokio::test]
async fn force_replaces_files_without_leaving_temporaries() {
    let files = fixture_files();
    let r = fixture_repo(&files);
    let (store, _, _) = seeded(&r).await;
    let dir = out_dir();
    export(&store, KB, dir.path(), false).await.unwrap();
    let again = export(&store, KB, dir.path(), true).await.unwrap();
    assert!(again.refused.is_empty(), "{:?}", again.refused);
    let mut stack = vec![dir.path().to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            assert!(
                !p.to_string_lossy().contains("mae-export-tmp"),
                "left behind: {}",
                p.display()
            );
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    for (path, text) in &files {
        assert_eq!(read(dir.path(), path), normalise(text).unwrap(), "{path}");
    }
}

/// Two peers inserting the same key concurrently can leave it in
/// `property_order` twice (ADR-093 Amendment 1 A3). A reader counts it once, at
/// its first position, and the file still seeds back (A4). Kills: writing each
/// listed key as it comes, which repeats the drawer line and fails the re-split.
#[tokio::test]
async fn a_key_listed_twice_in_the_order_is_written_once_at_its_first_place() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    let alpha = "0a000000-0000-4000-8000-000000000001";
    edit(&store, alpha, |d| {
        let mut h = d.org_header().expect("a seeded node has a header shape");
        // The fixture's order is ID, zeta, KIND, Alpha, mIxEd_Case, beta.
        assert_eq!(h.property_order[2], "KIND");
        h.property_order.push("KIND".into());
        d.set_org_header(&h)
    })
    .await;
    let dir = out_dir();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let text = read(dir.path(), "notes/alpha.org");
    let drawer: Vec<&str> = text.lines().take_while(|l| *l != ":END:").collect();
    assert_eq!(drawer.iter().filter(|l| l.starts_with(":KIND:")).count(), 1);
    assert_eq!(drawer[3], ":KIND: Concept", "kept at its first position");
}

const ALPHA: &str = "0a000000-0000-4000-8000-000000000001";

/// Seed the fixture, apply `change` to node alpha, export: alpha's refusal if
/// any, whether its file was written, and the report.
async fn export_after_editing_alpha(
    change: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>,
) -> (Option<ExportRefusal>, bool, ExportReport) {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    edit(&store, ALPHA, change).await;
    let dir = out_dir();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    let refusal = report.refused.iter().find(|r| r.node_id == ALPHA).cloned();
    let written = dir.path().join("notes/alpha.org").exists();
    (refusal, written, report)
}

fn assert_refused_naming(refusal: Option<ExportRefusal>, written: bool, field: &str) {
    let r = refusal.unwrap_or_else(|| panic!("a node with {field} must be refused"));
    assert_eq!(r.kind, ExportRefusalKind::Unrepresentable, "{r:?}");
    assert!(
        r.detail.contains(field),
        "the detail names {field}: {}",
        r.detail
    );
    assert!(!written, "a refused node's file is never written");
}

/// An alias held only in the `aliases` array has no place in the file (an
/// org-literal-v1 alias is the ROAM_ALIASES property, and the seed never fills
/// the array): the node is refused by name, not written without it. The other
/// nodes still export. Kills: export without the re-seed comparison.
#[tokio::test]
async fn an_aliases_array_refuses_the_export_naming_it() {
    let (refusal, written, report) =
        export_after_editing_alpha(|d| d.set_aliases(&["Alpha Prime".to_string()])).await;
    let detail = refusal
        .as_ref()
        .map(|r| r.detail.clone())
        .unwrap_or_default();
    assert_refused_naming(refusal, written, "aliases");
    assert!(detail.contains("ROAM_ALIASES"), "{detail}");
    assert_eq!(report.refused.len(), 1, "{:?}", report.refused);
    assert_eq!(report.written.len(), 4);
}

/// Every other field the file cannot carry refuses the same way, each named.
/// One field per seeded KB, so one field cannot hide another.
#[tokio::test]
async fn every_unrepresentable_field_refuses_the_export_naming_it() {
    type Change = Box<dyn FnOnce(&mut KbNodeDoc) -> Vec<u8>>;
    let cases: Vec<(&str, Change)> = vec![
        ("todo_state", Box::new(|d| d.set_todo_state(Some("TODO")))),
        ("priority", Box::new(|d| d.set_priority(Some("A")))),
        (
            "links",
            Box::new(|d| d.add_link("0a000000-0000-4000-8000-000000000002")),
        ),
        (
            "source_version",
            Box::new(|d| d.set_source_version(Some(3))),
        ),
    ];
    for (field, change) in cases {
        let (refusal, written, _) = export_after_editing_alpha(change).await;
        assert_refused_naming(refusal, written, field);
    }
}

/// The node's `kind` is its KIND property's value. Either one changed alone
/// leaves them disagreeing, and the file (which has only the property) would
/// seed back with the other kind: refused naming `kind`. Kills: comparing
/// properties but not the kind.
#[tokio::test]
async fn a_kind_that_disagrees_with_the_kind_property_refuses() {
    let (refusal, written, _) = export_after_editing_alpha(|d| d.set_kind(Some("Decision"))).await;
    assert_refused_naming(refusal, written, "kind");
    let (refusal, written, _) = export_after_editing_alpha(|d| {
        let mut p = d.properties();
        p.insert("KIND".into(), "Decision".into());
        d.set_properties(&p)
    })
    .await;
    assert_refused_naming(refusal, written, "kind");
}

/// A field the seed sets by definition (here provenance) is not the file's to
/// carry: a change to it alone still exports. Kills: dropping the exemption.
#[tokio::test]
async fn a_field_the_seed_sets_does_not_refuse() {
    let (refusal, written, report) =
        export_after_editing_alpha(|d| d.set_source(Some("manual"))).await;
    assert!(refusal.is_none(), "{refusal:?}");
    assert!(written);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
}

/// Edits the file can carry (title, body, tags, a property's value, a new
/// ROAM_ALIASES property placed in the recorded order, the KIND property and
/// the kind together) export with no refusal and no note, and the file is
/// canonical, so seeding and exporting it again gives the same bytes. Kills: a
/// comparison that refuses an edited node the file represents.
#[tokio::test]
async fn representable_edits_export_and_round_trip() {
    let r = fixture_repo(&fixture_files());
    let (store, _, _) = seeded(&r).await;
    edit(&store, ALPHA, |d| d.set_title("Alpha, renamed")).await;
    edit(&store, ALPHA, |d| {
        d.set_body("New body with \u{e9}.\n* Heading\nText.\n")
    })
    .await;
    edit(&store, ALPHA, |d| d.set_tags(&["y".into(), "z".into()])).await;
    edit(&store, ALPHA, |d| d.set_kind(Some("Decision"))).await;
    edit(&store, ALPHA, |d| {
        let mut p = d.properties();
        p.insert("Alpha".into(), "changed".into());
        p.insert("KIND".into(), "Decision".into());
        p.insert("ROAM_ALIASES".into(), "\"Alpha Prime\" AP".into());
        d.set_properties(&p)
    })
    .await;
    edit(&store, ALPHA, |d| {
        let mut h = d.org_header().unwrap();
        h.property_order.insert(1, "ROAM_ALIASES".into());
        d.set_org_header(&h)
    })
    .await;
    let dir = out_dir();
    let report = export(&store, KB, dir.path(), false).await.unwrap();
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let text = read(dir.path(), "notes/alpha.org");
    let expected = format!(
        ":PROPERTIES:\n:ID: {ALPHA}\n:ROAM_ALIASES: \"Alpha Prime\" AP\n:zeta: last\n\
         :KIND: Decision\n:Alpha: changed\n:mIxEd_Case: kept\n:beta:\n:END:\n\
         #+title: Alpha, renamed\n#+filetags: :y:z:\nNew body with \u{e9}.\n* Heading\nText.\n"
    );
    assert_eq!(text, expected);
    // Seed the exported file and export it again: the same bytes.
    let mut files = fixture_files();
    files[0].1 = text.clone();
    let r2 = fixture_repo(&files);
    let (store2, _, _) = seeded(&r2).await;
    let dir2 = out_dir();
    let again = export(&store2, KB, dir2.path(), false).await.unwrap();
    assert!(again.refused.is_empty(), "{:?}", again.refused);
    assert_eq!(read(dir2.path(), "notes/alpha.org"), text);
}
