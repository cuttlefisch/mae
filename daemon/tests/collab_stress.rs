//! Stress tests for collab infrastructure.
//! Gated behind MAE_STRESS_TEST=1 env var.
//! Run: MAE_STRESS_TEST=1 cargo test -p mae --test collab_stress -- --ignored --nocapture

use std::sync::{Arc, Once};

use mae_daemon::collab_handler::handle_client;
use mae_daemon::doc_store::DocStore;
use mae_daemon::storage::SqliteBackend;
use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use mae_sync::encoding::update_to_base64;
use mae_sync::text::TextSync;
use tokio::io::BufReader;

// --- Env gate ---

fn should_run() -> bool {
    std::env::var("MAE_STRESS_TEST").is_ok()
}

// --- Tracing ---

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_test_writer()
            .try_init();
    });
}

// --- Test Infrastructure ---

fn test_broadcaster() -> SharedBroadcaster {
    Arc::new(std::sync::Mutex::new(EventBroadcaster::new()))
}

fn test_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
}

struct Client {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    reader: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    next_id: u64,
}

impl Client {
    async fn connect(store: Arc<DocStore>, broadcaster: SharedBroadcaster) -> Self {
        let (client_stream, server_stream) = tokio::io::duplex(65536);
        let (server_read, server_write) = tokio::io::split(server_stream);
        let server_reader = BufReader::new(server_read);

        tokio::spawn(async move {
            handle_client(
                server_reader,
                server_write,
                store,
                broadcaster,
                std::time::Instant::now(),
                mae_sync::kb::Transport::Hub,
            )
            .await;
        });

        let (client_read, client_write) = tokio::io::split(client_stream);
        let client_reader = BufReader::new(client_read);

        let mut client = Client {
            writer: client_write,
            reader: client_reader,
            next_id: 1,
        };
        client.initialize().await;
        client.subscribe().await;
        client
    }

    async fn send(&mut self, msg: &serde_json::Value) {
        let body = serde_json::to_vec(msg).unwrap();
        mae_mcp::write_framed(&mut self.writer, &body, std::time::Duration::from_secs(5))
            .await
            .unwrap();
    }

    async fn recv(&mut self) -> serde_json::Value {
        loop {
            let text = mae_mcp::read_message(&mut self.reader)
                .await
                .unwrap()
                .unwrap();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            // Skip notifications (have "method" but no "result"/"error").
            if val.get("method").is_some()
                && val.get("result").is_none()
                && val.get("error").is_none()
            {
                continue;
            }
            return val;
        }
    }

    async fn recv_timeout(&mut self, ms: u64) -> Option<serde_json::Value> {
        match tokio::time::timeout(
            std::time::Duration::from_millis(ms),
            mae_mcp::read_message(&mut self.reader),
        )
        .await
        {
            Ok(Ok(Some(text))) => serde_json::from_str(&text).ok(),
            _ => None,
        }
    }

    async fn initialize(&mut self) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "initialize",
            "params": {"clientInfo": {"name": "stress-test"}}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "initialize failed: {resp}");
    }

    async fn subscribe(&mut self) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "notifications/subscribe",
            "params": {"types": ["sync_update", "peer_joined", "peer_left"]}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "subscribe failed: {resp}");
    }

    async fn share(&mut self, doc: &str, content: &str) {
        let ts = TextSync::new(content);
        let state = ts.encode_state();
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "sync/share",
            "params": {"doc": doc, "update": update_to_base64(&state)}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "share failed: {resp}");
    }

    async fn send_update(&mut self, doc: &str, update: &[u8]) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "sync/update",
            "params": {"doc": doc, "update": update_to_base64(update)}
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    async fn full_state(&mut self, doc: &str) -> Vec<u8> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "sync/full_state",
            "params": {"doc": doc}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        mae_sync::encoding::base64_to_update(resp["result"]["state"].as_str().unwrap()).unwrap()
    }

    async fn content(&mut self, doc: &str) -> String {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "docs/content",
            "params": {"doc": doc}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        resp["result"]["content"].as_str().unwrap().to_string()
    }

    /// Drain all pending notifications.
    async fn drain_notifications(&mut self) {
        while self.recv_timeout(100).await.is_some() {}
    }

    /// Graceful shutdown: close the write half so the server handler exits.
    async fn disconnect(self) {
        drop(self.writer);
        drop(self.reader);
    }
}

// ============================================================================
// Stress Tests
// ============================================================================

