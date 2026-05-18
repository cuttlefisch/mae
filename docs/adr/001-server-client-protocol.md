# ADR-001: Server-Client Protocol

**Status**: Accepted
**Date**: 2026-05-16
**KB Source**: `concept:adr-server-client-protocol`

## Context

MAE's MCP server was single-client and sequential — it could only handle one
connected client at a time. Messages used fragile line-based framing that breaks
if JSON contains literal newlines. There was no session management, heartbeat,
or state notification mechanism.

Multiple concurrent clients are needed for:
- Multiple AI agents working on the same project
- Human + AI collaboration (editor UI + Claude Code)
- Headless AI-only sessions alongside interactive editing

## Decision

Extend the existing MCP server with multi-client support rather than building a
new RPC layer. Reuse JSON-RPC 2.0 and adopt Content-Length framing from the LSP
transport (already implemented in `crates/lsp/src/transport.rs`).

### Protocol Changes

1. **Content-Length framing**: Messages use `Content-Length: N\r\n\r\n{body}`
   format. Auto-detect fallback to line-based for backward compatibility.

2. **Concurrent clients**: Each connection spawns its own tokio task with a
   `ClientSession`. No shared mutable state between client tasks — all tool
   calls go through the existing `mpsc::Sender<McpToolRequest>` to the editor
   thread.

3. **Session lifecycle** (3-phase, following LSP pattern):
   - Client sends `initialize` with `clientInfo`
   - Server responds with capabilities (including `multiClient: true`)
   - Client sends `notifications/initialized`

4. **Heartbeat**: `$/ping` method returns `"pong"`. Idle detection via
   `last_activity` timestamp on `ClientSession`.

5. **State notifications**: Clients subscribe via `notifications/subscribe`
   with event types (`buffer_edit`, `cursor_move`, `diagnostics`, etc.).
   Events delivered via per-client bounded mpsc channels (100 events).
   Slow clients have events dropped (backpressure), not blocked.

6. **Write timeout**: All socket writes wrapped in `tokio::time::timeout(5s)`.
   Slow clients are disconnected.

### Wire Format

```
Content-Length: 123\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{...}}
```

Backward compatibility: if first bytes are `{` (not `Content-Length:`), fall
back to line-based reading.

## Consequences

- **Breaking**: Responses now use Content-Length framing. Existing `mae-mcp-shim`
  clients need updates to parse the new format. The shim already handles this
  since it bridges to stdio.
- **Non-breaking**: The `initialize` handshake is the same as before, just with
  richer `serverInfo` (now includes `features.multiClient`).
- **Performance**: Negligible overhead from session tracking (<5ms per request).
  Per-client tokio tasks scale to hundreds of clients.

## References

- LSP specification: Content-Length framing
- Neovim msgpack-rpc: multiplexed request/response
- Zed GPUI: per-client broadcast channels
- VS Code Live Share: OT-based state sync (deferred for MAE)
