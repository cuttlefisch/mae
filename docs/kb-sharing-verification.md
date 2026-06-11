# KB Sharing — Cross-Device Verification Procedure

Step-by-step tests for verifying KB sharing across two machines. Run these
after any significant change to the collab stack, before a release, or when
diagnosing a reported issue.

Notation: **Machine A** hosts or shares; **Machine B** joins or receives.
All commands shown in the MAE command bar (`:command`) or as keybindings.

---

## Test 1: Dedicated Server + PSK Auth

Verifies the full server-backed workflow including mutual authentication,
real-time KB sync, and clean leave.

### Prerequisites

- `mae-daemon` binary available on Machine A (or a shared server host)
- Both machines have MAE v0.11.0+ installed
- Port 9473/tcp open between the two machines
- A shared PSK file distributed via a secure channel beforehand

### Steps

**1.1 — Generate PSK and configure server (Machine A)**

```bash
# Generate PSK
openssl rand -hex 32 > ~/.config/mae/collab-psk.txt

# Write server config
cat > ~/.config/mae/daemon.toml << 'EOF'
bind = "0.0.0.0:9473"
[auth]
mode = "psk"
psk_command = "cat ~/.config/mae/collab-psk.txt"
EOF

# Start server
mae-daemon
```

Expected output (server terminal):
```
INFO mae_daemon: listening on 0.0.0.0:9473
INFO mae_daemon: PSK auth enabled (HMAC-SHA256)
```

**1.2 — Configure client PSK (Machine B)**

```bash
mkdir -p ~/.config/mae
# Paste the PSK from Machine A's collab-psk.txt
echo "<key-from-machine-a>" > ~/.config/mae/collab-psk.txt

cat >> ~/.config/mae/config.toml << 'EOF'
[collaboration]
server_address = "192.168.1.10:9473"
psk_command = "cat ~/.config/mae/collab-psk.txt"
user_name = "machine-b"
EOF
```

**1.3 — Connect from Machine A**

In MAE on Machine A:
```
:collab-connect 127.0.0.1:9473
```

Expected output (minibuffer):
```
Connected to 127.0.0.1:9473 (PSK auth OK)
```

Status line updates to: `[collab:connected]`

**1.4 — Share KB from Machine A**

```
:kb-share default
```

Expected output:
```
Shared KB "default" (doc_id: kb/default/<hash>)
```

Status line updates to: `[KB:1|synced] [collab:connected]`

**1.5 — Connect and join from Machine B**

```
:collab-connect 192.168.1.10:9473
:kb-join default
```

Expected output after `:collab-connect`:
```
Connected to 192.168.1.10:9473 (PSK auth OK)
```

Expected output after `:kb-join default`:
```
Joined KB "default" — N nodes loaded
```

Status line on Machine B: `[KB:1|synced] [collab:connected]`

**1.6 — Edit a node on Machine A, verify on Machine B**

On Machine A, open a KB node and edit it. Using the AI tool or `:kb-update`:
```
:kb-update node-id "Updated content from Machine A"
```

On Machine B, within 1–2 seconds the node should reflect the change.
Verify by running:
```
SPC C i    (collab-status)
```

Expected output (Machine B):
```
Collab Status
  Server:    192.168.1.10:9473  [connected]
  Auth:      PSK (HMAC-SHA256)
  Shared KBs:
    default  [synced]  last_update: <timestamp>
  Pending:   0 updates
```

Open the same node on Machine B and confirm the body matches.

**1.7 — Leave KB on Machine B**

```
:kb-leave default
```

Expected output:
```
Left KB "default" — local copy retained at $XDG_DATA_HOME/mae/kb/shared/default/
```

Status line on Machine B returns to: `[collab:connected]` (no KB indicator)

### Cleanup

```bash
# Machine A: stop server
Ctrl-C  (in the mae-daemon terminal)

# Both machines: disconnect
:collab-disconnect   (or close MAE)
```

---

## Test 2: Embedded Server (P2P)

Verifies that `:collab-start` spins up a working embedded server and that KB
sync works over that embedded instance without a dedicated server process.

### Prerequisites

- Both machines on the same network, port 9473/tcp accessible
- Machine A's LAN IP known (run `ip addr` or `ifconfig`)
- No PSK required for this test (LAN trust assumed)

### Steps

**2.1 — Start embedded server on Machine A**

```
:collab-start
```

Expected output:
```
Embedded daemon started on 0.0.0.0:9473
```

Status line: `[collab:server] [collab:connected]`

**2.2 — Share a buffer from Machine A**

Open any file, then:
```
SPC C S    (collab-share)
```

Expected output:
```
Buffer shared: doc_id = file/<basename>
```

**2.3 — Connect from Machine B and verify buffer sync**

```
:collab-connect 192.168.1.10:9473
```

Expected:
```
Connected to 192.168.1.10:9473 (no auth)
```

On Machine B, open the shared buffer:
```
:collab-join file/<basename>
```

