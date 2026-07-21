# KB Sharing — User Guide

Share your MAE knowledge base with other users for real-time collaborative editing.

## Prerequisites

- Two or more MAE instances (v0.11.0+)
- Network connectivity between peers (LAN or internet)
- `mae-daemon` running (dedicated or embedded)

## Quick Start

### Option A: Dedicated Server

```bash
# On a shared machine:
mae-daemon --bind 0.0.0.0:9473

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
| `:collab-start` | `SPC C s` | Start embedded daemon |
| `:collab-connect <addr>` | `SPC C c` | Connect to daemon |
| `:kb-share [name]` | `SPC C K s` | Share KB (default: primary) |
| `:kb-join <kb-id>` | `SPC C K j` | Join a shared KB |
| `:kb-leave <kb-id>` | `SPC C K l` | Stop syncing (keeps local copy) |
| `:collab-discover` | `SPC C P` | Browse LAN for MAE peers (mDNS) |
| `:collab-status` | `SPC C i` | Show connection + KB sync status |

## Sync Modes

Controlled by the `collab_kb_sync_mode` option:

| Mode | Behavior |
|------|----------|
| `on_save` (default) | Auto-sync when you edit a KB node via `:kb-update` or AI tools |
| `manual` | Only sync when you explicitly run `:collab-sync` (`SPC C y`) |

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
3. Both are uploaded to the daemon
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

Shared KBs are stored under `$XDG_DATA_HOME/mae/kb/shared/<slug>/` (default
`~/.local/share/mae/kb/shared/`). The on-disk internal layout is an
implementation detail and subject to change — treat the directory as opaque and
manage shared KBs through MAE (the `*KB Sharing*` buffer, `:kb-*` commands, or
the Scheme/MCP primitives) rather than by editing files directly.

## Authentication

The daemon's `auth.mode` defaults to `none` (no auth — suitable only for trusted
loopback). For any multi-user / non-loopback deployment, authenticate.

### Recommended: `key` mode (trusted-peer mTLS)

The recommended collab auth is **`key` mode** — Ed25519 trusted-peer identities
with mutual TLS (ADR-017/018). Each peer gets a stable Ed25519 identity, the
channel is encrypted, the daemon is pinned on first connect (TOFU), edits are
attributed to the verified identity, and per-KB membership is enforced. There is
**no shared secret** to distribute.

The one-command client setup is idempotent:

```bash
mae setup-collab --server 192.168.1.10:9473
```

This generates the peer's Ed25519 identity, persists `collab-auth-mode=key` +
the server address + `collab-auto-connect` to `init.scm`, and prints the
`mae-daemon authorize …` line to hand to the admin.

Configure the daemon (`~/.config/mae/daemon.toml`):

```toml
[collab]
bind = "0.0.0.0:9473"

[collab.auth]
mode = "key"            # Ed25519 + mTLS (tls = true by default)
```

The full role/policy lifecycle — Owner/Editor/Viewer roles, the
`restrictive | invite | permissive` join policy, the `*KB Sharing*` management
buffer (`SPC C K m`), epoch fencing, peer authorization, and the Scheme/MCP
primitives — is documented in
[COLLABORATION.md §10 (Trusted-Peer Mode)](COLLABORATION.md#10-trusted-peer-mode-key-auth--mtls).

### Alternative: `psk` mode (pre-shared key)

PSK mutual authentication (HMAC-SHA256) is an alternative for quick shared-secret
setups. The secret must be supplied via a command — **never** a plaintext key in
`config.toml`.

1. **Generate a key:**
   ```bash
   # Option A: random key
   openssl rand -hex 32 > ~/.config/mae/collab-psk.txt

   # Option B: password manager (recommended)
   pass insert mae/collab-psk
   ```

2. **Configure the server** (`~/.config/mae/daemon.toml`):
   ```toml
   [collab.auth]
   mode = "psk"
   psk_command = "cat ~/.config/mae/collab-psk.txt"
   ```

3. **Configure each client** (`init.scm`):
   ```scheme
   (set-option! "collab-server-address" "192.168.1.10:9473")
   (set-option! "collab-auth-mode" "psk")
   (set-option! "collab-psk-command" "cat ~/.config/mae/collab-psk.txt")
   ```

#### How It Works

```
Client → Server: hello + nonce
Server → Client: challenge + HMAC(psk, client_nonce || server_nonce)
Client → Server: response + HMAC(psk, server_nonce || client_nonce)
Server → Client: ok | fail
```

Both sides prove knowledge of the PSK without transmitting it.
Nonces prevent replay attacks. If keys don't match, the connection is rejected
before any data exchange.

### No auth (default)

If `auth.mode = "none"` (the default), connections proceed without
authentication. This is suitable for trusted local networks only.

## Network Configuration

### Ports and Protocols

| Service | Protocol | Port | Direction |
|---------|----------|------|-----------|
| Daemon | TCP | 9473 (default) | Inbound to server |
| mDNS discovery | UDP | 5353 (multicast) | Bidirectional |

### Linux Firewall (firewalld)

```bash
# Allow daemon port (on the machine running mae-daemon):
sudo firewall-cmd --add-port=9473/tcp --permanent

