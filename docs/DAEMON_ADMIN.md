# MAE Daemon — Administration & Maintenance

The `mae-daemon` is MAE's optional background service: KB persistence (CozoDB/SQLite over a
Unix socket) + collaborative editing (CRDT sync over TCP, WAL-first) + a maintenance scheduler.
The editor runs standalone without it; the daemon is the upgrade that gives you a **persistent
shared KB, multi-machine collaboration, and services that outlive an editor session** (ADR-035,
`daemon_mode`).

This is the operator runbook: install, configure, manage trusted peers + keys, monitor, back up,
and troubleshoot. For the collaboration *user* story (sharing a KB, joining, E2E, key backup +
recovery) see [`COLLABORATION.md`](COLLABORATION.md) and [`E2E_ENCRYPTION.md`](E2E_ENCRYPTION.md).

---

## 1. Install & run

```bash
# Build (from the repo)
cd daemon && cargo build --release        # → daemon/target/release/mae-daemon

# Run (reads ~/.config/mae/daemon.toml; XDG-respecting)
mae-daemon                                # KB socket + collab TCP (default 127.0.0.1:9473)

# Overrides
mae-daemon --config /path/daemon.toml
mae-daemon --bind 0.0.0.0:9473            # bind all interfaces (firewall/VPN first — §5)
mae-daemon --data-dir /srv/mae-data
mae-daemon --check-config                 # validate config + exit (no listen)
mae-daemon --version
```

### Systemd (user unit)

`assets/mae-daemon.service` is a ready user unit:

```bash
mkdir -p ~/.config/systemd/user
cp assets/mae-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now mae-daemon
journalctl --user -u mae-daemon -f        # follow logs
```

---

## 2. Configuration (`~/.config/mae/daemon.toml`)

TOML, XDG-compliant. Legacy: auto-reads `state-server.toml` if `daemon.toml` is absent. Start from
`assets/daemon-config.toml`. Every key has a sane default; below are the ones operators touch, with
defaults.

```toml
# --- top level ---
# socket = "$XDG_RUNTIME_DIR/mae-daemon.sock"   # KB query socket
# data_dir = "~/.local/share/mae"               # CozoDB store + WAL live here
log_level = "info"                              # e.g. "mae_daemon=debug,info"
# maintenance_interval_secs = 3600
# sync_interval_secs = 30
# health_interval_secs = 300

[collab]
enabled = true
bind = "127.0.0.1:9473"                         # see §5 before exposing

[collab.auth]
mode = "key"                                    # "none" | "psk" | "key"  (key = recommended)
# psk = ""                                      # psk mode only — prefer psk_command
# psk_command = "pass show mae/psk"             # fetch the PSK from a secret manager
# keystore = "~/.config/mae/trusted_keys"       # psk keystore (multiple keyids)
# authorized_keys = "~/.local/share/mae/collab/authorized_keys"   # key mode trust store
# identity_dir = "~/.local/share/mae/collab"    # where the daemon's id_ed25519 lives
tls = true                                      # key mode: native mTLS (default)

[collab.storage]
backend = "sqlite"
compact_threshold = 500                         # compact a doc after N updates
max_wal_entries = 5000                          # …or N WAL rows
# secure_delete is ON for E2e scrub (see §4)

[collab.sync]
compaction_interval_secs = 60
max_update_size_bytes = 1048576                 # 1 MB — a single update is rejected above this
max_document_size_bytes = 10485760              # 10 MB — WARN-only (CRDT convergence; see §6)
```

### Auth modes

| Mode | Mechanism | Use |
|------|-----------|-----|
| `none` | No auth | Trusted loopback only |
| `psk` | Pre-shared key, HMAC-SHA256 mutual handshake | Quick shared-secret setups |
| `key` | **Ed25519 mTLS** + per-KB membership + TOFU pinning | **Recommended** (multi-user) |

`none`/`psk` are **plaintext on the wire** — keep them on a trusted LAN or behind a VPN. Never put a
secret in `daemon.toml`; use `psk_command` / a keystore.

---

## 3. Identity & trusted peers (`key` mode)

In `key` mode the daemon has its **own** Ed25519 identity, and it only accepts clients whose public
keys you've authorized. This is SSH-style trust-on-first-use + an explicit allow-list.

```bash
# The daemon's own identity (generates on first call). Share the fingerprint out-of-band so
# clients can verify the TOFU prompt.
mae-daemon identity
#   Daemon identity (…/collab/id_ed25519):
#     fingerprint: SHA256:…
#     public key:  mae-ed25519 <b64> daemon
#   ⚠ <backup advisory — losing this key loses the daemon's trusted identity>

# Authorize a client (its public key line — `mae <editor> --collab-identity` prints it).
mae-daemon authorize mae-ed25519 <b64> alice    # label must be unique
mae-daemon authorized                            # list trusted clients (label + fingerprint)
mae-daemon revoke alice                          # by label …
mae-daemon revoke SHA256:<fp>                    # … or by fingerprint
```

> **Back up the daemon's `id_ed25519`** (and each client's). Losing it means re-establishing trust
> with every peer. See [`COLLABORATION.md` §8 "Back up your identity key"](COLLABORATION.md).

### Key rotation (ADR-040)

A peer (or the daemon) can rotate its identity key with the old key still in hand: the editor's
`collab-rotate-identity` cross-signs the successor into every KB it owns (a `Rebind`), and the owner
re-wraps content keys to the new key. **The transport trust root is out-of-band** — after a client
rotates, `mae-daemon authorize` its **new** public key (and you may `revoke` the old one once
confirmed). The client then reconnects with the new key. For a *lost* or *compromised* key (no
self-rotation possible), follow the recovery runbook in `COLLABORATION.md §8`.