/// 3 clients, 1 doc, each sends 200 rapid single-char inserts.
/// Verify convergence: all 600 chars present.
#[tokio::test]
#[ignore]
async fn stress_sustained_session_3_peers() {
    if !should_run() {
        return;
    }
    init_tracing();

    let store = test_doc_store();
    let bc = test_broadcaster();

    // Client 1 shares the doc.
    let mut c1 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    c1.share("stress-3peer.txt", "").await;

    // Clients 2 and 3 join.
    let mut c2 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut c3 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Each client gets its own TextSync to track local state.
    let state = c1.full_state("stress-3peer.txt").await;
    let mut ts1 = TextSync::from_state(&state).unwrap();
    let mut ts2 = TextSync::from_state(&state).unwrap();
    let mut ts3 = TextSync::from_state(&state).unwrap();

    const EDITS_PER_CLIENT: usize = 200;

    // Each client inserts 200 chars. We interleave sends for realism.
    for i in 0..EDITS_PER_CLIENT {
        let ch1 = (b'a' + (i % 26) as u8) as char;
        let ch2 = (b'A' + (i % 26) as u8) as char;
        let ch3 = (b'0' + (i % 10) as u8) as char;

        // Each client inserts at the end of its own known content.
        let pos1 = ts1.content().chars().count() as u32;
        let update1 = ts1.insert(pos1, &ch1.to_string());
        c1.send_update("stress-3peer.txt", &update1).await;

        let pos2 = ts2.content().chars().count() as u32;
        let update2 = ts2.insert(pos2, &ch2.to_string());
        c2.send_update("stress-3peer.txt", &update2).await;

        let pos3 = ts3.content().chars().count() as u32;
        let update3 = ts3.insert(pos3, &ch3.to_string());
        c3.send_update("stress-3peer.txt", &update3).await;
    }

    // Give the server a moment to process all updates.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Drain notifications from all clients.
    c1.drain_notifications().await;
    c2.drain_notifications().await;
    c3.drain_notifications().await;

    // All clients fetch final content from server.
    let content1 = c1.content("stress-3peer.txt").await;
    let content2 = c2.content("stress-3peer.txt").await;
    let content3 = c3.content("stress-3peer.txt").await;

    // All should see the same content.
    assert_eq!(content1, content2, "c1 and c2 diverged");
    assert_eq!(content2, content3, "c2 and c3 diverged");

    // Total chars should be 3 * 200 = 600.
    assert_eq!(
        content1.chars().count(),
        3 * EDITS_PER_CLIENT,
        "expected {} chars, got {}",
        3 * EDITS_PER_CLIENT,
        content1.chars().count()
    );

    eprintln!(
        "stress_sustained_session_3_peers: converged with {} chars",
        content1.chars().count()
    );
}

/// Create a ~300KB doc (10k lines), 2 clients editing at distant positions.
/// 100 edits each. Verify convergence, no truncation.
#[tokio::test]
#[ignore]
async fn stress_large_document_concurrent_edits() {
    if !should_run() {
        return;
    }
    init_tracing();

    let store = test_doc_store();
    let bc = test_broadcaster();

    // Build a ~300KB document (10k lines of ~30 chars each).
    let mut initial = String::with_capacity(310_000);
    for i in 0..10_000 {
        initial.push_str(&format!("Line {:05}: abcdefghijklmnopqrst\n", i));
    }
    let initial_len = initial.chars().count();

    let mut c1 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    c1.share("stress-large.txt", &initial).await;

    let mut c2 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    let state = c1.full_state("stress-large.txt").await;
    let mut ts1 = TextSync::from_state(&state).unwrap();
    let mut ts2 = TextSync::from_state(&state).unwrap();

    const EDITS: usize = 100;

    // Client 1 edits near the beginning (positions 0..500).
    for i in 0..EDITS {
        let pos = (i * 5) as u32; // Spread across first ~500 chars.
        let update = ts1.insert(pos, "X");
        c1.send_update("stress-large.txt", &update).await;
    }

    // Client 2 edits near the end.
    for i in 0..EDITS {
        let content_len = ts2.content().chars().count();
        let pos = (content_len - 1 - (i * 3) % 500) as u32;
        let update = ts2.insert(pos, "Y");
        c2.send_update("stress-large.txt", &update).await;
    }

    // Wait for processing.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    c1.drain_notifications().await;
    c2.drain_notifications().await;

    let content1 = c1.content("stress-large.txt").await;
    let content2 = c2.content("stress-large.txt").await;

    assert_eq!(content1, content2, "clients diverged on large doc");

    let expected_len = initial_len + 2 * EDITS;
    assert_eq!(
        content1.chars().count(),
        expected_len,
        "expected {} chars (initial {} + {} edits), got {}",
        expected_len,
        initial_len,
        2 * EDITS,
        content1.chars().count()
    );

    // Verify no truncation: doc should still contain original line markers.
    assert!(content1.contains("Line 00000"), "beginning truncated");
    assert!(content1.contains("Line 09999"), "end truncated");

    eprintln!(
        "stress_large_document_concurrent_edits: converged with {} chars ({} bytes)",
        content1.chars().count(),
        content1.len()
    );
}

