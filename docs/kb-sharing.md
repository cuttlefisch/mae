# KB Sharing — User Guide

Share your MAE knowledge base with other users for real-time collaborative editing.

## Prerequisites

- Two or more MAE instances (v0.11.0+)
- Network connectivity between peers (LAN or internet)
- `mae-state-server` running (dedicated or embedded)

## Quick Start

### Option A: Dedicated Server

```bash
# On a shared machine:
mae-state-server --bind 0.0.0.0:9473

# User A (host):
mae ~/my-notes/
# In editor:
:collab-connect 192.168.1.10:9473
:kb-share default

# User B (guest):
mae
:collab-connect 192.168.1.10:9473
:kb-join default
```

### Option B: Embedded Server (P2P)

```bash
# User A starts an embedded server:
:collab-start
:kb-share default

# User B connects to User A:
:collab-connect <user-a-ip>:9473
:kb-join default
```

### Option C: mDNS Discovery (same LAN)

```bash
# User A starts server:
:collab-start

# User B discovers peers automatically:
:collab-discover
# Select peer from palette → auto-connects
:kb-join default
```

## Commands

| Command | Keybinding | Description |
|---------|-----------|-------------|
| `:collab-start` | `SPC C s` | Start embedded state server |
| `:collab-connect <addr>` | `SPC C c` | Connect to state server |
| `:kb-share [name]` | — | Share KB (default: primary) |
| `:kb-join <kb-id>` | — | Join a shared KB |
| `:kb-leave <kb-id>` | — | Stop syncing (keeps local copy) |
| `:collab-discover` | — | Browse LAN for MAE peers (mDNS) |
| `:collab-status` | `SPC C i` | Show connection + KB sync status |

## Sync Modes

Controlled by the `collab_kb_sync_mode` option:

| Mode | Behavior |
|------|----------|
| `on_save` (default) | Auto-sync when you edit a KB node via `:kb-update` or AI tools |
| `manual` | Only sync when you explicitly run `:kb-sync` |

Set via:
```
:set collab_kb_sync_mode manual
:set collab_kb_sync_mode on_save
```

Or in `config.toml`:
```toml
[collaboration]
kb_sync_mode = "on_save"
```

## What Happens Under the Hood

### Sharing (`:kb-share`)

1. All KB nodes are encoded as yrs CRDT documents (`KbNodeDoc`)
2. A collection manifest (`KbCollectionDoc`) is created listing all nodes
3. Both are uploaded to the state server
4. Other connected users can now `:kb-join`

### Joining (`:kb-join`)

1. Server sends the collection manifest + all node states
2. A shared KB directory is created at `$XDG_DATA_HOME/mae/kb/shared/<slug>/`
3. Each node is decoded from CRDT and inserted into the local KB
4. Future updates from the sharer are applied automatically

### Editing

When `on_save` mode is active:
1. You edit a shared KB node (via AI tools, `:kb-update`, etc.)
2. A CRDT update is generated (yrs diff, not full replacement)
3. The update is sent to the server
4. All other subscribers receive the update in real-time

### Leaving (`:kb-leave`)

1. Unsubscribes from future updates
2. Local copy of all nodes is preserved (local-first principle)
3. You can re-join later to get latest state

## Offline Behavior

- KB edits made while disconnected accumulate in an in-memory queue
- On reconnect, all queued updates are sent automatically
- Status line shows `[KB:N|offline|Mpending]` while disconnected
- No data is lost within a session (queue survives disconnect/reconnect)

## Status Line Indicators

| Indicator | Meaning |
|-----------|---------|
| `[KB:2\|synced]` | 2 KBs shared, all up-to-date |
| `[KB:1\|3pending]` | 1 KB shared, 3 local edits pending upload |
| `[KB:2\|offline\|5pending]` | Disconnected, 5 edits queued |
| (empty) | No KBs shared |

## Data Directory Layout

Shared KBs are stored under `$XDG_DATA_HOME/mae/kb/shared/`:

```
kb/
  shared/
    my-notes/
      kb.sqlite     # Local CRDT-enabled storage
      meta.toml     # Name, collab_id, creator, peers, last_sync
```

## Troubleshooting

Run `:collab-doctor` (`SPC C D`) for diagnostics.

Common issues:
- **"Not connected"**: Run `:collab-connect <addr>` first
- **"KB not found"**: Ensure the KB name matches (use "default" for primary)
- **No updates received**: Check that both peers are connected to the same server
- **mDNS not finding peers**: Ensure multicast is enabled on your network

## Architecture

KB sharing uses yrs (Yjs Rust port) CRDTs for conflict-free merging:

- `KbNodeDoc`: yrs document per KB node (title as YText, body as YText, tags as YArray)
- `KbCollectionDoc`: manifest listing all nodes in a shared KB
- Transport: JSON-RPC 2.0 over TCP with Content-Length framing
- Protocol methods: `kb/share`, `kb/join`, `kb/node_update`, `kb/leave`

See [ADR-005](adr/adr-005-kb-crdt.md) and [ADR-006](adr/adr-006-collaborative-state-engine.md) for design rationale.
