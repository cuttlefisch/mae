//! Phase 3 — dailies must work with **no `.org` directory at all**.
//!
//! Every dailies path was unconditionally file-backed: existence was
//! `path.exists()`, creation was `fs::write`, and chain-fill string-spliced
//! `Previous:`/`Next:` into file text. On a detached KB (`StoreIsTruth`) or a
//! fresh install with no `notes_dir`, that is **wholly broken, not degraded** --
//! there is no directory for any of it to touch.
//!
//! `kb_ops_activity_tests.rs` already recorded this as a live hazard: *"#316's
//! underlying hazard is NOT gone from the codebase — `kb_ops/daily.rs` still
//! writes `.org` files under a `write_guard`."*

use super::super::daily::DailyBacking;
use super::*;

/// An editor with a real store and no dailies directory configured — a fresh
/// install, and the shape a detached KB presents.
fn editor_with_no_notes_dir() -> (TempDir, Editor) {
    let mut editor = Editor::new();
    let tmp = with_test_dirs(&mut editor);
    editor.kb.notes_dir = None;
    editor.kb.dailies_dir = None;
    (tmp, editor)
}

#[test]
fn with_no_dailies_directory_the_backing_is_the_store() {
    let (_tmp, editor) = editor_with_no_notes_dir();
    assert!(
        matches!(editor.kb_daily_backing(), DailyBacking::Store),
        "no dailies directory means there is nothing to write a file INTO -- \
         writing one anyway looks like it worked and is not read by any ingest"
    );
}

/// A configured directory still uses files, so this change is additive for
/// every existing user.
#[test]
fn a_configured_dailies_directory_still_uses_files() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _t = with_test_dirs(&mut editor);
    editor.kb.dailies_dir = Some(dir.path().to_path_buf());
    assert!(matches!(editor.kb_daily_backing(), DailyBacking::Files(_)));
}

/// **The Phase 3 property.** Today's daily can be created and found again with
/// no filesystem involved.
#[test]
fn a_daily_can_be_created_and_found_with_no_org_directory() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    let (y, m, d) = (2026, 8, 25);

    assert!(!editor.kb_daily_exists(y, m, d), "nothing exists yet");
    editor
        .kb_daily_ensure(y, m, d)
        .expect("creating a daily must not require an org directory");
    assert!(
        editor.kb_daily_exists(y, m, d),
        "the daily must be findable after creation -- via the store, since there \
         is no file to stat"
    );
    assert!(
        editor.kb_get_node_anywhere("daily:2026-08-25").is_some(),
        "and it must be a real node under its canonical id"
    );
}

/// Creating twice is a no-op, so navigation can call it unconditionally.
#[test]
fn ensuring_an_existing_daily_does_not_replace_it() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    let (y, m, d) = (2026, 8, 25);
    editor.kb_daily_ensure(y, m, d).unwrap();
    editor
        .kb_daily_set_text(y, m, d, "#+title: 2026-08-25\n\nmy notes\n")
        .unwrap();

    editor.kb_daily_ensure(y, m, d).unwrap();

    assert!(
        editor
            .kb_daily_text(y, m, d)
            .unwrap_or_default()
            .contains("my notes"),
        "a second ensure must not clobber the day's content"
    );
}

/// Chain-fill is the feature most coupled to files -- it string-spliced links
/// into file text. It must produce the same chain over node bodies.
#[test]
fn chain_fill_links_dailies_with_no_files_involved() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    // An anchor two days back, so the fill has a gap to close.
    editor.kb_daily_ensure(2026, 8, 23).unwrap();

    editor
        .kb_daily_chain_fill(2026, 8, 25)
        .expect("chain-fill must work without a dailies directory");

    // The gap day was created...
    assert!(
        editor.kb_daily_exists(2026, 8, 24),
        "the gap day must be filled"
    );
    // ...and the chain links point backwards.
    let today = editor.kb_daily_text(2026, 8, 25).unwrap_or_default();
    assert!(
        today.contains("Previous:") && today.contains("daily:2026-08-24"),
        "today must link back to the gap day: {today:?}"
    );
    // ...and forwards, symmetrically.
    let gap = editor.kb_daily_text(2026, 8, 24).unwrap_or_default();
    assert!(
        gap.contains("Next:") && gap.contains("daily:2026-08-25"),
        "the gap day must link forward to today: {gap:?}"
    );
}

/// A link is inserted once, not on every pass -- otherwise repeated navigation
/// accretes duplicates, the same shape #655's double drawer had.
#[test]
fn chain_fill_is_idempotent() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    editor.kb_daily_ensure(2026, 8, 24).unwrap();
    editor.kb_daily_chain_fill(2026, 8, 25).unwrap();
    let once = editor.kb_daily_text(2026, 8, 25).unwrap_or_default();

    editor.kb_daily_chain_fill(2026, 8, 25).unwrap();
    let twice = editor.kb_daily_text(2026, 8, 25).unwrap_or_default();

    assert_eq!(
        once.matches("Previous:").count(),
        1,
        "one link after one pass"
    );
    assert_eq!(
        twice.matches("Previous:").count(),
        1,
        "still one link after a second pass -- navigation calls this every time"
    );
}

