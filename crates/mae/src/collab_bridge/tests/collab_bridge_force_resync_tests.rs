//! #338 regression coverage: `CollabEvent::BufferResynced` (the ForceSync response
//! path) must always full-replace via `load_sync_state`, never merge via
//! `apply_sync_update` — a buffer reopened after a restart has a `sync_doc` that was
//! only ever locally fabricated from on-disk content (`enable_sync`), with no real
//! causal relationship to the daemon's actual edit history. Before the fix, this
//! event reused `BufferJoined`'s handler, which took the merge path for any
//! already-existing synced buffer and — under a colliding yrs `client_id` between the
//! fabricated local doc and the real remote history — silently deleted content
//! instead of erroring. Reproduced live (see PR description / issue #338) down to
//! exactly this "buffer emptied" symptom; these tests pin it down headlessly.

use super::*;

/// The adversarial case: local and remote docs deliberately share a `client_id` (the
/// exact precondition that caused real data loss before the fix — see
/// `compute_client_id`'s doc comment for why this can no longer happen from normal
/// dispatch, but the fix must hold even if it did). Remote history is realistic (an
/// insert followed by an edit), not a trivial single op — the corruption only
/// reproduced with real edit history in the original repro.
#[test]
fn force_resync_preserves_content_even_under_a_colliding_client_id() {
    const COLLIDING_CLIENT_ID: u64 = 424_242;

    // Build the "remote" (daemon-authored) history: a realistic multi-op edit
    // sequence, not one bulk insert.
    let mut remote = mae_sync::text::TextSync::with_client_id("", COLLIDING_CLIENT_ID);
    remote.insert(0, "CANARY_A");
    remote.insert(8, "\n");
    remote.insert(0, "prefix-");
    remote.delete(0, 7); // undo the prefix — a real edit history, not a single op
    let state_bytes = remote.encode_state();
    let remote_final = remote.content();
    assert_eq!(remote_final, "CANARY_A\n");

    // Build the "local" buffer: reopened from disk, `enable_sync`'d under the SAME
    // client_id as the remote (the collision), then given a local-only edit before
    // the resync arrives — the exact scenario that produced total content loss in
    // the live repro.
    let mut editor = Editor::new();
    let mut buf = mae_core::Buffer::new();
    buf.name = "notes.txt".to_string();
    buf.insert_text_at(0, "CANARY_A\n"); // matches on-disk content
    buf.enable_sync(COLLIDING_CLIENT_ID);
    buf.collab_doc_id = Some("file:proj/notes.txt".to_string());
    buf.insert_text_at(9, "LOCAL_ONLY_EDIT\n");
    editor.buffers.push(buf);
    editor
        .collab
        .synced_buffers
        .insert("file:proj/notes.txt".to_string());

    // Fire the ForceSync response event.
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferResynced {
            doc_id: "file:proj/notes.txt".to_string(),
            state_bytes,
        },
    );

    let idx = editor
        .find_buffer_by_name("notes.txt")
        .expect("buffer must still exist");
    assert_eq!(
        editor.buffers[idx].text(),
        remote_final,
        "ForceSync resync must full-replace with the daemon's real content, \
         not merge-corrupt it (even under a colliding client_id) or go empty"
    );
}

/// Same colliding-client-id setup as above, but via the real dispatch path
/// (`CollabIntent::ForceSync` -> `doc_intent_to_command` -> `CollabCommand::ForceSync`)
/// rather than constructing the event directly, so the `doc_id` normalization drive-by
/// fix is exercised too.
#[test]
fn force_resync_via_collab_sync_command_normalizes_doc_id_and_preserves_content() {
    const COLLIDING_CLIENT_ID: u64 = 777_777;

    let mut remote = mae_sync::text::TextSync::with_client_id("", COLLIDING_CLIENT_ID);
    remote.insert(0, "REAL REMOTE CONTENT\n");
    let state_bytes = remote.encode_state();

    let mut editor = Editor::new();
    let mut buf = mae_core::Buffer::new();
    buf.name = "shared.txt".to_string();
    buf.insert_text_at(0, "REAL REMOTE CONTENT\n");
    buf.enable_sync(COLLIDING_CLIENT_ID);
    buf.collab_doc_id = Some("file:proj/shared.txt".to_string());
    editor.buffers.push(buf);
    editor
        .collab
        .synced_buffers
        .insert("file:proj/shared.txt".to_string());

    // :collab-sync passes the raw buffer NAME, not the doc_id — the drive-by fix
    // must resolve it to the buffer's established collab_doc_id.
    let cmd = crate::collab_bridge::events_doc::doc_intent_to_command(
        &mut editor,
        crate::collab_bridge::CollabIntent::ForceSync {
            buffer_name: "shared.txt".to_string(),
        },
    );
    match cmd {
        Some(CollabCommand::ForceSync { doc_id }) => {
            assert_eq!(
                doc_id, "file:proj/shared.txt",
                "ForceSync must resolve to the buffer's collab_doc_id, not the raw name"
            );
        }
        other => panic!("expected CollabCommand::ForceSync, got {other:?}"),
    }

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferResynced {
            doc_id: "file:proj/shared.txt".to_string(),
            state_bytes,
        },
    );
    let idx = editor.find_buffer_by_name("shared.txt").unwrap();
    assert_eq!(editor.buffers[idx].text(), "REAL REMOTE CONTENT\n");
}

/// Layer 2 (the underlying hazard class): `compute_client_id` must never repeat
/// within a process, even for the same buffer index called back-to-back — this is
/// what makes the collision precondition above unreachable from real dispatch.
#[test]
fn compute_client_id_never_repeats_for_the_same_buffer_index() {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for _ in 0..500 {
        let id = crate::collab_bridge::compute_client_id(0);
        assert!(
            seen.insert(id),
            "compute_client_id produced a repeat for the same buffer_idx: {id}"
        );
        // Must also stay within yrs's legal 53-bit ClientID range.
        assert!(
            id < (1u64 << 53),
            "client_id {id} exceeds yrs's 53-bit range"
        );
        assert_ne!(id, 0);
        assert_ne!(id, 1);
    }
}