# Allow mDNS for peer discovery (both machines):
sudo firewall-cmd --add-service=mdns --permanent

# Apply:
sudo firewall-cmd --reload
```

### Linux Firewall (iptables/nftables)

```bash
# Daemon:
sudo iptables -A INPUT -p tcp --dport 9473 -j ACCEPT

# mDNS:
sudo iptables -A INPUT -p udp --dport 5353 -d 224.0.0.251 -j ACCEPT
```

### macOS Firewall

macOS allows mDNS by default (Bonjour). For the daemon port:

1. **System Settings → Network → Firewall** → allow incoming connections for `mae-daemon`
2. Or via command line:
   ```bash
   # macOS firewall is typically permissive for user apps.
   # If blocked, add an explicit exception:
   sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add /usr/local/bin/mae-daemon
   sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp /usr/local/bin/mae-daemon
   ```

### Router / WLAN Requirements

- **TCP port 9473** must be reachable between peers (usually automatic on same LAN)
- **Multicast** must be enabled on the WiFi network for mDNS discovery
  - Most home/office routers allow multicast by default
  - Enterprise networks may block multicast — use manual `:collab-connect <ip>:9473` instead
  - If mDNS doesn't work, peers can still connect by IP address

### Verifying Connectivity

```bash
# From client machine, test TCP connectivity to server:
nc -zv 192.168.1.10 9473

# Test mDNS (Linux):
avahi-browse -t _mae-sync._tcp

# Test mDNS (macOS):
dns-sd -B _mae-sync._tcp local.
```

## Cross-Platform Setup (Linux ↔ macOS)

### Step-by-Step: Two Machines on Same WLAN

**On Machine A (Linux, hosting the server):**
```bash
# 1. Install/build MAE
cargo install --path crates/mae
cargo install --path daemon

# 2. Configure server for trusted-peer (key) mode
cat > ~/.config/mae/daemon.toml << 'EOF'
[collab]
bind = "0.0.0.0:9473"

[collab.auth]
mode = "key"
EOF

# 3. Start server
mae-daemon

# 4. Print the daemon's identity (share the fingerprint out-of-band)
mae-daemon identity

# 5. Get your IP
ip addr show | grep "inet " | grep -v 127.0.0.1
# → e.g., 192.168.1.10
```

**On Machine B (macOS, joining):**
```bash
# 1. Install/build MAE
cargo install --path crates/mae

# 2. One-command setup: generates an Ed25519 identity, writes init.scm,
#    and prints the `mae-daemon authorize …` line to send to Machine A's admin.
mae setup-collab --server 192.168.1.10:9473

# 3. On Machine A, authorize Machine B's printed key, e.g.:
#    mae-daemon authorize mae-ed25519 <base64> machine-b

# 4. Launch MAE and connect (accept the TOFU prompt — verify it matches step 4 above)
mae ~/my-notes/
# In editor:
:collab-connect 192.168.1.10:9473
:kb-share default
```

For the PSK alternative, set `[collab.auth] mode = "psk"` + `psk_command` on the
daemon and `collab-auth-mode=psk` + `collab-psk-command` in each client's
`init.scm` (see [Authentication](#authentication) above).

**On Machine A (in MAE):**
```
:collab-connect 127.0.0.1:9473
:kb-join default
```

### P2P Mode (No Dedicated Server)

```bash
# Machine A starts embedded server + shares:
mae ~/my-notes/
:collab-start
:kb-share default

# Machine B discovers via mDNS (same WLAN):
mae
:collab-discover
# Select Machine A from the list → auto-connects
:kb-join default
```

## Troubleshooting

Run `:collab-doctor` (`SPC C D`) for diagnostics.

Common issues:
- **"Not connected"**: Run `:collab-connect <addr>` first
- **"PSK auth failed"**: Ensure both sides use the same key; check `psk_command` output
- **"KB not found"**: Ensure the KB name matches (use "default" for primary)
- **No updates received**: Check that both peers are connected to the same server
- **mDNS not finding peers**: Ensure multicast is enabled on your network; try manual IP
- **Connection refused**: Check firewall rules (port 9473/tcp)
- **Timeout on connect**: Verify the server IP is reachable (`ping`, `nc -zv`)

### Debug Logging

```bash
# Full KB sharing lifecycle:
RUST_LOG="mae::collab_bridge=debug,mae_daemon::handler=debug,mae_kb=debug" mae

