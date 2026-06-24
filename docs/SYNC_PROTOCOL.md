# MAE Sync Protocol Specification

**Version:** 0.1 (v0.11.0)
**Status:** Normative — bug fixes and tests reference this spec.
**Transport:** JSON-RPC 2.0 with Content-Length framing over TCP (port 9473).

---

## 1. Terminology

| Term | Definition |
|------|-----------|
| **Document** | A named yrs CRDT document identified by a `doc_name` string. |
| **DocAddress** | Structured document identifier: `file:{hash}/{path}`, `kb:{id}`, `kbc:{kb_id}`, `shared:{name}`. |
| `kbc:{kb_id}` | KB collection manifest (node inventory for a shared KB). |
| **client_id** | yrs-level unique client identifier (u64). Deterministic: FNV-1a hash (see `compute_project_identity()` in mae-sync). |
| **State vector** | yrs `StateVector` — per-client-id clock summarizing known operations. |
| **Update** | yrs v1-encoded binary diff (base64 over the wire). |
| **WAL sequence** | Monotonically increasing server-side ID for each persisted update. |
| **Sharer** | Client that creates a document on the server via `sync/share`. |
| **Joiner** | Client that obtains document state from the server via `sync/resync`. |
| **Relay** | The daemon (`mae-daemon`) — applies updates, persists WAL, broadcasts to peers. |

---

## 2. Client State Machine

```
Disconnected ──Connect──> Connected ──Subscribe──> Subscribed
                              |                        |
                              <──────Disconnect────────<
                                                       |
                              Subscribed ──Share──> Syncing(doc)
                              Subscribed ──Join───> Syncing(doc)
```

| State | Description |
|-------|-------------|
| `Disconnected` | No TCP connection. Edits are local-only. |
| `Connected` | TCP established, `initialize` handshake complete. |
| `Subscribed` | `notifications/subscribe` sent — receiving sync_update, peer events. |
| `Syncing(doc_id)` | Actively sharing or joined to a document. Edits forwarded to server. |

**Transitions:**
- `Connect`: TCP connect + `initialize` + `subscribe`. On failure: remain Disconnected, schedule retry.
- `Share`: `sync/share` with full state. **Immediately** add doc_id to `collab_synced_buffers` (edits forwarded from this point). On server error: remove from synced set, clear `collab_doc_id`.
- `Join`: `sync/resync` → `from_state_with_client_id` → add to synced set. Edits forwarded.
- `Disconnect`: Clear `sync_doc`, `collab_doc_id`, `collab_synced_buffers` for all synced docs.

---

## 3. Server State Machine (per document)

```
NonExistent ──sync/share──> Active(connected=1)
Active ──sync/update──> Active (WAL appended, broadcast)
Active ──disconnect (last client)──> Idle
Idle ──eviction timer──> Evicted
Idle ──new client──> Active
Evicted ──sync/share──> Active (fresh)
```

| State | Description |
|-------|-------------|
| `NonExistent` | No in-memory or storage entry. |
| `Active` | In memory, `connected_clients > 0`. Updates persisted + broadcast. |
| `Idle` | In memory, `connected_clients == 0`. Subject to eviction timer. |
| `Evicted` | Removed from memory **and** storage. Equivalent to NonExistent. |

**Invariant:** Eviction MUST delete from both in-memory HashMap AND SQLite storage. Otherwise recovery reloads stale docs.

---

## 4. Message Catalog

### 4.1 `sync/share`

**Purpose:** Create or replace a document on the server.

- **Params:** `{ "doc": string, "update": base64 }`
- **Result:** `{ "doc": string, "wal_seq": u64 }`
- **Precondition:** Client is Connected/Subscribed.
- **Side effects:**
  1. Delete existing doc (memory + storage) if present.
  2. Create new doc, apply update, persist to WAL.
  3. Set `connected_clients = 1` for the sharer (atomic with creation).
  4. Broadcast `SyncUpdate` to all other subscribers.
- **Error:** Invalid base64, invalid yrs update, storage failure.

### 4.2 `sync/update`

**Purpose:** Apply an incremental edit to a document.

