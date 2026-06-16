# Collaborative Editing in MAE

MAE supports real-time collaborative editing through `mae-daemon` — a
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
      → TCP framed write → daemon  (sync/update)
        → WAL flush → in-memory apply
          → broadcast diff → connected peers
            → peer decodes → ropey mirror rebuild → redraw
```

The daemon is a **document hub**, not the source of truth. Clients hold
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

Multiple MAE instances or AI agents on one machine share a local daemon.

```bash
# Terminal 1: start the daemon
mae-daemon
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
mae-daemon --bind 0.0.0.0:9473
```

Each client (`config.toml` or `init.scm`):

```toml
[collaboration]
server_address = "192.168.1.10:9473"
auto_connect = true
user_name = "bob"
```

> **Security:** PSK mutual authentication (HMAC-SHA256) is required since v0.11.0.
> Set `collab_psk` on both server and all clients. For untrusted networks, use a VPN.
> See [Security](#8-security) below.

---

## 3. Configuration Reference

### Editor Options

Configured via `init.scm` (the primary config surface) — **not** `config.toml`,
which is being retired. Secrets (PSKs) never go in `config.toml`; see §8/§10.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `collab-server-address` | string | `127.0.0.1:9473` | Daemon `host:port`. |
| `collab-auto-connect` | bool | `false` | Connect automatically on startup. |
| `collab-user-name` | string | `""` | Display name (empty = hostname). Overridden by the authenticated identity in key mode (§10). |
| `collab-write-timeout-ms` | int | `5000` | Peer write timeout (ms). |
| `collab-reconnect-interval` | int | `5` | Base reconnect interval (s). |
| `collab-reconnect-backoff-factor` | int | `2` | Exponential backoff multiplier. |
| `collab-auth-mode` | string | `psk` | `none` \| `psk` \| `key` (trusted-peer mTLS — §10). |
| `collab-host-key-policy` | string | `prompt` | Key mode TOFU: `prompt` \| `accept-new` \| `strict`. |
| `collab-tls` | bool | `true` | Use mTLS in key mode (false = plaintext KeyAuth fallback). |
| `collab-psk` | string | `""` | PSK (plaintext fallback — prefer a keystore/command). |
| `collab-psk-command` | string | `""` | Command that prints the PSK (e.g. `pass show mae/key`). |

Set + persist at runtime (writes `init.scm`):

```
:set collab-server-address 127.0.0.1:9473
:set collab-auto-connect true
:set-save
```

Or directly in `init.scm`:

```scheme
(set-option! "collab-server-address" "192.168.1.10:9473")
(set-option! "collab-auto-connect" "true")
(set-option! "collab-auth-mode" "key")   ; trusted-peer mode (§10)
```

### Environment Variables

| Variable | Overrides |
|----------|-----------|
| `MAE_COLLAB_SERVER` | `collab-server-address` |
| `MAE_COLLAB_AUTO_CONNECT` | `collab-auto-connect` (`1` = true) |

---

## 4. Daemon Deployment

### CLI

```
mae-daemon [OPTIONS] [SUBCOMMAND]

Options:
  --bind <ADDR>          Override the collab listen address (e.g. 0.0.0.0:9473)
  --config <PATH>        Use a specific daemon.toml
  --data-dir <PATH>      Override the data directory
  --check-config         Validate configuration (+ print effective settings) and exit
  --version, -V          Print version

Subcommands:
  doctor                 Run diagnostics (config, collab storage, port)
  # Symmetric PSK mode (collab.auth.mode = "psk"):
  keygen [NAME]          Generate a random trusted key + write it to the keystore
  keys                   List trusted keys (names + fingerprints)
  # Asymmetric key mode (collab.auth.mode = "key", recommended — see §10):
  identity               Print this daemon's Ed25519 public key + fingerprint
  authorized             List authorized client keys
  authorize <pubkey>     Authorize a client public-key line (mae-ed25519 <b64> <label>)
  revoke <label>         Revoke an authorized client by label
```

All other settings (bind, storage, auth) live in `~/.config/mae/daemon.toml`
(see §4 config below). The daemon's KB Unix socket and persistence paths come
from config/XDG, not CLI flags.

Examples:

```bash
# Local loopback only (uses daemon.toml, default 127.0.0.1:9473)
mae-daemon

# LAN / VPN: override the bind for all interfaces
mae-daemon --bind 0.0.0.0:9473

# Validate config without starting
mae-daemon --check-config

