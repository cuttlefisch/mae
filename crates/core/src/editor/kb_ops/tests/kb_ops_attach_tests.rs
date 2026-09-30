//! `:kb-attach` is verified on CONTENT and refuses by default (#825).
//!
//! Every fixture registers a real `.org` directory through real ingest and
//! detaches it through the real setter, so the store bodies under comparison
//! are exactly what ingest writes — which is what made the first comparison
//! flag 143 of 151 nodes on a real corpus in which three had changed. A test
//! built from hand-made store rows would never have seen that.

use super::*;
use mae_kb::federation::IngestPolicy;
use mae_kb::KbStore;

const NAME: &str = "Attaching";

fn note(id: &str, title: &str, prose: &str) -> String {
    format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {title}\n#+filetags: :test:\n\n{prose}\n")
}

/// A daily with real content — the shape the store holds and a stub replaces.
fn real_daily(date: &str) -> String {
    note(
        &format!("daily:{date}"),
        date,
        "Previous: [[daily:2026-07-07][2026-07-07]]\n\n* Meeting notes\nDecided to move the \
         pipeline runner to the applications host after measuring its idle load.\n\
         * Follow-ups\n- write the ADR\n- tell the team",
    )
}

/// The hollow stub mae's chain-fill wrote: same id, a title and a link.
fn stub_daily(date: &str) -> String {
    note(
        &format!("daily:{date}"),
        date,
        "Next: [[daily:2026-07-09][2026-07-09]]",
    )
}

/// Registered from `dir` (real ingest), then detached (real setter).
fn detached(dir: &std::path::Path, files: &[(&str, String)]) -> (Editor, TempDir) {
    for (name, content) in files {
        std::fs::write(dir.join(name), content).unwrap();
    }
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    // Name-derived store paths, as in production — the re-register test's
    // "store untouched" oracle only means something when a second KB of the
    // same name WOULD land on the same store.
    with_production_kb_layout(&mut editor);
    editor
        .kb_register(NAME, dir)
        .expect("registration imports the directory");
    editor
        .kb_set_ingest_policy(NAME, IngestPolicy::StoreIsTruth)
        .expect("detach");
    (editor, dirs)
}

fn uuid(editor: &Editor) -> String {
    editor.kb.registry.find(NAME).unwrap().uuid.clone()
}

fn store_body(editor: &Editor, id: &str) -> Option<String> {
    let store = editor.kb.instance_stores.get(&uuid(editor))?;
    store.get_node(id).ok().flatten().map(|n| n.body)
}

fn policy(editor: &Editor) -> IngestPolicy {
    editor.kb.registry.find(NAME).unwrap().ingest_policy
}

/// The false-positive guard: a directory nobody touched must attach without
/// `confirm`. If normalization is wrong this refuses everything, and a check
/// that refuses everything gets confirmed past reflexively.
#[test]
fn an_untouched_directory_reattaches_without_confirm() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(
        dir.path(),
        &[
            (
                "a.org",
                note("note-a", "A", "Some prose.\n\n* Heading\nMore prose."),
            ),
            ("2026-07-08.org", real_daily("2026-07-08")),
        ],
    );

    let msg = editor.kb_attach(NAME, false).expect("nothing diverges");
    assert_eq!(policy(&editor), IngestPolicy::FromOrgDir, "{msg}");
}

/// The #825 scenario: stubs carrying the SAME ids as real notes. An id-presence
/// check calls this a match; it is the destruction of the note.
#[test]
fn stubs_carrying_the_real_ids_are_refused_and_the_store_is_untouched() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(dir.path(), &[("2026-07-08.org", real_daily("2026-07-08"))]);
    let before = store_body(&editor, "daily:2026-07-08").expect("ingested");
    std::fs::write(dir.path().join("2026-07-08.org"), stub_daily("2026-07-08")).unwrap();

    let err = editor
        .kb_attach(NAME, false)
        .expect_err("a stub over a real note must refuse");

    assert!(err.contains("OVERWRITTEN"), "{err}");
    assert!(
        err.contains("likely stubs"),
        "the refusal must name the stub shape: {err}"
    );
    assert!(
        err.contains("confirm"),
        "and say how to proceed deliberately: {err}"
    );
    assert_eq!(
        policy(&editor),
        IngestPolicy::StoreIsTruth,
        "refusal changes nothing"
    );
    assert_eq!(store_body(&editor, "daily:2026-07-08"), Some(before));
}

#[test]
fn confirm_proceeds_over_a_difference() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(dir.path(), &[("2026-07-08.org", real_daily("2026-07-08"))]);
    std::fs::write(dir.path().join("2026-07-08.org"), stub_daily("2026-07-08")).unwrap();

    let msg = editor.kb_attach(NAME, true).expect("confirmed");
    assert_eq!(policy(&editor), IngestPolicy::FromOrgDir);
    assert!(
        msg.contains("Confirmed over"),
        "the override is stated, not silent: {msg}"
    );
}

/// A node that exists only in the store (created after detach) has no file to
/// represent it once the directory is authoritative.
#[test]
fn a_node_only_the_store_holds_blocks_attach() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(dir.path(), &[("a.org", note("note-a", "A", "Prose."))]);
    editor
        .kb_create_node_in(
            Some(uuid(&editor)),
            "note-made-in-mae",
            "Made in mae",
            "Written after the KB was detached.",
            mae_kb::NodeKind::Note,
        )
        .unwrap();

    let err = editor.kb_attach(NAME, false).expect_err("store-only node");
    assert!(err.contains("exist only in the store"), "{err}");
    assert!(err.contains("note-made-in-mae"), "{err}");
}

