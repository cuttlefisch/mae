# Collaborative Editing in MAE

MAE supports real-time collaborative editing through the `mae-state-server` — a
standalone CRDT document hub backed by WAL-persisted SQLite. Multiple editor
instances (human users or AI agents) converge automatically using the
[yrs](https://github.com/y-crdt/y-crdt) Rust port of Yjs (YATA algorithm).

---

## 1. Architecture Overview

Every collaborative document is identified by a URI namespace:

| Namespace | Example | Meaning |
|-----------|---------|---------|
| `file:` | `file:///home/user/project/main.rs` | File buffer |
| `kb:` | `kb://default/concept:collab-architecture` | KB node |
| `shared:` | `shared://session-id/scratchpad` | Anonymous shared doc |

**Data flow:**

```
Local edit (user or AI)
  → yrs transaction (YText insert/delete)
    → mae-sync encodes update bytes
      → TCP framed write → state server  (sync/update)
        → WAL flush → in-memory apply
          → broadcast diff → connected peers
            → peer decodes → ropey mirror rebuild → redraw
```

The state server is a **document hub**, not the source of truth. Clients hold
the authoritative CRDT state; the server merges and redistributes. On restart it
recovers by loading the latest snapshot then replaying the WAL tail.

See also: [ADR-002](adr/002-text-sync-model.md) (text sync decision),
[ADR-006](adr/006-collaborative-state.md) (state engine).

---

## 2. Quick Start

### Workflow A — Solo (no server)

No configuration needed. All edits are yrs transactions locally; undo/redo and
AI attribution work out of the box. The upgrade path to loopback is a single
option change — no data migration required.

### Workflow B — Loopback (local multi-agent)

Multiple MAE instances or AI agents on one machine share a local server.

```bash
# Terminal 1: start the server
mae-state-server
# Listening on 127.0.0.1:9473

# Terminal 2+: start MAE instances
mae
```

In each MAE instance, configure via `config.toml` (recommended):

```toml
# In ~/.config/mae/config.toml:
[collaboration]
server_address = "127.0.0.1:9473"
auto_connect = true
user_name = "alice"
```

Or via Scheme (runtime):

```scheme
(set-option! "collab-server-address" "127.0.0.1:9473")
(set-option! "collab-auto-connect" "true")
```

Or use the interactive commands: `SPC C s` (start server), `SPC C c` (connect).

### Workflow C — Collaborative (multi-user, LAN/VPN)

```bash
# Server machine
mae-state-server --bind 0.0.0.0:9473
```

Each client (`config.toml` or `init.scm`):

```toml
[collaboration]
server_address = "192.168.1.10:9473"
auto_connect = true
user_name = "bob"
```

> **Security note (v1):** There is no authentication. Restrict access via
> firewall or VPN. Do not expose the state server to the public internet.
> See [Security](#8-security) below.

---

## 3. Configuration Reference

### Editor Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `collab-server-address` | string | `""` | Server `host:port`. Empty string = solo mode. |
| `collab-auto-connect` | bool-string | `"false"` | Connect automatically on startup when address is set. |
| `collab-username` | string | `""` | Display name shown to peers (empty = system hostname). |
| `collab-wal-threshold` | integer | `500` | WAL entries before compaction (server-side). |
| `collab-write-timeout-ms` | integer | `5000` | Peer write timeout in milliseconds. |

Set options at runtime:

```scheme
(set-option! "collab-server-address" "127.0.0.1:9473")
```

Persist across restarts with `:set-save`:

```
:set collab-server-address 127.0.0.1:9473
:set-save
```

### Environment Variables

| Variable | Overrides |
|----------|-----------|
| `MAE_COLLAB_ADDR` | `collab-server-address` |
| `MAE_COLLAB_AUTO_CONNECT` | `collab-auto-connect` (`1` = true) |

### config.toml

```toml
[collab]
server_address = "127.0.0.1:9473"
auto_connect = true
username = "alice"
```

---

## 4. State Server Deployment

### CLI

```
mae-state-server [OPTIONS] [SUBCOMMAND]

Options:
  --bind <ADDR>          Listen address (default: 127.0.0.1:9473)
  --unix-socket <PATH>   Also listen on a Unix domain socket
  --db <PATH>            SQLite WAL path (default: ~/.local/share/mae/collab.db)
  --wal-threshold <N>    Compact after N WAL entries (default: 500)
  --check-config         Validate configuration and exit

Subcommands:
  doctor                 Run diagnostics (port, WAL, disk space)
```

Examples:

```bash
# Local loopback only
mae-state-server

# LAN / VPN (all interfaces)
mae-state-server --bind 0.0.0.0:9473

# Custom database path
mae-state-server --db /var/lib/mae/collab.db

# Validate config without starting
mae-state-server --check-config

# Diagnose a running or stopped server
mae-state-server doctor
```

### Systemd (user unit)

A unit file is provided at `assets/mae-state-server.service`. The recommended
way to install it is:

```bash
make install-service
# Builds binary, installs unit file, runs daemon-reload
```

Then enable and start:

```bash
systemctl --user enable --now mae-state-server
systemctl --user status mae-state-server
journalctl --user -u mae-state-server -f   # logs
```

Manual installation (without make):

```bash
cp assets/mae-state-server.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now mae-state-server
```

### Build and Install

```bash
# Build binary
make build-state-server

# Install to ~/.local/bin
make install-state-server

# Or directly
cargo install --path crates/state-server
```

### Client-Frame Workflow

Once the service is running, use `mae --connect` to open a new editor frame
that auto-connects to the state server — similar to `emacsclient -c`:

```bash
mae --connect                    # GUI, auto-connects to 127.0.0.1:9473
mae --connect 10.0.0.5:9473     # GUI, connects to remote server
mae --connect -nw                # terminal mode + auto-connect
```

Desktop launcher: `mae-connect.desktop` is installed by `make install`. It
shows up as "MAE (Connected)" in application launchers.

Add a sway/i3 keybind for instant connected frames:

```
bindsym $mod+Shift+e exec mae --connect
```

---

## 5. Network Setup

### Binding to All Interfaces

By default, `mae-state-server` listens on `127.0.0.1:9473` (loopback only).
For multi-machine collaboration, bind to all interfaces:

```bash
mae-state-server --bind 0.0.0.0:9473
```

Or in `~/.config/mae/state-server.toml`:

```toml
bind = "0.0.0.0:9473"
```

Or via a systemd override:

```bash
systemctl --user edit mae-state-server
# Add:
# [Service]
# ExecStart=
# ExecStart=%h/.local/bin/mae-state-server --bind 0.0.0.0:9473
```

### Firewall Rules

The state server binary runs as a user service (no sudo). Only firewall
changes need root privileges.

**firewalld (Fedora/RHEL/CentOS):**

```bash
sudo firewall-cmd --add-port=9473/tcp --permanent
sudo firewall-cmd --reload
```

**ufw (Ubuntu/Debian):**

```bash
sudo ufw allow 9473/tcp
```

**nftables (direct):**

```bash
sudo nft add rule inet filter input tcp dport 9473 accept
```

**iptables (legacy):**

```bash
sudo iptables -A INPUT -p tcp --dport 9473 -j ACCEPT
```

### Security Warnings

> **v1 has no authentication.** Any client that can reach the port can read
> and write all shared documents. Do not expose to the public internet.

Recommendations:
- **Local only**: Use the default `127.0.0.1` binding (no firewall needed).
- **Trusted LAN**: Bind to `0.0.0.0` with firewall rules limiting source IPs.
- **Untrusted networks**: Use [Tailscale](https://tailscale.com) or
  [WireGuard](https://www.wireguard.com) — both create encrypted tunnels
  that make the state server appear on a private IP. No firewall rules needed.
- **Never** bind to `0.0.0.0` on a machine with a public IP without a VPN.

### Connectivity Check

From a client machine:

```bash
nc -zv <server-host> 9473
```

From inside MAE: `SPC C D` (`:collab-doctor`) or `mae doctor` from the CLI.

---

## 6. Commands Reference

### Editor Commands

| Key | Command | Description |
|-----|---------|-------------|
| `SPC C s` | `:collab-start-server` | Start a local state server process |
| `SPC C c` | `:collab-connect` | Connect to configured server |
| `SPC C d` | `:collab-disconnect` | Disconnect from current server |
| `SPC C S` | `:collab-share-buffer` | Share active buffer with connected peers |
| `SPC C i` | `:collab-status` | Show connection info, peers, shared docs |
| `:collab-doctor` | — | Comprehensive diagnostic report |
| `:collab-status` | — | Live connection state (also available as `SPC C i`) |

### AI Tools

The AI agent has direct access to collaboration state:

| Tool | Description |
|------|-------------|
| `collab_status` | Report connection state, peer list, shared documents |
| `collab_connect` | Connect to (or reconnect to) the configured server |
| `collab_share` | Share a named buffer with connected peers |
| `collab_doctor` | Diagnostics: reachability, WAL health, peer count |

Example AI interaction:

```
User: connect to the collab server and share this buffer
AI: [calls collab_connect, then collab_share with current buffer name]
```

### Sync Protocol Methods (JSON-RPC 2.0)

These are low-level methods on the TCP transport, documented for
integrators building non-MAE clients:

| Method | Description |
|--------|-------------|
| `sync/update` | Push a yrs update to the server |
| `sync/state_vector` | Retrieve the server's state vector for a document |
| `sync/full_state` | Fetch the full CRDT document bytes |
| `sync/diff` | Get the diff between client and server state vectors |
| `docs/list` | List all documents held by the server |
| `docs/content` | Fetch materialized text content of a document |
| `$/debug` | Dump internal server state (diagnostics only) |

---

## 7. Debugging and Troubleshooting

### Quick Checks

```bash
# Is the server listening?
ss -tlnp | grep 9473

# View server logs
journalctl --user -u mae-state-server -f

# Run the doctor subcommand
mae-state-server doctor
```

### From Inside MAE

- `SPC C i` / `:collab-status` — live peer list and document state
- `:collab-doctor` — full diagnostic: TCP reachability, WAL row count, compaction
  status, peer latency
- `MAE_LOG=mae_state_server=debug mae-state-server` — verbose server logging

### MCP Debug Tool

Ask the AI to call `$/debug` on the server:

```
User: show me the state server internals
AI: [calls collab_doctor or issues $/debug via sync transport]
```

### Common Issues

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Connection refused | Server not running | `mae-state-server` or `SPC C s` |
| No peers visible | Wrong `collab-server-address` | Check all clients use same address |
| Stale state after restart | WAL replay needed | Automatic; check logs for errors |
| Slow sync | Peer write timeout | Increase `collab-write-timeout-ms` |
| WAL grows unbounded | Compaction threshold too high | Lower `collab-wal-threshold` |

### WAL Integrity

The state server appends every `sync/update` to the SQLite WAL **before**
applying it to memory. On restart:

1. Load the latest compacted snapshot (if any).
2. Replay WAL entries newer than the snapshot.
3. Serve from the recovered in-memory state.

If the WAL is corrupted, delete `~/.local/share/mae/collab.db` and restart. All
connected clients will push their local state on reconnect, restoring the merged
document.

---

## 8. Security

**v1 posture: no authentication.** The TCP port is open to any client that can
reach it. Planned upgrade path:

| Phase | Mechanism |
|-------|-----------|
| v1 (current) | No auth — trusted LAN / VPN only |
| v2 | Pre-shared key (PSK) in `initialize` params |
| v3 | SSH key exchange |
| v4 | OAuth 2.0 / OIDC for enterprise deployments |

**Recommendations for v1:**

- Bind to `127.0.0.1` for solo/loopback use (default).
- Use a VPN (WireGuard, Tailscale) when collaborating across machines.
- Firewall the port (`9473`) from untrusted networks.
- Never bind to `0.0.0.0` on a machine with a public IP without a VPN or firewall rule.

Unix domain socket (`--unix-socket`) access is controlled by filesystem
permissions. Use it for intra-machine IPC where tighter isolation is needed.

---

## 9. Data Lifecycle

### Disconnect Behavior

| Scenario | What happens |
|----------|-------------|
| Graceful quit (`:q`) | TCP close → server broadcasts `peer_left` → doc persists |
| Client crash | TCP keepalive timeout → same as graceful |
| Network drop | Write timeout (5s) → server drops client → `peer_left` |
| Last client leaves | Doc stays in memory + WAL. Idle timer starts. Evicted after `idle_eviction_secs`. |

### Reconnection

1. Client connects to state server
2. Sends `sync/diff` with local state vector
3. Server returns missing updates
4. Client applies updates → rebuilds rope → status bar shows diff count
5. Client decides when to `:w` (local file may be stale)

### Save Behavior for Joiners

- Joiners always get a `file_path` set (even if the file doesn't exist yet)
- `:w` creates parent directories if needed
- Each client writes their own local copy independently
- `docs/save_committed` notifies peers ("saved by alice" in status bar)

### Git Workflow

CRDT and git are complementary:
- CRDT handles real-time character-level sync
- Git handles version history and branching
- Each client commits to their own worktree
- Conflicts are rare because CRDT already converged content

---

## See Also

- `docs/adr/002-text-sync-model.md` — text sync decision (ADR-002)
- `docs/adr/006-collaborative-state.md` — state engine architecture (ADR-006)
- `:help concept:collab-architecture` — KB node with data-flow diagram
- `:help concept:collab-workflows` — KB node with per-workflow recipes
- `:help lesson:collab-setup` — step-by-step setup tutorial
- `assets/mae-state-server.service` — systemd unit file