- **Params:** `{ "doc": string, "update": base64, "client_id"?: u64 }`
- **Result:** `{ "doc": string, "wal_seq": u64 }`
- **Precondition:** Document exists (Active or will be auto-created).
- **Side effects:**
  1. Validate update bytes.
  2. WAL append (durability before memory).
  3. Apply to in-memory doc.
  4. Broadcast `SyncUpdate` to all subscribers **except sender** (echo filtering).
  5. Trigger compaction if `update_count >= compact_threshold`.

### 4.3 `sync/state_vector`

**Purpose:** Get the server's state vector for a document.

- **Params:** `{ "doc": string }`
- **Result:** `{ "doc": string, "sv": base64 }`
- **Precondition:** None (creates empty doc if not found).

### 4.4 `sync/full_state`

**Purpose:** Get the full encoded state of a document.

- **Params:** `{ "doc": string }`
- **Result:** `{ "doc": string, "state": base64 }`
- **Precondition:** None (creates empty doc if not found).
- **Side effects:** Tracks client connection for disconnect cleanup.

### 4.5 `sync/diff`

**Purpose:** Compute what the server has that the client doesn't.

- **Params:** `{ "doc": string, "sv": base64 }`
- **Result:** `{ "doc": string, "update": base64, "server_sv": base64 }`
- **Precondition:** None.
- **Invariant:** `update` and `server_sv` MUST be computed under a single lock acquisition (INV-2).

### 4.6 `sync/resync`

**Purpose:** Full resync — returns full state + state vector atomically.

- **Params:** `{ "doc": string }`
- **Result:** `{ "doc": string, "state": base64, "sv": base64 }`
- **Precondition:** None.
- **Invariant:** `state` and `sv` MUST be computed under a single lock acquisition (INV-2).

### 4.7 `docs/list`

**Purpose:** List all in-memory documents.

- **Params:** None.
- **Result:** `{ "documents": [string] }`

### 4.8 `docs/content`

**Purpose:** Get plain text content of a document.

- **Params:** `{ "doc": string }`
- **Result:** `{ "doc": string, "content": string }`

### 4.9 `docs/stats`

**Purpose:** Get statistics for a document.

- **Params:** `{ "doc": string }`
- **Result:** `{ "doc": string, "stats": DocStats }`
- **DocStats:** `{ wal_seq, update_count, content_length, idle_secs, connected_clients }`

### 4.10 `docs/save_intent`

**Purpose:** Pre-save check — verify content hash before writing to disk.

- **Params:** `{ "doc": string, "expected_hash": string }`
- **Result:** `{ "doc": string, "result": { "status": "ok"|"conflict", "server_hash": string, "save_epoch"?: u64 } }`
- **Side effects:** On match, increments `save_epoch`.

### 4.11 `docs/save_committed`

**Purpose:** Notify server that a save completed.

- **Params:** `{ "doc": string, "saved_by": string, "save_epoch": u64, "content_hash": string }`
- **Result:** `{ "doc": string, "committed": true }`
- **Side effects:** Records save metadata. Broadcasts `SaveCommitted` to all subscribers except sender.

### 4.12 `docs/delete`

**Purpose:** Delete a document from memory and storage.

- **Params:** `{ "doc": string }`
- **Result:** `{ "doc": string, "deleted": true }`

### 4.13 `$/ping`

**Purpose:** Heartbeat / latency measurement.

- **Params:** None.
- **Result:** `"pong"`

### 4.14 `$/debug`

**Purpose:** Server diagnostics.

- **Params:** None.
- **Result:** `{ documents, doc_stats, version, uptime_secs, connection_count }`

### 4.15 `sync/awareness`

**Purpose:** Broadcast ephemeral cursor/selection/presence state to peers.

- **Params:** `{ "doc": string, "state": AwarenessState }`
- **AwarenessState:** `{ "user_name": string, "cursor_row": int, "cursor_col": int, "selection": [sr,sc,er,ec] | null, "mode": string }`
- **Result:** `{ "doc": string }`
- **Side effects:** Server relays to all subscribers on the same document (echo-filtered via `broadcast_except`).
- **Guarantees:** Not persisted. Clients throttle to 20 Hz. Stale users removed after 30s.