# Diagnose a running or stopped server
mae-daemon doctor
```

### Systemd (user unit)

A unit file is provided at `assets/mae-daemon.service`. The recommended
way to install it is:

```bash
make install-service
# Builds binary, installs unit file, runs daemon-reload
```

Then enable and start:

```bash
systemctl --user enable --now mae-daemon
systemctl --user status mae-daemon
journalctl --user -u mae-daemon -f   # logs
```

Manual installation (without make):

```bash
cp assets/mae-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now mae-daemon
```

### Build and Install

```bash
# Build binary
make build-daemon

# Install to ~/.local/bin
make install-daemon

# Or directly
cargo install --path daemon
```

### Client-Frame Workflow

Once the service is running, use `mae --connect` to open a new editor frame
that auto-connects to the daemon — similar to `emacsclient -c`:

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

By default, `mae-daemon` listens on `127.0.0.1:9473` (loopback only).
For multi-machine collaboration, bind to all interfaces:

```bash
mae-daemon --bind 0.0.0.0:9473
```

Or in `~/.config/mae/daemon.toml`:

```toml
bind = "0.0.0.0:9473"
```

Or via a systemd override:

```bash
systemctl --user edit mae-daemon
# Add:
# [Service]
# ExecStart=
# ExecStart=%h/.local/bin/mae-daemon --bind 0.0.0.0:9473
```

### Firewall Rules

The daemon binary runs as a user service (no sudo). Only firewall
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

> **PSK authentication is required.** Both server and clients must share the
> same `collab_psk` (HMAC-SHA256). Connections without a valid PSK are rejected.

Recommendations:
- **Local only**: Use the default `127.0.0.1` binding (no firewall needed).
- **Trusted LAN**: Bind to `0.0.0.0` with firewall rules limiting source IPs.
- **Untrusted networks**: Use [Tailscale](https://tailscale.com) or
  [WireGuard](https://www.wireguard.com) — both create encrypted tunnels
  that make the daemon appear on a private IP. No firewall rules needed.
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
| `SPC C s` | `:collab-start-server` | Start a local daemon process |
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

## 6a. Awareness (Cursor/Selection/Presence)

MAE broadcasts cursor position, selection ranges, and user presence to connected
peers in real-time. Awareness is ephemeral — not persisted to WAL or SQLite.

### How It Works

1. Local cursor moves → `sync/awareness` JSON-RPC with `AwarenessState`
2. Server relays to all peers on the same document (echo-filtered)
3. Remote cursors rendered as colored markers (8-color theme palette)
4. Stale users removed after 30s with no update

### AwarenessState Schema

| Field | Type | Description |
|-------|------|-------------|
| `user_name` | string | Display name (config > git > $USER > hostname) |
| `cursor_row` | integer | Zero-indexed cursor line |
| `cursor_col` | integer | Zero-indexed cursor column |
| `selection` | `[sr, sc, er, ec]` or null | Visual mode selection range |
| `mode` | string | "normal", "insert", "visual", etc. |

### Cursor Drift Prevention

Remote edits can shift local cursor positions. MAE captures all window cursor
offsets before applying `apply_sync_update`, then restores them after the rope
is rebuilt. This prevents the cursor from "jumping" when a peer edits above the
current line.

### Throttling

Awareness updates are sent at most 20 Hz (50ms minimum interval) to avoid
flooding the server. The server relays without persistence overhead.

---

## 6b. Offline Recovery

MAE preserves sync state during disconnection and reconciles on reconnect.

### Disconnect Behavior

- `sync_doc` (yrs Doc) is preserved on the buffer — local edits continue generating
  CRDT transactions
- `collab_doc_id` and `collab_synced_buffers` are cleared (edits not forwarded)
- Buffer status shows "offline" indicator

### Reconnection

1. Client detects connection loss → `CollabStatus::Reconnecting`
2. Exponential backoff: `collab_reconnect_interval` base × `collab_reconnect_backoff_factor`
3. On reconnect: re-`initialize`, re-`subscribe`
4. For each previously synced buffer: re-`sync/share` with local CRDT state
5. Server merges local edits with any remote changes → convergence guaranteed by yrs/YATA
6. Gap detection: client tracks `wal_seq` per doc, triggers `ForceSync` if sequence gap detected

### Config Options

| Option | Default | Description |
|--------|---------|-------------|
| `collab_reconnect_interval` | `5` | Base reconnect interval (seconds) |
| `collab_reconnect_backoff_factor` | `1.5` | Exponential backoff multiplier |

---

## 7. Debugging and Troubleshooting

### Quick Checks

```bash
# Is the server listening?
ss -tlnp | grep 9473

# View server logs
journalctl --user -u mae-daemon -f