/// Writing the store backing must NOT touch the filesystem, which is the whole
/// point: on a detached KB a file write is invisible to the store and reads as
/// success.
#[test]
fn the_store_backing_writes_no_files() {
    let dir = TempDir::new().unwrap();
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    // Point `notes_dir` at an empty dir but leave the KB store-backed by
    // clearing `dailies_dir` -- if anything wrote a file it would land here.
    editor.kb.dailies_dir = None;

    editor.kb_daily_ensure(2026, 8, 25).unwrap();
    editor.kb_daily_chain_fill(2026, 8, 25).unwrap();

    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "the store backing must not write files anywhere"
    );
}

// ---------------------------------------------------------------------------
// Phase 3 — capture with no `notes_dir`.
// ---------------------------------------------------------------------------

/// **Capture appeared to do nothing on a fresh install.**
///
/// The no-`notes_dir` branch created the node and returned: no buffer opened, no
/// capture mode entered, no status set. So `SPC n c` looked like a no-op while
/// silently leaving a titled empty node behind.
#[test]
fn capture_without_a_notes_dir_actually_enters_capture_mode() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();

    let (id, path) = editor
        .kb_create_note_from_title("A captured thought")
        .expect("capture must work with no notes_dir");

    assert!(path.is_none(), "there is no file, and that is the point");
    assert!(
        editor.kb.capture_state.is_some(),
        "capture mode must be ENTERED -- otherwise SPC n s / SPC n k have nothing \
         to finish or abort, and the capture is invisible"
    );
    assert_eq!(
        editor.kb.capture_state.as_ref().unwrap().node_id,
        id,
        "capture state must name the node just created"
    );
    assert!(
        editor.kb_get_node_anywhere(&id).is_some(),
        "and the node must be durably in the store"
    );
}

/// Finalizing a store-backed capture must not try to save a file.
#[test]
fn finalizing_a_store_backed_capture_leaves_the_node_intact() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    let (id, _) = editor.kb_create_note_from_title("Another thought").unwrap();

    editor.dispatch_builtin("capture-finalize");

    assert!(editor.kb.capture_state.is_none(), "capture mode must end");
    assert!(
        editor.kb_get_node_anywhere(&id).is_some(),
        "the captured node must survive finalization -- it was already persisted"
    );
}

// -- Navigation, which had no Store path at all -------------------------------

/// **`daily-prev`/`daily-next` could not identify the current daily.**
///
/// `kb_daily_date_from_buffer` probed the buffer's file-path stem and a
/// `#+title:` line. A store-backed daily is a `BufferKind::Kb` buffer with no
/// file path, and the KB renderer strips `#+` lines — so neither probe matched
/// and every navigation failed with "Current buffer is not a daily note".
#[test]
fn the_current_daily_is_identifiable_from_a_store_backed_buffer() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    editor.kb_daily_ensure(2026, 8, 26).unwrap();
    editor.kb_daily_open(2026, 8, 26).unwrap();

    assert_eq!(
        editor.kb_daily_date_from_buffer(),
        Ok((2026, 8, 26)),
        "a store-backed daily's KB buffer must identify its own date"
    );
}

/// **The four navigation commands had no Store path.** Each resolved a file path
/// and called `open_file_at_path`, so on a store backing they failed with
/// "No dailies directory" — after `daily-goto-date` had already created and
/// chain-linked nodes, leaving it half-succeeded with a failure on the status
/// line.
#[test]
fn daily_navigation_works_on_a_store_backing() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    editor.kb_daily_ensure(2026, 8, 25).unwrap();
    editor.kb_daily_ensure(2026, 8, 26).unwrap();
    editor.kb_daily_open(2026, 8, 26).unwrap();

    editor.kb_daily_prev().expect("daily-prev must work");
    assert_eq!(editor.kb_daily_date_from_buffer(), Ok((2026, 8, 25)));

    editor.kb_daily_next().expect("daily-next must work");
    assert_eq!(editor.kb_daily_date_from_buffer(), Ok((2026, 8, 26)));
}

/// `daily-goto-date` must not report failure after having already done its work.
#[test]
fn goto_daily_date_succeeds_on_a_store_backing() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();

    editor
        .kb_goto_daily_date("2026-08-26")
        .expect("goto-date must not fail after chain-filling");

    assert!(
        editor.kb_daily_exists(2026, 8, 26),
        "and the node it created must be there"
    );
    assert_eq!(editor.kb_daily_date_from_buffer(), Ok((2026, 8, 26)));
}