Type a character on Machine A. Within 1 second it should appear on Machine B.

**2.4 — Share KB from Machine A**

```
:kb-share default
```

Expected:
```
Shared KB "default" (doc_id: kb/default/<hash>)
```

**2.5 — Join KB from Machine B**

```
:kb-join default
```

Expected:
```
Joined KB "default" — N nodes loaded
```

**2.6 — Verify KB sync**

On Machine A, create a new KB node:
```
:kb-create "Test node from P2P" "Body text for verification"
```

Expected output:
```
Created node: <node-id>
```

On Machine B, search for the new node:
```
SPC m f    (kb-find)
Test node from P2P
```

The node should appear in the results with the body text from Machine A.

### Cleanup

Close MAE on Machine A. The embedded server stops automatically with the editor process. Machine B will show a disconnected status within the TCP keepalive timeout (~30s) or immediately if you run `:collab-disconnect`.

---

## Test 3: mDNS Discovery

Verifies that two MAE instances on the same WLAN can find each other without
manually specifying an IP address.

### Prerequisites

- Both machines on the **same WLAN** (not isolated AP/VLAN)
- Multicast traffic (UDP 5353) not blocked by the router
- `avahi-daemon` running on Linux (or Bonjour on macOS — enabled by default)

### Steps

**3.1 — Start server on Machine A**

```
:collab-start
```

Machine A registers a `_mae-sync._tcp.local` mDNS record. Verify at the OS
level (optional):

```bash
# Linux:
avahi-browse -t _mae-sync._tcp

# macOS:
dns-sd -B _mae-sync._tcp local.
```

Expected OS-level output (Linux):
```
= wlan0 IPv4 mae-<hostname> _mae-sync._tcp local
   hostname = [<hostname>.local]
   address = [192.168.1.10]
   port = [9473]
```

**3.2 — Discover peer from Machine B**

```
:collab-discover
```

Expected: a picker/palette opens listing discovered MAE peers, e.g.:

```
MAE Peers (mDNS)
> machine-a.local — 192.168.1.10:9473
```

**3.3 — Select peer and verify auto-connect**

Press Enter on the listed peer.

Expected output:
```
Connected to 192.168.1.10:9473 (via mDNS)
```

Status line on Machine B: `[collab:connected]`

**3.4 — Join KB and verify**

```
:kb-join default
```

Expected:
```
Joined KB "default" — N nodes loaded
```

Run `:collab-status` (`SPC C i`) to confirm:

```
Collab Status
  Server:    machine-a.local (192.168.1.10:9473)  [connected]
  Discovery: mDNS
  Shared KBs:
    default  [synced]
```

### Cleanup

```
:collab-disconnect   (Machine B)
```

Machine A's embedded server continues running until MAE closes.

---

## Test 4: PSK Auth Failure and Recovery

Verifies that a wrong PSK is rejected before any data exchange, and that
correcting the key allows a successful reconnect.

### Prerequisites

- `mae-daemon` running on Machine A with a known PSK
- Machine B configured with an intentionally wrong PSK

### Steps

**4.1 — Start server with correct PSK (Machine A)**

```bash
echo "correct-psk-value" > ~/.config/mae/collab-psk.txt

cat > ~/.config/mae/daemon.toml << 'EOF'
bind = "0.0.0.0:9473"
[auth]
mode = "psk"
psk = "correct-psk-value"
EOF

mae-daemon
```

Expected (server):
```
INFO mae_daemon: listening on 0.0.0.0:9473
INFO mae_daemon: PSK auth enabled (HMAC-SHA256)
```

**4.2 — Configure wrong PSK on Machine B**

```bash
cat >> ~/.config/mae/config.toml << 'EOF'
[collaboration]
server_address = "192.168.1.10:9473"
psk = "wrong-key"
EOF
```

**4.3 — Attempt connection with wrong key**

```
:collab-connect 192.168.1.10:9473
```

Expected minibuffer error:
```
Connection failed: PSK auth rejected by server
```

Status line: no collab indicator (connection was not established).

Server log should show:
```
WARN mae_daemon::auth: PSK auth failed for <Machine B IP> — HMAC mismatch
INFO mae_daemon: connection from <Machine B IP> rejected
```

No editor state or KB data was exchanged.

**4.4 — Fix the PSK on Machine B**

Edit `~/.config/mae/config.toml` to use the correct key:
```toml
[collaboration]
psk = "correct-psk-value"
```

Or use `psk_command` pointing at a file containing the correct value.

**4.5 — Reconnect with correct key**

```
:collab-connect 192.168.1.10:9473
```

Expected:
```
Connected to 192.168.1.10:9473 (PSK auth OK)
```

Status line: `[collab:connected]`

**4.6 — Verify normal operation resumes**

```
:kb-join default
```

Expected:
```
Joined KB "default" — N nodes loaded
```

Run `:collab-doctor` (`SPC C D`) to confirm clean state:
```
collab-doctor: OK
  auth:     PSK (HMAC-SHA256) [verified]
  KBs:      1 joined, 0 pending updates
```