# Run the doctor subcommand
mae-daemon doctor
```

### From Inside MAE

- `SPC C i` / `:collab-status` — live peer list and document state
- `:collab-doctor` — full diagnostic: TCP reachability, WAL row count, compaction
  status, peer latency
- `MAE_LOG=mae_daemon=debug mae-daemon` — verbose daemon logging

### MCP Debug Tool

Ask the AI to call `$/debug` on the server:

```
User: show me the daemon internals
AI: [calls collab_doctor or issues $/debug via sync transport]
```

### Common Issues

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Connection refused | Server not running | `mae-daemon` or `SPC C s` |
| No peers visible | Wrong `collab-server-address` | Check all clients use same address |
| Stale state after restart | WAL replay needed | Automatic; check logs for errors |
| Slow sync | Peer write timeout | Increase `collab-write-timeout-ms` |
| WAL grows unbounded | Compaction threshold too high | Lower `collab-wal-threshold` |

### WAL Integrity

The daemon appends every `sync/update` to the SQLite WAL **before**
applying it to memory. On restart:

1. Load the latest compacted snapshot (if any).
2. Replay WAL entries newer than the snapshot.
3. Serve from the recovered in-memory state.

If the WAL is corrupted, delete `~/.local/share/mae/collab/state.db` and restart. All
connected clients will push their local state on reconnect, restoring the merged
document.

---

## 8. Security

Three auth modes (`collab.auth.mode` on the daemon, `collab-auth-mode` on the
editor):

| Mode | Mechanism | Use |
|------|-----------|-----|
| `none` | No auth | Trusted loopback only |
| `psk` | Pre-shared key, HMAC-SHA256 mutual handshake | Quick shared-secret setups |
| `key` | **Ed25519 mTLS** — encryption + mutual auth + TOFU pinning + per-KB membership | **Recommended; multi-user / enterprise (§10)** |

| Phase | Mechanism | Status |
|-------|-----------|--------|
| v1 | No auth — trusted LAN / VPN only | Superseded |
| v2 | Pre-shared key (PSK) HMAC-SHA256 | ✅ Shipped (v0.11.0) |
| v3 | **Ed25519 mTLS trusted peers + per-KB membership** (ADR-017) | ✅ Shipped |
| v4 | OAuth 2.0 / OIDC for enterprise SSO | Planned |

**Secrets** never belong in `config.toml`. Use `key` mode (no shared secret), or
a PSK keystore / `collab-psk-command` for `psk` mode.

**Recommendations:**

- Bind to `127.0.0.1` for solo/loopback use (default).
- For multi-machine, use **`key` mode** — it encrypts (mTLS) and authenticates
  each peer. On a trusted LAN it's sufficient; on untrusted networks add a VPN
  (WireGuard, Tailscale).
- `psk`/`none` modes are **plaintext** on the wire — keep them on trusted
  networks or behind a TLS-terminating proxy / VPN.
- Firewall the port (`9473`) from untrusted networks; never bind `0.0.0.0` on a
  public IP without a VPN or firewall rule.

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

1. Client connects to daemon
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

## Disconnect Lifecycle

MAE handles several disconnection scenarios:

### Graceful Quit

When a client runs `:q` or `:collab-disconnect`:
1. Editor sets `pending_collab_intent = Disconnect`
2. Bridge sends TCP close, tears down read/write halves
3. Server detects EOF → calls `track_client_disconnect()` for all session docs
4. Server broadcasts `PeerLeft { peer_count }` to remaining clients
5. Editor clears `collab_doc_id`, `sync_doc`, and `pending_sync_updates` on **all** buffers

### Client Crash / Network Drop

1. Server's `read_message()` returns `Err` or `Ok(None)`
2. Same cleanup as graceful quit (step 3–4 above)
3. Surviving clients see "Peer count: N" or "All other collaborators disconnected"
4. If `collab_reconnect_interval` is set, crashed client auto-reconnects

### Last Client Leaves

When the last client disconnects (`peer_count` reaches 0):
- Server keeps the document in memory (no eviction while `idle_eviction_secs` hasn't elapsed)
- Document state persists in WAL — reconnecting clients get the full state via `sync/resync`
- If `idle_eviction_secs` elapses with no clients, server compacts and evicts the doc from memory
  (but WAL/snapshot remain in SQLite for recovery)

### Reconnection

1. Client detects connection loss → `CollabStatus::Reconnecting`
2. Exponential backoff with `collab_reconnect_interval` base and `collab_reconnect_backoff_factor`
3. On reconnect: re-`initialize`, re-`subscribe`, re-share/re-join previously synced buffers
4. Full state reload via `sync/resync` ensures convergence after partition

### Save Protocol During Disconnect

- If a save is in flight when disconnection occurs, the `SendSaveIntent` / `SendSaveCommitted`
  commands are dropped silently. The local file save (`:w`) has already succeeded at that point.
- Peers will not receive a `save_committed` notification, but the CRDT state is consistent.

---

## 10. Trusted-Peer Mode (key auth + mTLS)

For multi-user / multi-machine collaboration and **long-term shared knowledge
management**, use `key` mode (ADR-017). It gives every peer a stable **Ed25519
identity**, encrypts the channel with **mutual TLS**, pins the daemon on first
connect (**TOFU**), authoritatively attributes edits to the verified identity,
and enforces **per-KB membership** (least privilege). It supersedes `psk` mode's
shared secret. Layout lives under `$XDG_DATA_HOME/mae/collab/` (like `~/.ssh/`):
`id_ed25519`(.pub), `known_hosts`, `authorized_keys`.

### Daemon (the hub)

`~/.config/mae/daemon.toml`:

```toml
[collab]
bind = "0.0.0.0:9473"     # LAN; for untrusted networks tunnel via VPN

