//! A RETIRED KB still owns the directory it came from, for MAE's own writes.
//!
//! Found on a real machine (2026-09-29): a KB was cut over and retired, so its
//! `org_dir` was cleared, but `kb-notes-dir` still pointed at its origin. Every
//! "who owns this directory?" check matched only a non-empty `org_dir`, so the
//! answer was "nobody", dailies fell back to the primary's file-backed policy,
//! and chain-fill wrote ~100 hollow `Previous:`/`Next:` stub files into the
//! retired origin — files a later attach or re-register would ingest over the
//! real store (#825). Capture had the same blind spot.
//!
//! Every test here builds the retired state the way production reaches it —
//! detach, then `kb_retire_archive` — rather than hand-assembling registry
//! fields, so the fixture cannot drift from what retirement actually writes.

use super::super::daily::DailyBacking;
use super::*;
use mae_kb::KbStore;

const UUID: &str = "uuid-retire";

/// A KB imported from `origin`, detached, then retired: native, `org_dir`
/// cleared, `import_record.origin` = `origin`. The notes dir points at the
/// retired origin, exactly the shape found on the real machine.
fn retired_kb_with_notes_dir_at_origin(origin: &std::path::Path) -> (Editor, TempDir) {
    let (mut editor, dirs) = super::kb_ops_retire_tests::detached_kb(origin, &[("a.org", "AAA")]);
    editor
        .kb_retire_archive("Retiring")
        .expect("the fixture's archive is fully represented, so retirement succeeds");
    let inst = editor
        .kb
        .registry
        .find("Retiring")
        .expect("still registered");
    assert!(
        inst.org_dir.as_os_str().is_empty(),
        "premise: retirement clears org_dir — that is the state under test"
    );
    assert_eq!(
        inst.import_record.as_ref().map(|r| r.origin.clone()),
        Some(origin.canonicalize().unwrap()),
        "premise: retirement records where the KB came from, canonical (#832)"
    );
    // Load the in-memory mirror from the store, as `kb_adopt_detached_instance`
    // does at every startup. The shared `detached_kb` fixture only installs the
    // durable store; without the mirror, a create routed to this instance falls
    // back to an in-memory PRIMARY insert that production never takes.
    let mut mirror = mae_kb::KnowledgeBase::new();
    for node in editor.kb.instance_stores[UUID]
        .load_all()
        .expect("load store")
    {
        mirror.insert(node);
    }
    editor.kb.instances.insert(UUID.to_string(), mirror);
    editor.kb.notes_dir = Some(origin.to_path_buf());
    editor.kb.dailies_dir = None;
    (editor, dirs)
}

/// Every `.org` file under `dir`, recursively — the oracle for "wrote nothing".
fn org_files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("org") {
                out.push(p);
            }
        }
    }
    out
}

fn in_retired_store(editor: &Editor, id: &str) -> bool {
    editor
        .kb
        .instance_stores
        .get(UUID)
        .is_some_and(|s| matches!(s.get_node(id), Ok(Some(_))))
}

#[test]
fn a_daily_in_a_retired_origin_writes_no_file_and_lands_in_that_kb() {
    let origin = TempDir::new().unwrap();
    let (mut editor, _dirs) = retired_kb_with_notes_dir_at_origin(origin.path());

    assert!(
        matches!(editor.kb_daily_backing(), DailyBacking::Store),
        "the dailies dir is a retired KB's origin, so its store is the truth"
    );
    editor
        .kb_daily_ensure(2026, 9, 7)
        .expect("create the daily");

    assert_eq!(
        org_files_under(origin.path()),
        Vec::<std::path::PathBuf>::new(),
        "nothing may be written into a retired origin — a file there is a stub \
         a later attach would ingest over the store"
    );
    assert!(
        in_retired_store(&editor, "daily:2026-09-07"),
        "the daily belongs to the KB that owns the dailies dir"
    );
    assert!(
        editor.kb.primary.get("daily:2026-09-07").is_none(),
        "…and NOT to the primary store, where prefix routing used to send it"
    );
}

