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

## Authentication (PSK)

MAE supports mutual authentication using a pre-shared key (HMAC-SHA256).
Both the server and all clients must share the same key.

### Setup

1. **Generate a key** (any method):
   ```bash
   # Option A: random key
   openssl rand -hex 32 > ~/.config/mae/collab-psk.txt

   # Option B: password manager (recommended)
   pass insert mae/collab-psk
   ```

2. **Configure the server** (`~/.config/mae/state-server.toml`):
   ```toml
   [auth]
   mode = "psk"
   psk_command = "cat ~/.config/mae/collab-psk.txt"
   # Or plaintext fallback (not recommended):
   # psk = "your-secret-key"
   ```

3. **Configure each client** (`~/.config/mae/config.toml`):
   ```toml
   [collaboration]
   server_address = "192.168.1.10:9473"
   psk_command = "cat ~/.config/mae/collab-psk.txt"
   # Or plaintext fallback:
   # psk = "your-secret-key"
   ```

### How It Works

```
Client → Server: hello + nonce
Server → Client: challenge + HMAC(psk, client_nonce || server_nonce)
Client → Server: response + HMAC(psk, server_nonce || client_nonce)
Server → Client: ok | fail
```

Both sides prove knowledge of the PSK without transmitting it.
Nonces prevent replay attacks. If keys don't match, the connection is rejected
before any data exchange.

### No PSK (Default)

If no PSK is configured, connections proceed without authentication.
This is suitable for trusted local networks only.

## Network Configuration

### Ports and Protocols

| Service | Protocol | Port | Direction |
|---------|----------|------|-----------|
| State server | TCP | 9473 (default) | Inbound to server |
| mDNS discovery | UDP | 5353 (multicast) | Bidirectional |

### Linux Firewall (firewalld)

```bash
# Allow state server port (on the machine running mae-state-server):
sudo firewall-cmd --add-port=9473/tcp --permanent

# Allow mDNS for peer discovery (both machines):
sudo firewall-cmd --add-service=mdns --permanent

# Apply:
sudo firewall-cmd --reload
```

### Linux Firewall (iptables/nftables)

```bash
# State server:
sudo iptables -A INPUT -p tcp --dport 9473 -j ACCEPT

# mDNS:
sudo iptables -A INPUT -p udp --dport 5353 -d 224.0.0.251 -j ACCEPT
```

### macOS Firewall

macOS allows mDNS by default (Bonjour). For the state server port:

1. **System Settings → Network → Firewall** → allow incoming connections for `mae-state-server`
2. Or via command line:
   ```bash
   # macOS firewall is typically permissive for user apps.
   # If blocked, add an explicit exception:
   sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add /usr/local/bin/mae-state-server
   sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp /usr/local/bin/mae-state-server
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
cargo install --path crates/state-server

# 2. Create PSK
mkdir -p ~/.config/mae
openssl rand -hex 32 > ~/.config/mae/collab-psk.txt

# 3. Configure server
cat > ~/.config/mae/state-server.toml << 'EOF'
bind = "0.0.0.0:9473"
[auth]
mode = "psk"
psk_command = "cat ~/.config/mae/collab-psk.txt"
EOF

# 4. Start server
mae-state-server

# 5. Get your IP
ip addr show | grep "inet " | grep -v 127.0.0.1
# → e.g., 192.168.1.10

# 6. Share the PSK with Machine B (secure channel!)
# Copy ~/.config/mae/collab-psk.txt to Machine B
```

**On Machine B (macOS, joining):**
```bash
# 1. Install/build MAE
cargo install --path crates/mae

# 2. Place the same PSK file
mkdir -p ~/.config/mae
# (paste the PSK from Machine A)
echo "the-same-key" > ~/.config/mae/collab-psk.txt

# 3. Configure client
cat >> ~/.config/mae/config.toml << 'EOF'
[collaboration]
server_address = "192.168.1.10:9473"
psk_command = "cat ~/.config/mae/collab-psk.txt"
user_name = "machine-b"
EOF

# 4. Launch MAE and connect
mae ~/my-notes/
# In editor:
:collab-connect 192.168.1.10:9473
:kb-share default
```

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
RUST_LOG="mae::collab_bridge=debug,mae_state_server::handler=debug,mae_kb=debug" mae

# Server-side:
RUST_LOG="mae_state_server=debug" mae-state-server

# mDNS discovery:
RUST_LOG="mae::mdns_discovery=debug" mae
```

## Architecture

KB sharing uses yrs (Yjs Rust port) CRDTs for conflict-free merging:

- `KbNodeDoc`: yrs document per KB node (title as YText, body as YText, tags as YArray)
- `KbCollectionDoc`: manifest listing all nodes in a shared KB
- Transport: JSON-RPC 2.0 over TCP with Content-Length framing
- Protocol methods: `kb/share`, `kb/join`, `kb/node_update`, `kb/leave`
- Authentication: mutual HMAC-SHA256 PSK handshake (optional, before JSON-RPC)
- Discovery: mDNS `_mae-sync._tcp.local` service type

See [ADR-005](adr/adr-005-kb-crdt.md) and [ADR-006](adr/adr-006-collaborative-state-engine.md) for design rationale.