/// Additions destroy nothing, so they are reported but do not block.
#[test]
fn a_file_only_node_does_not_block_attach() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(dir.path(), &[("a.org", note("note-a", "A", "Prose."))]);
    std::fs::write(dir.path().join("b.org"), note("note-b", "B", "New in git.")).unwrap();

    editor
        .kb_attach(NAME, false)
        .expect("an addition is not a loss");
    assert_eq!(policy(&editor), IngestPolicy::FromOrgDir);
}

/// A real prose edit in a file must count, even though the header differs
/// between store and parse (the normalization must not swallow content).
#[test]
fn a_prose_edit_counts_as_a_difference() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(
        dir.path(),
        &[("a.org", note("note-a", "A", "Original sentence."))],
    );
    std::fs::write(
        dir.path().join("a.org"),
        note("note-a", "A", "Edited sentence."),
    )
    .unwrap();
    assert!(editor.kb_attach(NAME, false).is_err());
}

/// A shape reached in practice: retired (native, `org_dir` cleared), then git put the
/// files back. Re-attach goes to the recorded origin and restores `org_dir` —
/// a transition ADR-110 as written did not have.
#[test]
fn a_retired_kb_reattaches_to_its_recorded_origin() {
    let dir = TempDir::new().unwrap();
    let files = [("a.org", note("note-a", "A", "Prose."))];
    let (mut editor, _d) = detached(dir.path(), &files);
    editor.kb_retire_archive(NAME).expect("retire");
    assert!(
        editor
            .kb
            .registry
            .find(NAME)
            .unwrap()
            .org_dir
            .as_os_str()
            .is_empty(),
        "premise: native"
    );
    for (name, content) in &files {
        std::fs::write(dir.path().join(name), content).unwrap(); // "git restored it"
    }

    editor
        .kb_attach(NAME, false)
        .expect("origin matches the store");

    let inst = editor.kb.registry.find(NAME).unwrap();
    assert_eq!(inst.ingest_policy, IngestPolicy::FromOrgDir);
    assert_eq!(
        inst.org_dir.canonicalize().unwrap(),
        dir.path().canonicalize().unwrap(),
        "org_dir is restored to the origin, or ingest would have nothing to read"
    );
}

/// Retired and NOT restored: the origin is empty, so every node is store-only.
#[test]
fn a_retired_kb_whose_origin_is_empty_is_refused() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(dir.path(), &[("a.org", note("note-a", "A", "Prose."))]);
    editor.kb_retire_archive(NAME).expect("retire");

    let err = editor.kb_attach(NAME, false).expect_err("nothing on disk");
    assert!(err.contains("exist only in the store"), "{err}");
}

#[test]
fn a_native_kb_with_no_origin_is_refused() {
    let mut editor = Editor::new();
    let _d = with_test_dirs(&mut editor);
    editor.kb_new("BornNative").unwrap();
    editor
        .kb_set_ingest_policy("BornNative", IngestPolicy::StoreIsTruth)
        .ok();
    let err = editor
        .kb_attach("BornNative", true)
        .expect_err("no directory");
    assert!(err.contains("no recorded origin"), "{err}");
}

/// The second overwrite path: registering a retired KB's NAME again used to
/// append a `FromOrgDir` row over the SAME live store and import into it.
#[test]
fn re_registering_a_retired_kb_by_name_is_refused_and_the_store_is_untouched() {
    let dir = TempDir::new().unwrap();
    let (mut editor, _d) = detached(dir.path(), &[("2026-07-08.org", real_daily("2026-07-08"))]);
    editor.kb_retire_archive(NAME).expect("retire");
    let before = store_body(&editor, "daily:2026-07-08").expect("in store");
    let rows_before = editor.kb.registry.instances.len();
    let other = TempDir::new().unwrap();
    std::fs::write(other.path().join("x.org"), stub_daily("2026-07-08")).unwrap();

    assert!(
        editor.kb_register(NAME, other.path()).is_none(),
        "must refuse"
    );
    // The slug, not the exact spelling, decides the store directory.
    assert!(editor
        .kb_register(&NAME.to_lowercase(), other.path())
        .is_none());

    assert_eq!(
        editor.kb.registry.instances.len(),
        rows_before,
        "no row appended"
    );
    assert_eq!(store_body(&editor, "daily:2026-07-08"), Some(before));
}

#[test]
fn content_of_strips_only_the_leading_header() {
    use super::super::attach::content_of;
    let stored = ":PROPERTIES:\n:ID: x\n:last-accessed: 2026-08-10\n:END:\n#+title: T\n#+filetags: :a:\n\nBody.\n\n* H\n#+begin_src sh\nls\n#+end_src\n";
    assert_eq!(
        content_of(stored),
        "Body.\n\n* H\n#+begin_src sh\nls\n#+end_src",
        "a `#+` line INSIDE the content (a src block) is content, not header"
    );
    assert_eq!(content_of("Body."), "Body.");
    assert_eq!(
        content_of(":PROPERTIES:\n:ID: x\n"),
        ":PROPERTIES:\n:ID: x",
        "unterminated drawer: keep"
    );
}