/// The exact failure: chain-fill across a gap. On the real machine this is what
/// produced ~90 stub files in a single burst.
#[test]
fn chain_fill_across_a_gap_writes_no_stub_files_into_a_retired_origin() {
    let origin = TempDir::new().unwrap();
    let (mut editor, _dirs) = retired_kb_with_notes_dir_at_origin(origin.path());
    editor.kb_daily_ensure(2026, 9, 1).unwrap();

    let res = editor.kb_daily_chain_fill(2026, 9, 7).expect("chain-fill");

    assert_eq!(
        org_files_under(origin.path()),
        Vec::<std::path::PathBuf>::new(),
        "chain-fill created {:?} and wrote files for them into the retired origin",
        res.stubs_created
    );
    for day in 2..=7 {
        let id = format!("daily:2026-09-{day:02}");
        assert!(
            in_retired_store(&editor, &id),
            "{id} must be in the retired KB's store"
        );
    }
    let today = editor.kb_daily_text(2026, 9, 7).unwrap_or_default();
    assert!(
        today.contains("daily:2026-09-06"),
        "the chain still links, over node bodies: {today:?}"
    );
}

#[test]
fn capture_into_a_retired_origin_writes_no_file_and_lands_in_that_kb() {
    let origin = TempDir::new().unwrap();
    let (mut editor, _dirs) = retired_kb_with_notes_dir_at_origin(origin.path());

    let (id, path) = editor
        .kb_create_note_from_title("Captured after retirement")
        .expect("capture");

    assert_eq!(path, None, "a store-backed capture has no file");
    assert_eq!(
        org_files_under(origin.path()),
        Vec::<std::path::PathBuf>::new(),
        "capture must not write into a retired origin"
    );
    assert!(
        in_retired_store(&editor, &id),
        "the captured note belongs to the KB that owns the notes dir"
    );
    assert!(editor.kb.primary.get(&id).is_none());
}

/// Ownership must not depend on how the directory is SPELLED: the real config
/// held an absolute path, but a symlink or `..` spelling is ordinary.
#[cfg(unix)]
#[test]
fn a_symlinked_spelling_of_the_retired_origin_is_still_owned() {
    let origin = TempDir::new().unwrap();
    let (mut editor, _dirs) = retired_kb_with_notes_dir_at_origin(origin.path());
    let links = TempDir::new().unwrap();
    let alias = links.path().join("notes-alias");
    std::os::unix::fs::symlink(origin.path(), &alias).unwrap();
    editor.kb.notes_dir = Some(alias.clone());

    editor.kb_daily_ensure(2026, 9, 18).unwrap();

    assert_eq!(
        org_files_under(origin.path()),
        Vec::<std::path::PathBuf>::new()
    );
    assert!(in_retired_store(&editor, "daily:2026-09-18"));
}

/// Control: an ATTACHED KB owning the notes dir keeps file-backed dailies —
/// the fix narrows only the retired case and changes nothing for everyone else.
#[test]
fn an_attached_kb_owning_the_notes_dir_still_uses_files() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _t = with_test_dirs(&mut editor);
    editor.kb_register("Attached", dir.path()).unwrap();
    editor.kb.notes_dir = Some(dir.path().to_path_buf());
    editor.kb.dailies_dir = None;

    assert!(matches!(editor.kb_daily_backing(), DailyBacking::Files(_)));
    editor.kb_daily_ensure(2026, 9, 7).unwrap();
    assert!(
        dir.path().join("daily").join("2026-09-07.org").exists(),
        "an attached KB's dailies are files, as before"
    );
}

/// The human-facing guards deliberately keep the narrower scope: a retired
/// origin may be an ordinary project repo, and refusing a person's own `.org`
/// file there is not a call this fix makes. Pinned so widening it is a
/// deliberate change, not an accident of sharing the resolver.
#[test]
fn human_file_guards_do_not_claim_a_retired_origin() {
    let origin = TempDir::new().unwrap();
    let (editor, _dirs) = retired_kb_with_notes_dir_at_origin(origin.path());
    assert_eq!(
        editor.kb_orphan_org_target(&origin.path().join("my-own-note.org")),
        None
    );
    assert_eq!(
        editor.kb_stale_archive_instance(&origin.path().join("a.org")),
        None
    );
}