# Server-side:
RUST_LOG="mae_daemon=debug" mae-daemon

# mDNS discovery:
RUST_LOG="mae::mdns_discovery=debug" mae
```

## Architecture

KB sharing uses yrs (Yjs Rust port) CRDTs for conflict-free merging:

- `KbNodeDoc`: yrs document per KB node (title as YText, body as YText, tags as YArray)
- `KbCollectionDoc`: manifest listing all nodes in a shared KB
- Transport: JSON-RPC 2.0 over TCP with Content-Length framing
- Protocol methods: `kb/share`, `kb/join`, `kb/node_update`, `kb/leave`
- Authentication: `key` mode (Ed25519 trusted-peer mTLS, recommended) or `psk` mode (mutual HMAC-SHA256 handshake); both optional, before JSON-RPC
- Discovery: mDNS `_mae-sync._tcp.local` service type

See [ADR-005](adr/005-kb-crdt.md) and [ADR-006](adr/006-collaborative-state-engine.md) for design rationale.

---

## Server Deployment (Cloud / VPS)

For persistent availability across devices or over the internet, run
`mae-daemon` on a VPS or home server.

```bash
# Bind to all interfaces on the VPS:
mae-daemon --bind 0.0.0.0:9473

# Or set in daemon.toml:
[collab]
bind = "0.0.0.0:9473"
```

**Important:** `mae-daemon` speaks raw TCP with JSON-RPC framing — it
is NOT an HTTP service. Do not put it behind an HTTP reverse proxy (nginx,
Caddy, etc.). A TCP load-balancer (HAProxy stream mode) is fine.

### Firewall Rules

**ufw (Ubuntu/Debian):**
```bash
sudo ufw allow 9473/tcp comment "MAE daemon"
sudo ufw reload
```

**firewalld (Fedora/RHEL):**
```bash
sudo firewall-cmd --add-port=9473/tcp --permanent
sudo firewall-cmd --reload
```

**nftables (manual):**
```bash
sudo nft add rule inet filter input tcp dport 9473 accept
```

For internet-facing deployments, always authenticate — prefer `key` mode
(`[collab.auth] mode = "key"` in `daemon.toml`), or `psk` as an alternative.
Without auth (`mode = "none"`), anyone who can reach port 9473 can read and write
your shared KB.

---

## Systemd Hardening

The bundled unit file (`assets/mae-daemon.service`) runs as a user
service. For a system-level deployment with additional hardening, create a
drop-in override:

```bash
sudo systemctl edit mae-daemon
```

Example hardened override (`/etc/systemd/system/mae-daemon.d/hardening.conf`):

```ini
[Service]
# Isolate /tmp so the process can't read other services' temp files
PrivateTmp=true

# Mount /usr, /boot, /etc read-only
ProtectSystem=strict

# Allow writes only to the data and config dirs
# (adjust to your service user's XDG dirs, e.g. ~/.local/share/mae and ~/.config/mae)
ReadWritePaths=%h/.local/share/mae %h/.config/mae

# Prevent privilege escalation
NoNewPrivileges=true

# Drop all capabilities
CapabilityBoundingSet=

# Restrict syscalls to a safe subset
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
```

After editing:
```bash
sudo systemctl daemon-reload
sudo systemctl restart mae-daemon
sudo systemctl status mae-daemon
```

Verify it is running and the data directory is writable before connecting clients.

---

## VPN / WireGuard Tunnel

For internet deployments, binding `mae-daemon` to a WireGuard tunnel
interface provides network-level encryption as defense-in-depth alongside
`key`-mode mTLS (or PSK) authentication.

```toml
# daemon.toml — bind to WireGuard interface only
[collab]
bind = "10.0.0.1:9473"   # wg0 address
```

```bash
# Bring up wg0 first, then start the server:
sudo wg-quick up wg0
mae-daemon
```

Clients connect to the WireGuard peer address (`init.scm`):
```scheme
(set-option! "collab-server-address" "10.0.0.1:9473")
(set-option! "collab-auth-mode" "key")   ; or "psk" + collab-psk-command
```

Even without app-level auth, the WireGuard tunnel encrypts all traffic with
Curve25519 + ChaCha20-Poly1305. Using both (WireGuard + `key`/`psk` auth)
provides defense-in-depth: a compromised WireGuard key does not leak KB data
unless the peer identity or PSK is also compromised.

---

## IPv6

`mae-daemon` supports IPv6. To listen on all interfaces (dual-stack):

```toml
[collab]
bind = "[::]:9473"
```

On systems where `IPV6_V6ONLY` is unset (Linux default), `[::]:9473` also
accepts IPv4 connections. On systems where it is set (BSD, some hardened
Linux configs), run two instances or use a dual-stack TCP wrapper.

Clients connect using bracket notation:
```
:collab-connect [2001:db8::1]:9473
```

**mDNS and IPv6:** The `_mae-sync._tcp.local` mDNS record is published for
both A (IPv4) and AAAA (IPv6) addresses when available. Peers on link-local
IPv6 (`fe80::/10`) can discover each other without a router.

---

## Bandwidth and Scale

KB sync uses incremental yrs CRDT updates — only the diff between document
states is transmitted, not the full node body.

| Operation | Typical wire size |
|-----------|------------------|
| Single character insert/delete | ~50–120 bytes |
| Sentence edit (~50 chars) | ~150–400 bytes |
| Full node sync on join | ~1–20 KB per node |
| Collection manifest (100 nodes) | ~5–15 KB |

**Capacity estimates** for a single `mae-daemon` instance on modest
hardware (2 CPU cores, 1 GB RAM):

| Metric | Estimate |
|--------|---------|
| Concurrent connected peers | ~200 |
| Shared KB nodes | ~50,000 (SQLite limit) |
| Update fanout latency (LAN) | <5 ms |
| Update fanout latency (WAN) | RTT + <2 ms server processing |
| SQLite WAL flush interval | 60s (configurable) |

These are design targets, not benchmarks. See ADR-008 for CRDT performance
targets and measurement methodology.

---

## collab-doctor Output Explained

Running `:collab-doctor` (`SPC C D`) produces a structured diagnostic report.
Here is an annotated example:

```
collab-doctor: OK                          ← overall health (OK / WARN / ERROR)

