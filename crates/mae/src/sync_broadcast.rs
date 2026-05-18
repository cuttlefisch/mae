//! Drain pending sync updates from buffers and broadcast to MCP clients.

use mae_core::Editor;
use mae_mcp::broadcast::{EditorEvent, SharedBroadcaster};

/// Drain all pending yrs sync updates from editor buffers and broadcast
/// them to subscribed MCP clients.
///
/// This is a no-op if no buffers have sync enabled or no updates are pending.
/// Called on `IdleTick` (~100ms) and after `McpToolRequest` completion.
pub fn drain_and_broadcast(editor: &mut Editor, broadcaster: &SharedBroadcaster) {
    for buf in &mut editor.buffers {
        if buf.pending_sync_updates.is_empty() {
            continue;
        }
        let updates: Vec<Vec<u8>> = buf.pending_sync_updates.drain(..).collect();
        let buffer_name = buf.name.clone();
        let mut bc = broadcaster.lock().unwrap();
        for update in updates {
            let event = EditorEvent::SyncUpdate {
                buffer_name: buffer_name.clone(),
                update_base64: mae_sync::encoding::update_to_base64(&update),
                wal_seq: 0,
            };
            bc.broadcast(&event);
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
        drain_and_broadcast(&mut editor, &bc);
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

        drain_and_broadcast(&mut editor, &bc);

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

        drain_and_broadcast(&mut editor, &bc);

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

        drain_and_broadcast(&mut editor, &bc);

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