### 4.16 `docs/metadata`

**Purpose:** Get or set document metadata (e.g., kb_id, creator, node_count).

- **Params:** `{ "doc": string, "metadata"?: object }`
- **Result:** `{ "doc": string, "metadata": object }`
- **Side effects:** If `metadata` param present, stores it. Otherwise returns existing metadata.

---

## 5. Invariants

| ID | Invariant | Enforcement |
|----|-----------|-------------|
| INV-1 | WAL entry exists before in-memory apply | `DocStore::apply_update` calls `wal_append` before `doc.sync.apply_update` |
| INV-2 | State vector consistency | `sync/resync` and `sync/diff` compute state + sv under single doc lock |
| INV-3 | Echo filtering | `sync/update` broadcasts via `broadcast_except(session_id)` |
| INV-4 | Convergence | All clients applying the same update set reach identical content (yrs/YATA guarantee) |
| INV-5 | connected_clients accuracy | `sync/share` atomically creates doc with `connected_clients = 1`. Disconnect decrements. |
| INV-6 | Eviction completeness | `evict_idle` removes from HashMap AND deletes from SQLite storage |
| INV-7 | Cursor drift prevention | `collab_bridge.rs` captures char offsets before `apply_sync_update`, restores after |
| INV-8 | WAL sequence gap detection | Client tracks `wal_seq` per doc, triggers `ForceSync` on gap |

---

## 6. Sync Lifecycle (Normative)

### 6.1 Share

1. Editor: `enable_sync(client_id)` on the buffer.
2. Editor: Compute `doc_id` from `DocAddress`.
3. Editor: Set `buf.collab_doc_id = Some(doc_id)`.
4. Editor: **Immediately** add `doc_id` to `collab_synced_buffers` (edits forwarded from this tick).
5. Editor: Send `CollabCommand::ShareBuffer { doc_id, state_bytes }`.
6. Background task: Send `sync/share` to server.
7. Server: Delete old doc, create new, apply update, set `connected_clients = 1`.
8. Server: Respond with `wal_seq`.
9. Background task: On success, emit `CollabEvent::BufferShared`.
10. Background task: On error, emit `CollabEvent::ShareFailed` → editor removes from synced set.

### 6.2 Join

1. Editor: Send `CollabCommand::JoinDoc { doc_id }`.
2. Background task: Send `sync/resync` to server.
3. Server: Return full state + state vector (atomic, single lock).
4. Background task: Emit `CollabEvent::BufferJoined { doc_id, state_bytes }`.
5. Editor: `buf.load_sync_state(state_bytes, client_id)`.
6. Editor: Add `doc_id` to `collab_synced_buffers`.
7. Edits are now forwarded to server via `drain_and_broadcast`.

### 6.3 Edit (local)

1. User types → `buf.insert_text_at()` → yrs transaction → `pending_sync_updates` populated.
2. `drain_and_broadcast()` (every tick): drain updates, broadcast to MCP subscribers.
3. If `doc_id in collab_synced_buffers`: forward update via `CollabCommand::SendUpdate`.
4. Background task: Send `sync/update` to server.
5. Server: WAL append → in-memory apply → broadcast to other sessions.

### 6.4 Edit (remote)

1. Server broadcasts `SyncUpdate` notification to subscriber.
2. Background task receives notification, emits `CollabEvent::RemoteUpdate`.
3. Editor: `buf.apply_sync_update(update_bytes)` → yrs apply → rope rebuilt.

### 6.5 Disconnect

1. TCP connection drops or `CollabCommand::Disconnect` sent.
2. Background task emits `CollabEvent::Disconnected`.
3. Editor: For all synced buffers: clear `sync_doc`, `collab_doc_id`, `pending_sync_updates`.
4. Editor: Clear `collab_synced_buffers`, set `collab_synced_docs = 0`.

### 6.6 Awareness (Cursor/Selection/Presence)

Awareness is a lightweight, **ephemeral** protocol layer for sharing cursor position,
selection ranges, and user presence. It is NOT persisted — no WAL, no SQLite.