---

## 4. Persistence, WAL & at-rest

- **WAL-first.** Every sync update is appended to a SQLite WAL before being applied in memory, then
  compacted into a snapshot at `compact_threshold` updates / `max_wal_entries` rows /
  `compaction_interval_secs`.
- **E2e at-rest scrub.** For an E2e KB the daemon stays **key-blind** (only ciphertext + node-ids at
  rest). On encryption-enable it force-compacts with `secure_delete` so superseded plaintext is
  zeroed from freed pages (verified in CI: the `#171` purge + `compact_scrubs_…` tests).
- **Durability caveat (#77).** The WAL connection runs `synchronous=NORMAL`: a hard power loss can
  lose the last up-to-`compaction_interval_secs` (~60 s) / `max_wal_entries` of *acked* updates.
  CRDT convergence re-heals this **from peers** — but a **solo / authoritative daemon with no live
  peer has no heal source**. For a single-daemon deployment holding irreplaceable data, take regular
  backups (below) and treat the ~60 s window as the durability floor.

---

## 5. Network exposure

```bash
mae-daemon                       # default 127.0.0.1:9473 (loopback — safe)
mae-daemon --bind 0.0.0.0:9473   # all interfaces — ONLY with key mode + a firewall/VPN
```

- Prefer a VPN (WireGuard, Tailscale) over raw exposure; `psk`/`none` are plaintext.
- Firewall the port from untrusted networks. Never bind `0.0.0.0` on a public IP without a firewall
  rule or VPN.
- `mae-daemon doctor` runs connectivity diagnostics.

---

## 6. Monitoring

```bash
mae-daemon doctor                 # diagnostics (config, socket, port, store)
journalctl --user -u mae-daemon   # logs (or the file you redirect to)
ss -tln | grep 9473               # is the collab port listening?  (lsof/netstat fallback)
```

From the editor: `collab-status` / `collab-doctor`, and `kb_health` for KB-level counts.

> **Known gap (#207):** CRDT op-set / membership-log growth is **not** yet surfaced by `doctor` /
> `kb_health`. Op-sets and the membership log are currently grow-only (no compaction of the CRDT
> state itself — see `E2E_ENCRYPTION.md` F8 / ADR-028), so disk + memory track *total-edits-ever*
> rather than live-content size. For a long-lived, high-churn KB, watch the `data_dir` size directly.

---

## 7. Backup & restore

The SQLite store has live `-wal` / `-shm` sidecars and `secure_delete` churn, so **never `cp` the
live DB file** — you can capture a torn/stale state. Use SQLite's consistent online copy:

```bash
# Consistent snapshot of a running daemon's store (safe; SQLite walks a read transaction):
sqlite3 ~/.local/share/mae/<store>.cozo ".backup '/backups/mae-$(date +%F).cozo'"
# or:  sqlite3 <store>.cozo "VACUUM INTO '/backups/mae.cozo'"

# Back up the collab trust material too — these are NOT in the DB. `cp -a` of the whole dir
# captures everything that matters for identity + recovery:
#   id_ed25519            — your identity seed (the root of all access; losing it = losing every KB)
#   authorized_keys       — the daemon's trust allow-list (key mode)
#   known_hosts           — host keys this peer has pinned (TOFU)
#   collections/          — per-KB key-blind collection op-logs (ADR-040 B2): required to RECOVER a
#                           lost identity on a new machine (the recovering key authors its Rebind
#                           against these without re-fetching from the daemon)
#   content_keys/         — recovered per-KB content keys (re-derivable from the op-log, but cached)
#   recovery/             — your registered OFFLINE recovery key, if you ran collab-register-recovery-key
cp -a ~/.local/share/mae/collab/ /backups/mae-collab-$(date +%F)/
# NOTE: for real key separation, keep `recovery/` on SEPARATE offline media from this backup —
# a backup holding BOTH your primary and your recovery key gives a thief either path in. The
# recovery key's purpose is to survive loss/compromise of the primary; co-locating them defeats it.

# Restore: stop the daemon, replace the store file + the collab dir, restart.
systemctl --user stop mae-daemon
cp /backups/mae-2026-06-30.cozo ~/.local/share/mae/<store>.cozo
cp -a /backups/mae-collab-2026-06-30/ ~/.local/share/mae/collab/
systemctl --user start mae-daemon
```

Recovery from a corrupt snapshot degrades to the WAL and "heals via re-sync" from a peer if one
exists (see §4). Keep backups for the solo-daemon case.

---

## 8. Troubleshooting

| Symptom | Check |
|---------|-------|
| Daemon won't start / "another daemon is listening" | a stale socket or a running instance — `ss -tln`, remove a stale `socket` path |
| Client can't connect (`key` mode) | the client's key is `authorize`d (`mae-daemon authorized`); host-key TOFU pinned on the client (`known_hosts`); ports/firewall (§5) |
| Client rejected after rotating its key | `authorize` its **new** public key (ADR-040, §3) |
| "rebase required" after rotation | expected once — the rotated key has a new write lineage; `collab-fence-resolution = auto` re-authors silently (ADR-023/040) |
| E2e content unreadable on a peer | the owner must have approved + wrapped the key to that member; a member who re-syncs from scratch after a key rotation loses pre-rotation content (no key-history yet, #176) |
| Store growing fast | grow-only CRDT state (§6 / #207) — watch `data_dir`; compaction of the CRDT itself is tracked (ADR-028) |

See also: [`COLLABORATION.md`](COLLABORATION.md), [`E2E_ENCRYPTION.md`](E2E_ENCRYPTION.md),
[`SECURITY_REVIEW.md`](SECURITY_REVIEW.md), and ADR-035 (editor↔daemon boundary).
