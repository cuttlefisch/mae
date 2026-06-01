# MCP Architecture — MAE

> Last updated: 2026-05-19 (v0.11.0)

## Overview

MAE exposes its editor tools via the **Model Context Protocol (MCP)** — a JSON-RPC 2.0-based protocol for AI tool calling. Three components form the MCP subsystem:

1. **Server** (`mae-mcp`, `lib.rs`) — accepts connections from MCP clients over a Unix domain socket, dispatches tool calls to the editor.
2. **Client** (`mae-mcp`, `client.rs`) — connects to *external* MCP servers (e.g., filesystem, GitHub) via stdio transport.
3. **Shim** (`mae-mcp-shim`) — bridges stdio ↔ Unix socket so tools like Claude Code can connect.

```
┌──────────────┐     stdio      ┌──────────────┐   Unix socket   ┌──────────────┐
│  Claude Code │ ◄────────────► │ mae-mcp-shim │ ◄──────────────► │  MAE Editor  │
│  (MCP client)│                │   (bridge)   │                  │ (MCP server) │
└──────────────┘                └──────────────┘                  └──────────────┘

┌──────────────┐     stdio      ┌──────────────┐
│ External MCP │ ◄────────────► │  MAE Editor   │
│   server     │                │ (MCP client)  │
└──────────────┘                └───────────────┘
```

The built-in AI agent (SPC a p) does **not** use MCP — it dispatches tools directly via `tool_tx` channels. MCP is only exercised by external clients.

## Wire Format

All messages use **Content-Length framing** (LSP-compatible):

```
Content-Length: 42\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"$/ping"}
```

The server's reader (`read_message()`) auto-detects framing:
- If the stream starts with `Content-Length:`, reads the header + exact body bytes.
- Otherwise, reads a single line (legacy line-based fallback for backward compatibility).

The server's writer always uses Content-Length framing. The client writer (v0.11.0+) also uses Content-Length framing.

**Maximum message size:** 10 MB (`MAX_MESSAGE_SIZE`).

## Handshake

The MCP handshake follows JSON-RPC conventions:

```
Client → Server:  initialize (request, with id)
Server → Client:  response (capabilities, serverInfo, protocolVersion)
Client → Server:  notifications/initialized (notification, NO id)
                  ← session.initialized = true
Client → Server:  tools/list, tools/call, etc.
```

Key points:
- `notifications/initialized` is a **notification** (no `id` field, no response expected). Per JSON-RPC 2.0 spec, notifications MUST NOT have an `id`.
- For backward compatibility, the server also handles `notifications/initialized` as a request (with `id`) in `handle_request()`, but the proper path is the notification handler in `handle_client()`.
- The protocol version constant is `PROTOCOL_VERSION` in `protocol.rs` (currently `"2024-11-05"`).

## Method Catalog

### Protocol Methods

| Method | Type | Description |
|--------|------|-------------|
| `initialize` | request | Handshake — client sends capabilities, server responds with its own |
| `notifications/initialized` | notification | Client confirms ready state |
| `shutdown` | request | Graceful session teardown |
| `$/ping` | request | Heartbeat, returns `"pong"` |
| `$/health` | request | Session diagnostics (uptime, message count, initialized flag) |
| `$/resync` | request | Session state dump for recovery |
| `$/debug` | request | Server-wide debug info (state-server only) |

### Tool Methods

| Method | Type | Description |
|--------|------|-------------|
| `tools/list` | request | Enumerate available tools with schemas |
| `tools/call` | request | Execute a tool by name with arguments |

### Event Subscription

| Method | Type | Description |
|--------|------|-------------|
| `notifications/subscribe` | request | Subscribe to event types |

### Sync Methods (State Server)

| Method | Type | Description |
|--------|------|-------------|
| `sync/update` | request | Apply CRDT update |
| `sync/state_vector` | request | Get state vector for a document |
| `sync/full_state` | request | Get full CRDT state |
| `sync/diff` | request | Compute diff from state vector |
| `sync/resync` | request | Full resync (gap recovery) |
| `sync/share` | request | Share a document |
| `docs/list` | request | List active documents |
| `docs/content` | request | Get document content |
| `docs/stats` | request | Document statistics |
| `docs/save_intent` | request | Declare intent to save (SHA-256 hash) |
| `docs/save_committed` | request | Confirm save completed |
| `docs/delete` | request | Delete a document |