Connection
  server:    192.168.1.10:9473             ← configured server address
  status:    connected                     ← TCP connection state
  auth:      key (Ed25519 mTLS) [verified] ← auth mode + last handshake result
  latency:   4 ms                          ← round-trip to server ($/ping)
  uptime:    2h 14m                        ← time since last successful connect

Shared KBs
  default    [synced]                      ← KB name + sync state
    doc_id:  kb/default/a3f7...            ← server-side document identifier
    nodes:   47                            ← node count in local replica
    pending: 0                             ← edits queued but not yet sent
    last_sync: 2026-05-31T14:22:01Z       ← timestamp of last received update

Offline Queue
  pending updates: 0                       ← edits accumulated while disconnected

mDNS
  status:    active                        ← whether discovery is broadcasting
  service:   _mae-sync._tcp.local          ← registered service name
  peers:     1 discovered                  ← peers seen via mDNS

Issues
  (none)                                   ← specific warnings or errors listed here
```

When `collab-doctor` reports `WARN` or `ERROR`, the **Issues** section lists
actionable items, e.g.:

```
Issues
  WARN: 3 pending updates not sent (disconnected 5m ago)
  → Run :collab-connect to flush the queue
```

---

## mae-daemon doctor

The server binary has its own diagnostics subcommand:

```bash
mae-daemon doctor
```

Example output (illustrative — exact fields and formatting vary by version):
```
mae-daemon doctor
  bind:          0.0.0.0:9473                 [listening]
  auth.mode:     key                          [Ed25519 + mTLS]
  data dir:      ~/.local/share/mae           [writable]
  collab db:     ~/.local/share/mae/collab/state.db  [ok]
  peers:         2 connected
  uptime:        3h 07m

  Self-test:     PASS (ping round-trip: 1ms)
```

The collab database lives at `<data_dir>/collab/state.db`, where `<data_dir>`
defaults to the XDG data dir (`~/.local/share/mae`).

Flags:
- `--check-config` — validate `daemon.toml` without binding a port
- `doctor` — live diagnostics (requires a running instance or starts briefly)

---

## Embedded Server Lifecycle

When you run `:collab-start` (`SPC C s`), MAE spawns an embedded
`mae-daemon` instance as an async task within the editor process.

**Start:**
```
:collab-start
```
- Binds to `0.0.0.0:9473` (or `collab_server_address` port if set)
- Registers `_mae-sync._tcp.local` mDNS record
- Automatically connects the local editor as the first client
- Status line: `[collab:server] [collab:connected]`

**While running:**
- Other peers connect via `:collab-connect <your-ip>:9473`
- All shared buffers and KBs are served from in-process memory + SQLite WAL
- The embedded server and the editor share the same process — no IPC overhead

**Stop:**
There is no `:collab-stop` command. The embedded server exits when MAE exits.
This is by design: the embedded server is a convenience feature for P2P
sessions, not a long-running daemon. For persistent availability, run a
dedicated `mae-daemon` (see [Server Deployment](#server-deployment-cloud--vps) above).

When MAE closes with peers connected, those peers will see a TCP disconnect
within the OS keepalive timeout. They can reconnect to a new session if you
restart MAE and run `:collab-start` again. CRDT state persists in each peer's
local replica, so no data is lost — they will resync on reconnect.