/// 1 stable client shares a doc, 5 transient clients cycle:
/// connect -> 5 edits -> disconnect, 5 times each.
/// Stable client verifies final content has all 125 edits.
#[tokio::test]
#[ignore]
async fn stress_rapid_connect_disconnect() {
    if !should_run() {
        return;
    }
    init_tracing();

    let store = test_doc_store();
    let bc = test_broadcaster();

    // Stable client shares an empty doc.
    let mut stable = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    stable.share("stress-churn.txt", "").await;

    const TRANSIENT_CLIENTS: usize = 5;
    const CYCLES: usize = 5;
    const EDITS_PER_CYCLE: usize = 5;
    let total_expected = TRANSIENT_CLIENTS * CYCLES * EDITS_PER_CYCLE; // 125

    for cycle in 0..CYCLES {
        for client_idx in 0..TRANSIENT_CLIENTS {
            let mut transient = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

            // Get current state from server.
            let state = transient.full_state("stress-churn.txt").await;
            let mut ts = TextSync::from_state(&state).unwrap();

            // Perform edits.
            for edit in 0..EDITS_PER_CYCLE {
                let pos = ts.content().chars().count() as u32;
                let marker = format!("[c{}r{}e{}]", client_idx, cycle, edit);
                let update = ts.insert(pos, &marker);
                transient.send_update("stress-churn.txt", &update).await;
            }

            // Small delay to let the server process before disconnect.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Disconnect by dropping.
            transient.disconnect().await;
        }
    }

    // Wait for all server processing to complete.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    stable.drain_notifications().await;

    let final_content = stable.content("stress-churn.txt").await;

    // Verify all edit markers are present.
    let mut found = 0;
    for cycle in 0..CYCLES {
        for client_idx in 0..TRANSIENT_CLIENTS {
            for edit in 0..EDITS_PER_CYCLE {
                let marker = format!("[c{}r{}e{}]", client_idx, cycle, edit);
                if final_content.contains(&marker) {
                    found += 1;
                }
            }
        }
    }

    assert_eq!(
        found, total_expected,
        "expected {total_expected} edit markers, found {found}"
    );

    eprintln!(
        "stress_rapid_connect_disconnect: {found}/{total_expected} markers verified, content len = {}",
        final_content.len()
    );
}

/// 2 clients. Client A inserts 50 chars (tracked). Client B inserts 50 chars
/// concurrently. Client A undoes its 50. Verify B's edits survive.
#[tokio::test]
#[ignore]
async fn stress_undo_under_concurrent_load() {
    if !should_run() {
        return;
    }
    init_tracing();

    let store = test_doc_store();
    let bc = test_broadcaster();

    // Client A shares an empty doc.
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    ca.share("stress-undo.txt", "").await;

    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Both get the initial (empty) state.
    let state = ca.full_state("stress-undo.txt").await;
    let mut ts_a = TextSync::from_state(&state).unwrap();
    ts_a.enable_undo();
    let mut ts_b = TextSync::from_state(&state).unwrap();

    const EDITS: usize = 50;

    // Client A inserts 50 chars: "A" repeated, each as its own undo group.
    for _ in 0..EDITS {
        let pos = ts_a.content().chars().count() as u32;
        let update = ts_a.insert(pos, "A");
        ca.send_update("stress-undo.txt", &update).await;
        // Reset undo grouping so each insert is a separate undo item.
        ts_a.undo_reset();
    }

    // Client B inserts 50 chars: "B" repeated.
    for _ in 0..EDITS {
        let pos = ts_b.content().chars().count() as u32;
        let update = ts_b.insert(pos, "B");
        cb.send_update("stress-undo.txt", &update).await;
    }

    // Wait for server to process all updates.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Sync client A with server state before undoing.
    let server_state = ca.full_state("stress-undo.txt").await;
    ts_a.apply_update(&server_state).unwrap();

    // Verify both sets of edits are present.
    let pre_undo = ts_a.content();
    let a_count_before = pre_undo.chars().filter(|&c| c == 'A').count();
    let b_count_before = pre_undo.chars().filter(|&c| c == 'B').count();
    assert_eq!(a_count_before, EDITS, "expected {EDITS} A's before undo");
    assert_eq!(b_count_before, EDITS, "expected {EDITS} B's before undo");

    // Client A undoes all its edits.
    let mut undo_successes = 0;
    for _ in 0..EDITS {
        let result = ts_a.undo();
        if result.success {
            // Send each undo's CRDT updates to the server.
            for upd in &result.updates {
                ca.send_update("stress-undo.txt", upd).await;
            }
            undo_successes += 1;
        }
    }

    // Wait for undo updates to propagate.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ca.drain_notifications().await;
    cb.drain_notifications().await;

    // Fetch final content from server.
    let final_content = ca.content("stress-undo.txt").await;

    // Client B's edits should survive.
    let b_count_after = final_content.chars().filter(|&c| c == 'B').count();
    assert_eq!(
        b_count_after, EDITS,
        "B's edits should survive A's undo: expected {EDITS} B's, got {b_count_after}"
    );

    // Client A's edits should be (mostly) gone.
    let a_count_after = final_content.chars().filter(|&c| c == 'A').count();
    eprintln!(
        "stress_undo_under_concurrent_load: undo_successes={undo_successes}, A's remaining={a_count_after}, B's={b_count_after}"
    );

    // With per-origin undo, A's chars should be removed.
    assert!(
        a_count_after < a_count_before,
        "undo should have removed some of A's edits: before={a_count_before}, after={a_count_after}"
    );
}
