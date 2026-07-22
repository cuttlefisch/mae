//! #346 regression coverage: `refresh_kb_sharing_buffer()` was previously wired only
//! into KB-lifecycle events (share/join/leave/membership broadcast), never into
//! `handle_connected_event`/`handle_disconnected_event` — so an open `*KB Sharing*`
//! buffer could keep showing "Connected to X — N peer(s)" well after the daemon
//! actually died, with no unrelated KB event to trigger a repaint.

use super::*;

#[test]
fn kb_sharing_buffer_reflects_disconnect_immediately() {
    let mut editor = Editor::new();
    editor.open_kb_sharing();
    editor.collab.server_address = "127.0.0.1:9473".to_string();

    // Simulate an established connection — the buffer must show "Connected".
    handle_collab_event(
        &mut editor,
        CollabEvent::Connected {
            address: "127.0.0.1:9473".to_string(),
            peer_count: 2,
        },
    );
    let idx = editor
        .find_buffer_by_name("*KB Sharing*")
        .expect("KB Sharing buffer should exist");
    assert!(
        editor.buffers[idx].text().contains("Connected to"),
        "expected the buffer to show Connected after a Connected event, got: {}",
        editor.buffers[idx].text()
    );

    // The daemon dies — the buffer must reflect this WITHOUT any unrelated
    // KB-membership event happening first.
    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "connection reset".to_string(),
        },
    );
    let text = editor.buffers[idx].text();
    assert!(
        !text.contains("Connected to"),
        "buffer must not still show 'Connected to' after a disconnect event, got: {text}"
    );
    assert!(
        text.contains("disconnected"),
        "expected the buffer to show the disconnected status, got: {text}"
    );
}

#[test]
fn kb_sharing_buffer_reflects_reconnect_immediately() {
    let mut editor = Editor::new();
    editor.open_kb_sharing();
    editor.collab.server_address = "127.0.0.1:9473".to_string();

    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "boot".to_string(),
        },
    );
    let idx = editor.find_buffer_by_name("*KB Sharing*").unwrap();
    assert!(editor.buffers[idx].text().contains("disconnected"));

    handle_collab_event(
        &mut editor,
        CollabEvent::Connected {
            address: "127.0.0.1:9473".to_string(),
            peer_count: 1,
        },
    );
    let text = editor.buffers[idx].text();
    assert!(
        text.contains("Connected to"),
        "expected the buffer to reflect a fresh reconnect immediately, got: {text}"
    );
}
