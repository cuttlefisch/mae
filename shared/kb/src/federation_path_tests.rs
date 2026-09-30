//! #832: stored KB paths have one canonical spelling, and a directory carrying
//! another KB's instance marker cannot hijack that KB's registry row.

use super::*;
use tempfile::TempDir;

fn note(id: &str) -> String {
    format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\nbody\n")
}

/// Rows written before canonicalization (or by hand) are repaired on load, so
/// every reader compares like with like.
#[cfg(unix)]
#[test]
fn a_legacy_row_spelled_through_a_symlink_is_canonical_after_load() {
    let data = TempDir::new().unwrap();
    let real = TempDir::new().unwrap();
    let links = TempDir::new().unwrap();
    let alias = links.path().join("notes");
    std::os::unix::fs::symlink(real.path(), &alias).unwrap();
    std::fs::write(
        data.path().join("kb-registry.toml"),
        format!(
            "[[instances]]\nuuid = \"u-legacy\"\nname = \"Legacy\"\norg_dir = \"{a}\"\n\
             db_path = \"/nonexistent/kb.sqlite\"\nprimary = false\nenabled = true\n\
             [instances.import_record]\norigin = \"{a}\"\n",
            a = alias.display()
        ),
    )
    .unwrap();

    let reg = KbRegistry::load(data.path());
    let inst = reg.find("Legacy").expect("row loaded");
    let want = real.path().canonicalize().unwrap();
    assert_eq!(inst.org_dir, want, "org_dir is canonical after load");
    assert_eq!(
        inst.import_record.as_ref().map(|r| r.origin.clone()),
        Some(want),
        "and so is the recorded origin"
    );
}

/// A copy of a KB directory — or a checkout of an old revision that still
/// tracks the marker — carries the KB's uuid. The original still exists, so
/// this is not a move: re-pointing the row would silently swap the KB's source
/// for the copy's content.
#[test]
fn a_copy_carrying_another_kbs_marker_is_refused_while_the_original_exists() {
    let data = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let original = home.path().join("notes");
    std::fs::create_dir_all(&original).unwrap();
    std::fs::write(original.join("a.org"), note("n:a")).unwrap();
    let mut reg = KbRegistry::default();
    let uuid = reg
        .register("Notes".into(), original.clone(), data.path(), None)
        .expect("register the original");

    let copy = home.path().join("notes-copy");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::copy(
        original.join(INSTANCE_SENTINEL),
        copy.join(INSTANCE_SENTINEL),
    )
    .unwrap();

    let err = reg
        .register("NotesCopy".into(), copy.clone(), data.path(), None)
        .expect_err("the original still exists, so this is a copy, not a move");
    assert!(
        err.contains(INSTANCE_SENTINEL),
        "the remedy names the marker: {err}"
    );
    let row = reg.find_by_uuid(&uuid).expect("row kept");
    assert_eq!(
        row.org_dir,
        original.canonicalize().unwrap(),
        "the row still points at the original"
    );
    assert_eq!(reg.instances.len(), 1, "and no second row was added");
}

/// A store-authoritative KB (native or retired) has no directory. A directory
/// that turns up carrying its marker — git restoring the origin, say — must not
/// silently become its source: that is `:kb-attach`'s job, because it compares
/// the content first (#825).
#[test]
fn a_marker_for_a_store_authoritative_kb_is_refused_and_points_at_kb_attach() {
    let data = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let mut reg = KbRegistry::default();
    let mut native = KbInstance::local(
        "u-native".into(),
        "Native".into(),
        PathBuf::new(),
        home.path().join("native.sqlite"),
    );
    native.ingest_policy = IngestPolicy::StoreIsTruth;
    reg.instances.push(native);

    let restored = home.path().join("restored");
    std::fs::create_dir_all(&restored).unwrap();
    write_sentinel(&restored, "u-native", "Native").unwrap();

    let err = reg
        .register("Restored".into(), restored, data.path(), None)
        .expect_err("a store-authoritative KB is never given a directory here");
    assert!(err.contains(":kb-attach"), "{err}");
    let row = reg.find_by_uuid("u-native").unwrap();
    assert!(row.org_dir.as_os_str().is_empty(), "the row is untouched");
}