## Multi-Client Sessions

Each connected client gets a `ClientSession` with:
- **Session ID**: monotonically increasing u64
- **Client info**: name, version (from `initialize` params)
- **Initialized flag**: set by `notifications/initialized`
- **Subscriptions**: set of event types (e.g., `buffer_edit`, `cursor_move`)
- **Counters**: `messages_received`, `tool_calls`
- **Timestamps**: `connected_at`, `last_activity` (for idle detection)

Sessions are independent — one client's failure doesn't affect others.

## Event Broadcasting

The `SharedBroadcaster` distributes editor state changes to subscribed clients:

- **Queue size**: 100 events per client (bounded)
- **Backpressure**: if a client's queue is full, events are dropped (not blocked)
- **Write timeout**: 5 seconds per write — slow clients are disconnected
- **Wildcard**: subscribing to `"*"` receives all event types
- **Sequencing**: each notification carries a per-client `seq` number for ordering

Event types: `buffer_edit`, `cursor_move`, `diagnostics`, `mode_change`, `buffer_open`, `buffer_close`.

## Error Codes

### Standard JSON-RPC

| Code | Name |
|------|------|
| -32700 | Parse error |
| -32601 | Method not found |
| -32603 | Internal error |

### MAE Application Codes

| Code | Name | Description |
|------|------|-------------|
| -32000 | Backpressure | Client queue full, event dropped |
| -32001 | Editor busy | Tool dispatch channel full |
| -32002 | Tool not found | Unknown tool name |
| -32003 | Invalid session | Session ID not recognized |
| -32004 | Session expired | Session timed out |

## Transport Layers

| Transport | Used by | Address |
|-----------|---------|---------|
| Unix socket | Editor MCP server | `/tmp/mae-{PID}.sock` |
| TCP | State server | `127.0.0.1:9473` (configurable) |
| stdio | Shim bridge, MCP client | stdin/stdout of child process |

## Client Implementation

`McpClient` manages a connection to an external MCP server:

1. **Spawn** child process with piped stdin/stdout
2. **Writer task**: serializes JSON-RPC messages with Content-Length framing to stdin
3. **Reader task**: reads responses using `read_message()` (auto-detects framing)
4. **Pending requests**: `HashMap<u64, oneshot::Sender>` for correlating responses by `id`
5. **Notifications**: `send_notification()` sends fire-and-forget messages (no `id`, no response tracking)

`McpClientManager` manages multiple `McpClient` instances, configured via `[[mcp.servers]]` in `config.toml`.

## Shim Behavior

`mae-mcp-shim` is a standalone binary that bridges stdio ↔ Unix socket:

1. **Socket auto-discovery**: scans `/tmp/mae-*.sock` for a valid MAE socket
2. **Bidirectional relay**: stdin → socket, socket → stdout
3. **Framing**: uses `read_message()` / `write_framed()` for Content-Length framing on both sides
4. **Debug logging**: set `MAE_MCP_SHIM_LOG=/path/to/log` to trace all traffic
5. **Error handling**: exits cleanly on EOF from either side

## Security

- **Unix socket permissions**: standard filesystem permissions (owner-only by default)
- **MCP socket**: no per-client auth (Unix permissions only, local use)
- **Collab TCP**: PSK mutual auth (HMAC-SHA256) since v0.11.0; no TLS (plaintext)
- **Auth roadmap**: ✅ PSK → SSH key exchange → OAuth/OIDC (via `initialize` params extension)
- **Transcripts**: stored in `~/.local/share/mae/transcripts/` — contain raw tool output (no secret scrubbing)
- **Shell blocklist**: substring-based, bypassable — defense in depth, not a sandbox

## Files

| File | Role |
|------|------|
| `crates/mcp/src/lib.rs` | Server: listener, client handler, request dispatch, `read_message`/`write_framed` |
| `crates/mcp/src/client.rs` | Client: connect to external MCP servers via stdio |
| `crates/mcp/src/client_mgr.rs` | Client manager: lifecycle for multiple external servers |
| `crates/mcp/src/protocol.rs` | JSON-RPC types, `PROTOCOL_VERSION` constant, error codes |
| `crates/mcp/src/session.rs` | `ClientSession` struct, idle tracking |
| `crates/mcp/src/broadcast.rs` | `SharedBroadcaster`, event types, subscription filtering |
| `crates/mcp/src/shim.rs` | `mae-mcp-shim` binary |