[collab.auth]
mode = "key"              # Ed25519 + mTLS (tls = true by default)
```

```bash
mae-daemon identity        # prints the daemon's fingerprint + public key line
mae-daemon --check-config  # shows auth.mode=key, tls, identity, authorized count
```

Share the daemon's **fingerprint** out-of-band so clients can verify the TOFU
prompt.

### Editor setup (each peer) — one command

```bash
mae setup-collab --server 192.168.1.10:9473
```

This is **idempotent**: it generates the peer's Ed25519 identity (if absent),
persists `collab-auth-mode=key` + the server address + `collab-auto-connect` to
`init.scm`, and prints the exact `mae-daemon authorize …` line to hand to the
admin. (`mae --collab-identity` just prints the identity without touching config.)

**Reuse an existing SSH key** (opt-in — convenient if you already manage an
Ed25519 SSH key; it becomes your collab identity):

```bash
mae setup-collab --server 192.168.1.10:9473 --ssh-key ~/.ssh/id_ed25519
```

> Trade-off: reusing one key across SSH and MAE means a compromise of either
> affects both. A dedicated MAE identity (the default) keeps them separate.
> Only unencrypted Ed25519 SSH keys are supported.

### Authorize each peer (daemon host)

The label you assign is the peer's identity for attribution + membership.

```bash
# MAE-native key (from `mae setup-collab` / `mae --collab-identity`):
mae-daemon authorize mae-ed25519 <base64> alice
# ...or import the peer's SSH public key (pairs with editor `--ssh-key`):
mae-daemon authorize --from-ssh-pub /path/to/alice_id_ed25519.pub alice

mae-daemon authorized      # list trusted peers
mae-daemon revoke alice    # per-peer revocation (no secret rotation)
```

### First connect (TOFU)

Launch `mae`; an unknown daemon key triggers a **"Trust Daemon Key? SHA256:…
[y/N]"** dialog — verify it matches `mae-daemon identity`, accept to pin. A
*changed* key later aborts the connection (MITM defense). Headless
(`mae --test`/CI) should set `collab-host-key-policy` to `accept-new`.

### Shared KBs among members

The KB **creator/owner** shares it and manages membership; only members may join
or edit:

```
:kb-share                       # owner shares the active KB
:kb-member-add <kb-id> alice    # owner adds a trusted peer (by label)
:kb-member-remove <kb-id> alice
:kb-join <kb-id>                # members only — non-members are denied
```

The daemon binds the shared collection's creator to the *authenticated* identity
and rejects a spoofed `creator`/cursor name. **Known limitation:** a member can
still smuggle membership edits through a raw collection update; the sanctioned
path is the owner-only `kb-member-*` commands (server-side CRDT field ACLs are
future work).

### Validate it

`make test-collab-mtls-e2e` (single-host mTLS) and
`make test-collab-membership-e2e` (two-editor membership) exercise the full
stack with real daemon + editors.

---

## Known Limitations

- **Large undo produces heavy sync updates.** `reconcile_to()` uses a single yrs
  transaction with an LCS diff — the update is minimal and correct, not a
  full-buffer replacement. However, undoing deletion of N lines means N lines of
  insert ops in a single update, which can be heavy for large undos. Full fix
  requires yrs `UndoManager` integration (Phase F) for CRDT-native inverse
  operations. Fixed in v0.10.4: yrs `UndoManager` with per-user undo stacks
  and cursor position restoration via `CursorMeta`.

---

## See Also

- `docs/adr/002-text-sync-model.md` — text sync decision (ADR-002)
- `docs/adr/006-collaborative-state.md` — state engine architecture (ADR-006)
- `:help concept:collab-architecture` — KB node with data-flow diagram
- `:help concept:collab-workflows` — KB node with per-workflow recipes
- `:help lesson:collab-setup` — step-by-step setup tutorial
- `assets/mae-daemon.service` — systemd unit file