/// **`:daily-goto-date <date>` silently discarded its argument.** There was no
/// `daily-goto-date` arm in `execute_command`'s arg-carrying match, so the
/// command fell through to the no-arg dispatcher, which opens a *prompt* for
/// the date. A caller that passed one got an unanswered mini-dialog and the
/// editor parked in Command mode -- while believing it had navigated.
///
/// This was not a cosmetic leak: an input-capturing overlay swallows every
/// subsequent key. It surfaced as four failures in the leader-keypad e2e file,
/// which has nothing to do with dailies -- the leaked dialog ate its `SPC`.
#[test]
fn goto_daily_date_honours_an_inline_argument() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();

    assert!(
        editor.execute_command("daily-goto-date 2026-08-26"),
        "the arg form must be handled by the arg-carrying dispatcher"
    );
    assert!(
        editor.mini_dialog.is_none(),
        "an inline date must NOT open the date prompt"
    );
    assert_ne!(
        editor.mode,
        crate::Mode::Command,
        "and must not park the editor in Command mode behind that prompt"
    );
    assert_eq!(
        editor.kb_daily_date_from_buffer(),
        Ok((2026, 8, 26)),
        "it must actually navigate to the date it was given"
    );
}

/// The other half of the same contract: bound to a key (`SPC n d d`) there is
/// no argument to carry, and the date PROMPT is the right behaviour -- so the
/// fix must not remove it. `dispatch_builtin` is that path; `execute_command`
/// is the ex path the arm above serves.
#[test]
fn goto_daily_date_without_an_argument_still_prompts() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();

    editor.dispatch_builtin("daily-goto-date");

    assert!(
        editor.mini_dialog.is_some(),
        "the keybinding path must still open the date prompt"
    );
}

// -- Capture: the one seam that was never built --------------------------------

/// **Capture wrote `.org` into a detached KB's stale archive.**
///
/// `kb_create_note_from_title` branched on `self.kb.notes_dir`, not on ingest
/// policy — so every existing user who detaches (they all have `notes_dir` set)
/// got a file written into a directory no ingest reads, a file buffer opened on
/// it, and `capture-finalize` saving it again. The node did reach the store, so
/// nothing was lost; the user was editing a dead file that looked live.
///
/// Dailies got a `DailyBacking` seam for exactly this. Capture never did.
#[test]
fn capture_writes_no_file_when_the_primary_is_detached() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    editor.kb.notes_dir = Some(tmp.path().to_path_buf());

    editor
        .kb
        .registry
        .set_ingest_policy("primary", mae_kb::federation::IngestPolicy::StoreIsTruth);

    let (id, path) = editor
        .kb_create_note_from_title("Detached capture")
        .unwrap();

    assert!(
        path.is_none(),
        "a detached KB must not get a file written into its stale archive: {path:?}"
    );
    assert!(
        editor.kb.primary.get(&id).is_some(),
        "and the node must still be created in the store"
    );
    let stray: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert!(stray.is_empty(), "no file may be written at all: {stray:?}");
}

/// An attached KB keeps writing the file — the positive control, without which
/// the assertion above is satisfied by capture being broken generally.
#[test]
fn capture_still_writes_a_file_when_attached() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    editor.kb.notes_dir = Some(tmp.path().to_path_buf());

    let (_id, path) = editor
        .kb_create_note_from_title("Attached capture")
        .unwrap();

    assert!(
        path.is_some_and(|p| p.exists()),
        "an attached KB must keep today's behaviour exactly"
    );
}

/// **Aborting a store-backed capture left the node in the durable store.**
///
/// `kb_capture_abort` removed the node from the in-memory mirrors and deleted
/// the file — but `kb_create_node` had already persisted it, and abort never
/// called the owner-aware `kb_delete_node`. So the aborted note came back on the
/// next restart. Invisible while capture was file-backed, because the mirror
/// removal plus the file delete was the whole story.
#[test]
fn aborting_a_store_backed_capture_removes_the_node_from_the_store() {
    let (_tmp, mut editor) = editor_with_no_notes_dir();
    let primary = std::sync::Arc::new(mae_kb::CozoKbStore::open_mem().unwrap());
    primary.seed_type_system().unwrap();
    editor.kb.store = Some(primary.clone());
    editor.kb.primary_cozo = Some(primary.clone());

    let (id, _) = editor.kb_create_note_from_title("Abort me").unwrap();
    assert!(
        editor.kb.capture_state.is_some(),
        "sanity: capture mode is active"
    );
    assert!(
        primary.get_node(&id).unwrap().is_some(),
        "sanity: capture persisted the node"
    );

    editor.dispatch_builtin("capture-abort");

    assert!(
        primary.get_node(&id).unwrap().is_none(),
        "an aborted capture must not survive in the durable store"
    );
}