### Cleanup

```bash
# Machine A: stop server
Ctrl-C
```

---

## Test 5: Offline Edit + Reconnect (CRDT Merge)

Verifies that edits made while disconnected are queued and correctly merged
via CRDT when the connection is restored, without data loss or duplication.

### Prerequisites

- Both machines connected to the same server, KB joined on both
- `collab_kb_sync_mode` set to `on_save` (default) on both

### Steps

**5.1 — Establish baseline (both machines connected)**

On Machine A:
```
:collab-connect 192.168.1.10:9473
:kb-share default
```

On Machine B:
```
:collab-connect 192.168.1.10:9473
:kb-join default
```

Confirm both are synced:
```
SPC C i    (collab-status on each machine)
```

Expected on both:
```
  Shared KBs:
    default  [synced]  pending: 0
```

**5.2 — Create a baseline node**

On Machine A:
```
:kb-create "Offline Test Node" "Initial content. Line two."
```

Wait for Machine B's status to show `[synced]` (usually <2s). Confirm node exists on Machine B via `:kb-find`.

**5.3 — Disconnect Machine B**

```
:collab-disconnect
```

Expected:
```
Disconnected from 192.168.1.10:9473
```

Status line on Machine B: `[KB:1|offline|0pending]`

**5.4 — Edit on Machine A while B is offline**

On Machine A, update the node:
```
:kb-update <node-id> "Machine A offline edit — appended while B was away."
```

Machine A's update is sent to the server immediately. Machine B does not receive it (offline).

**5.5 — Edit the same node on Machine B while offline**

On Machine B, update the same node:
```
:kb-update <node-id> "Machine B offline edit — concurrent change."
```

Status line on Machine B: `[KB:1|offline|1pending]`

The edit is queued locally.

**5.6 — Reconnect Machine B**

```
:collab-connect 192.168.1.10:9473
```

Expected:
```
Connected to 192.168.1.10:9473 (PSK auth OK)
Draining 1 pending KB update(s)...
```

The pending queue is flushed automatically. The server receives Machine B's
CRDT update and fans it out. Machine A receives the update.

**5.7 — Verify CRDT merge on both machines**

On both Machine A and Machine B, retrieve the node:
```
:kb-find "Offline Test Node"
```

The body should contain **both** edits merged by the CRDT (yrs YATA algorithm).
Neither edit is lost. The exact merge output depends on character-level
interleaving, but both strings will be present:

```
Initial content. Line two.Machine A offline edit — appended while B was away.Machine B offline edit — concurrent change.
```

(Ordering of the two appended segments is deterministic but depends on peer
ID ordering — both orderings are valid CRDT outcomes. What matters is that
neither edit is dropped.)

Status line on both machines: `[KB:1|synced]`

Run `:collab-doctor` on Machine B:
```
SPC C D
```

Expected:
```
collab-doctor: OK
  connection:  192.168.1.10:9473  [connected]
  auth:        PSK (HMAC-SHA256)  [verified]
  KBs:         1 joined
    default    [synced]  pending: 0  last_sync: <timestamp>
  queue:       empty
```

### Cleanup

```
:kb-leave default     (Machine B)
:collab-disconnect    (Machine B)
# Machine A: stop daemon
Ctrl-C
```

---

## Quick Reference: Commands Used in These Tests

| Command | Keybinding | Purpose |
|---------|-----------|---------|
| `:collab-start` | `SPC C s` | Start embedded daemon |
| `:collab-connect <addr>` | `SPC C c` | Connect to daemon |
| `:collab-disconnect` | — | Disconnect from server |
| `:collab-discover` | `SPC C P` | mDNS peer browser |
| `:collab-share` | `SPC C S` | Share current buffer |
| `:collab-status` | `SPC C i` | Connection + KB sync status |
| `:collab-doctor` | `SPC C D` | Full diagnostics |
| `:kb-share [name]` | `SPC C K s` | Share a KB |
| `:kb-join <kb-id>` | `SPC C K j` | Join a shared KB |
| `:kb-leave <kb-id>` | `SPC C K l` | Leave a shared KB |
| `:kb-find` | `SPC m f` | Search KB nodes |
| `:kb-create <title> <body>` | — | Create a new KB node |
| `:kb-update <id> <body>` | — | Update a KB node |

## Config Options Referenced

| Option | Config Key | Default | Description |
|--------|-----------|---------|-------------|
| `collab_server_address` | `[collaboration] server_address` | — | Daemon address |
| `collab_psk` | `[collaboration] psk` | — | Plaintext PSK (prefer `psk_command`) |
| `collab_psk_command` | `[collaboration] psk_command` | — | Shell command that prints the PSK |
| `collab_kb_sync_mode` | `[collaboration] kb_sync_mode` | `on_save` | `on_save` or `manual` |
| `collab_user_name` | `[collaboration] user_name` | hostname | Peer display name |
