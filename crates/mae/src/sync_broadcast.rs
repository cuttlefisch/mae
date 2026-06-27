//! Drain pending sync updates from buffers and broadcast to MCP clients
//! and optionally forward to the collaborative state server.

use mae_core::Editor;
use mae_mcp::broadcast::{EditorEvent, SharedBroadcaster};
use tracing::{info, warn};

/// Drain all pending yrs sync updates from editor buffers and broadcast
/// them to subscribed MCP clients. If `collab_tx` is provided and the
/// buffer is tracked in `collab_synced_buffers`, also forward updates to
/// the state server (fixes B5: local edits never reaching the server).
///
/// This is a no-op if no buffers have sync enabled or no updates are pending.
/// Called on `IdleTick` (~100ms) and after `McpToolRequest` completion.
pub fn drain_and_broadcast(
    editor: &mut Editor,
    broadcaster: &SharedBroadcaster,
    collab_tx: Option<&tokio::sync::mpsc::Sender<crate::collab_bridge::CollabCommand>>,
) {
    for buf in &mut editor.buffers {
        if buf.pending_sync_updates.is_empty() {
            continue;
        }
        let buffer_name = buf.name.clone();
        // Use collab_doc_id for server communication (may differ from buffer name).
        let doc_id = buf
            .collab_doc_id
            .clone()
            .unwrap_or_else(|| buffer_name.clone());
        let is_collab_synced = editor.collab.synced_buffers.contains(&doc_id);

        // If this buffer has a collab_doc_id (was shared/joined) but isn't
        // currently in synced_buffers (e.g. during disconnect/reconnect),
        // do NOT drain updates — they would be irretrievably lost.
        // Leave them in pending_sync_updates for the next drain cycle after
        // the buffer is re-added to synced_buffers.
        if buf.collab_doc_id.is_some() && !is_collab_synced {
            warn!(
                buffer = %buffer_name,
                doc = %doc_id,
                pending_count = buf.pending_sync_updates.len(),
                synced_buffers = ?editor.collab.synced_buffers,
                "deferring sync update drain — collab buffer not in synced_buffers"
            );
            continue;
        }

        let updates: Vec<Vec<u8>> = std::mem::take(&mut buf.pending_sync_updates);
        info!(buffer = %buffer_name, doc = %doc_id, update_count = updates.len(), is_collab_synced, "draining sync updates");

        let mut bc = broadcaster.lock().unwrap();
        for update in updates {
            let update_b64 = mae_sync::encoding::update_to_base64(&update);
            let event = EditorEvent::SyncUpdate {
                buffer_name: buffer_name.clone(),
                update_base64: update_b64.clone(),
                wal_seq: 0,
                content_header: None,
            };
            bc.broadcast(&event);

            // Forward to state server if this buffer is collaboratively synced.
            if is_collab_synced {
                if let Some(tx) = collab_tx {
                    info!(
                        buffer = %buffer_name,
                        doc = %doc_id,
                        update_b64_len = update_b64.len(),
                        "forwarding sync update to state server"
                    );
                    if tx
                        .try_send(crate::collab_bridge::CollabCommand::SendUpdate {
                            doc_id: doc_id.clone(),
                            update_base64: update_b64,
                        })
                        .is_err()
                    {
                        warn!(doc = %doc_id, "collab command channel full — sync update dropped");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::Buffer;

    fn test_broadcaster() -> SharedBroadcaster {
        std::sync::Arc::new(std::sync::Mutex::new(
            mae_mcp::broadcast::EventBroadcaster::new(),
        ))
    }

    #[test]
    fn drain_noop_when_no_sync() {
        let mut editor = Editor::default();
        editor.buffers.push(Buffer::new());
        let bc = test_broadcaster();
        drain_and_broadcast(&mut editor, &bc, None);
        assert!(editor.buffers[0].pending_sync_updates.is_empty());
    }

    #[tokio::test]
    async fn drain_clears_pending() {
        let mut editor = Editor::default();
        let mut buf = Buffer::new();
        buf.name = "test.rs".to_string();
        buf.insert_text_at(0, "hello");
        buf.enable_sync(1);
        // insert_text_at generates a sync update when sync is enabled
        buf.insert_text_at(5, " world");
        assert!(!buf.pending_sync_updates.is_empty());
        editor.buffers.push(buf);

        let bc = test_broadcaster();
        let mut rx = bc
            .lock()
            .unwrap()
            .subscribe(99, vec!["sync_update".to_string()]);

        drain_and_broadcast(&mut editor, &bc, None);

        assert!(editor.buffers[0].pending_sync_updates.is_empty());
        let event = rx.recv().await.unwrap();
        match event {
            EditorEvent::SyncUpdate { buffer_name, .. } => {
                assert_eq!(buffer_name, "test.rs");
            }
            _ => panic!("expected SyncUpdate"),
        }
    }

    #[tokio::test]
    async fn drain_multiple_buffers() {
        let mut editor = Editor::default();

        let mut buf_a = Buffer::new();
        buf_a.name = "a.rs".to_string();
        buf_a.insert_text_at(0, "aaa");
        buf_a.enable_sync(1);
        buf_a.insert_text_at(3, "A");
        editor.buffers.push(buf_a);

        let mut buf_b = Buffer::new();
        buf_b.name = "b.rs".to_string();
        buf_b.insert_text_at(0, "bbb");
        buf_b.enable_sync(2);
        buf_b.insert_text_at(3, "B");
        editor.buffers.push(buf_b);

        let bc = test_broadcaster();
        let mut rx = bc.lock().unwrap().subscribe(1, vec!["*".to_string()]);

        drain_and_broadcast(&mut editor, &bc, None);

        assert!(editor.buffers[0].pending_sync_updates.is_empty());
        assert!(editor.buffers[1].pending_sync_updates.is_empty());

        let mut names: Vec<String> = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EditorEvent::SyncUpdate { buffer_name, .. } = event {
                names.push(buffer_name);
            }
        }
        assert!(names.contains(&"a.rs".to_string()));
        assert!(names.contains(&"b.rs".to_string()));
    }

    #[tokio::test]
    async fn drain_forwards_to_collab_when_synced() {
        let mut editor = Editor::default();
        let mut buf = Buffer::new();
        buf.name = "collab.rs".to_string();
        buf.insert_text_at(0, "hello");
        buf.enable_sync(1);
        buf.insert_text_at(5, " world");
        editor.buffers.push(buf);
        editor.collab.synced_buffers.insert("collab.rs".to_string());

        let bc = test_broadcaster();
        let (collab_tx, mut collab_rx) =
            tokio::sync::mpsc::channel::<crate::collab_bridge::CollabCommand>(8);

        drain_and_broadcast(&mut editor, &bc, Some(&collab_tx));

        // Should have forwarded to collab channel.
        let cmd = collab_rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            crate::collab_bridge::CollabCommand::SendUpdate { .. }
        ));
    }

    #[test]
    fn drain_skips_non_sync_buffers() {
        let mut editor = Editor::default();

        // buf0: no sync — insert doesn't generate sync updates
        let mut buf0 = Buffer::new();
        buf0.name = "no-sync".to_string();
        buf0.insert_text_at(0, "hello");
        editor.buffers.push(buf0);

        // buf1: sync enabled
        let mut buf1 = Buffer::new();
        buf1.name = "synced".to_string();
        buf1.insert_text_at(0, "world");
        buf1.enable_sync(1);
        buf1.insert_text_at(5, "Y");
        editor.buffers.push(buf1);

        // buf2: no sync
        editor.buffers.push(Buffer::new());

        let bc = test_broadcaster();
        let mut rx = bc
            .lock()
            .unwrap()
            .subscribe(1, vec!["sync_update".to_string()]);

        drain_and_broadcast(&mut editor, &bc, None);

        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert!(
            count > 0,
            "should have received sync events from synced buffer"
        );
        assert!(editor.buffers[0].pending_sync_updates.is_empty());
    }
}