**Method:** `sync/awareness`

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "sync/awareness",
  "params": {
    "doc": "file:project/main.rs",
    "state": {
      "user_name": "Alice",
      "cursor_row": 42,
      "cursor_col": 10,
      "selection": [1, 0, 3, 15],
      "mode": "visual"
    }
  }
}
```

**Response (ack):** `{ "result": { "doc": "file:project/main.rs" } }`

**Relay:** Server broadcasts to all other clients on the same document via
`broadcast_except(EditorEvent::AwarenessUpdate, sender_session_id)`. The sender
is echo-filtered — they never receive their own awareness updates.

**Notification (to peers):**
```json
{
  "method": "notifications/awareness_update",
  "params": {
    "seq": 5,
    "event": {
      "type": "awareness_update",
      "data": {
        "doc_id": "file:project/main.rs",
        "client_id": 42,
        "user_name": "Alice",
        "cursor_row": 42,
        "cursor_col": 10,
        "selection": [1, 0, 3, 15]
      }
    }
  }
}
```

**AwarenessState schema:**

| Field | Type | Description |
|-------|------|-------------|
| `user_name` | string | Display name (from config, git, $USER, or hostname) |
| `cursor_row` | integer | Zero-indexed cursor line |
| `cursor_col` | integer | Zero-indexed cursor column |
| `selection` | `[sr, sc, er, ec]` or null | Selection range (visual mode), null otherwise |
| `mode` | string | Editor mode: "normal", "insert", "visual" |

**Guarantees:**

- **Throttle:** Clients SHOULD send at most 20 Hz (50ms minimum interval).
- **Timeout:** Clients remove stale remote users after 30s with no update.
- **No persistence:** Awareness is purely ephemeral. It does not appear in WAL or SQLite.
- **Echo filtering:** Same mechanism as `sync/update` — `broadcast_except` with sender's session ID.
- **Subscription:** Clients must subscribe to `"awareness_update"` event type to receive notifications.

**User identity resolution order:**
1. Configured `user_name` — `init.scm` via `(set-option! "user_name" "Alice")` is the primary surface; `config.toml` `[collaboration] user_name = "Alice"` is the legacy bootstrap.
2. `git config user.name`
3. `$USER` environment variable
4. `hostname`
5. `"anonymous"` fallback

---

## 7. Known Limitations

Completed in v0.11.0:
1. ~~No offline edit recovery~~ — sync_doc preserved on disconnect, reconcile_to on reconnect *(b8d4b6a)*
2. ~~No client-side gap detection~~ — wal_seq tracking per doc, ForceSync on gap *(b8d4b6a)*
3. ~~Save protocol not wired to `:w`~~ — save_intent/save_committed called from editor save *(ca6c202)*
4. ~~No heartbeat/keepalive~~ — 30s `$/ping` (configurable via `collab_heartbeat_interval`), latency logging, missed pong → disconnect *(b8d4b6a)*

5. ~~No awareness protocol~~ — `sync/awareness` JSON-RPC relay with 50ms throttle, 30s timeout, echo filtering, 8-color theme palette, GUI+TUI rendering *(v0.11.0)*

Still deferred:
6. **No direct P2P transport.** All sync goes through the daemon. mDNS LAN peer *discovery* is implemented (`crates/mae/src/mdns_discovery.rs`, `collab_discover`); direct P2P *transport* (daemon-less sync) is still deferred.
7. **No end-to-end content encryption.** Transport is encrypted in `key` mode (mutual-TLS, ADR-017); `psk`/`none` modes remain plaintext TCP. End-to-end content encryption (encrypted-at-rest CRDT payloads) is the still-deferred piece.

---

## 8. References

- ADR-001: Protocol design (JSON-RPC 2.0, Content-Length framing)
- ADR-002: Text sync (yrs/YATA accepted)
- ADR-003: File safety (content-hash, advisory locks)
- ADR-006: Collaborative state engine
- ADR-007: Save coordination
- ADR-017: Asymmetric peer authentication (Ed25519 trusted-peer mutual-TLS)
- y-websocket: We align on update/sv exchange; we diverge on transport (TCP vs WebSocket) and framing (Content-Length vs WebSocket frames).
