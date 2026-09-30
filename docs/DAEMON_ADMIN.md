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

# Overrides — these select WHICH instance every command below operates on.
mae-daemon --config /path/daemon.toml
mae-daemon --bind 0.0.0.0:9473            # bind all interfaces (firewall/VPN first — §5)
mae-daemon --oauth-bind 0.0.0.0:8443      # the OAuth HTTPS listener's port (§2)
mae-daemon --data-dir /srv/mae-data
mae-daemon --socket /run/mae/prod.sock    # KB query socket
mae-daemon --check-config                 # validate config + exit (no listen)
mae-daemon --version
```

`--config` applies to **every** subcommand, not just to starting the daemon —
`doctor`, `--check-config`, `keygen`, `keys`, `identity`, `authorized`,
`authorize`, `revoke` and `token mint` all operate on the instance it names:

```bash
mae-daemon doctor      --config ~/.config/mae/daemon-prod.toml
mae-daemon authorize   --config ~/.config/mae/daemon-staging.toml "$(cat peer.pub)" laptop
```

> Before v0.15 those subcommands silently read the default
> `~/.config/mae/daemon.toml` no matter what `--config` said. If you have scripts
> written against that behaviour, they were operating on the default instance —
> re-check especially any `authorize` calls, which would have trusted the peer on
> the wrong instance.

### Systemd (user unit)

`assets/mae-daemon.service` is a ready user unit:

```bash
mkdir -p ~/.config/systemd/user
cp assets/mae-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now mae-daemon
journalctl --user -u mae-daemon -f        # follow logs
```

### Multi-tenant deployment: shared process vs. process-per-tenant (ADR-060)

Two supported deployment shapes, not one — pick based on the isolation guarantee a given
tenant actually needs:

- **Shared process (default, `mae-daemon.service` above).** Multiple tenants — e.g. several
  teams inside the same trusted organization — share one daemon process. Phase A-D give this
  real, software-enforced isolation: per-tenant instance addressing (never resolves another
  tenant's data), per-tenant cost-weighted request-points budgets + connection caps, and
  tenant-boundary role composition that never leaks across KBs. Efficient (one process, shared
  cache), and sufficient for the common case.
- **Process-per-tenant (`assets/mae-daemon@.service`, a systemd *template* unit).** For a
  tenant whose operator needs "if this tenant's daemon crashes or gets OOM-killed, it must
  not take any other tenant down with it" — a guarantee Phase A-D's in-process isolation,
  however correct, cannot give (they're still one process, one failure domain). Each
  instantiation gets its own PID, its own `daemon.toml`, its own data directory:

  ```bash
  # One-time per instance. `daemon-instance-config.toml` is the MULTI-instance
  # template — it already sets every path that must be unique (see "What must
  # be unique per instance" below). `daemon-config.toml` is the single-daemon one.
  cp assets/daemon-instance-config.toml ~/.config/mae/daemon-acme-corp.toml
  # replace INSTANCE with the name; pick ports no other instance uses

  cp assets/mae-daemon@.service ~/.config/systemd/user/
  systemctl --user daemon-reload
  systemctl --user enable --now 'mae-daemon@'"$(systemd-escape acme-corp)"'.service'
  journalctl --user -u 'mae-daemon@*' -f    # follow all tenant instances' logs
  ```

  **Linux-only** — mae-daemon is confirmed never expected to run on macOS or Windows as a
  deployed service, so there is no `launchd`/Service-Control-Manager equivalent of this unit.

Both shapes can run simultaneously: e.g. most tenants sharing one `mae-daemon.service`
process, with one tenant that has stricter isolation requirements split out to its own
`mae-daemon@acme-corp.service` instance.

#### What must be unique per instance

**Eight** resources, not four. Setting `data_dir` scopes the first four and
**does not** scope the last three:

| Setting | Scoped by `data_dir`? |
|---|---|
| `socket` | no — set it explicitly (also `--socket`) |
| `data_dir` | — (also `--data-dir`) |
| `collab.storage.data_dir` | **yes**, defaults to `<data_dir>/collab` |
| `collab.bind` | no — set it explicitly (also `--bind`) |
| `oauth.bind` | no — set it explicitly (also `--oauth-bind`) |
| `collab.auth.identity_dir` | **no** — shared `$XDG_DATA_HOME/mae/collab/` |
| `collab.auth.authorized_keys` | **no** — shared `$XDG_DATA_HOME/mae/collab/authorized_keys` |
| `collab.auth.keystore` | **no** — shared `$XDG_DATA_HOME/mae/collab/trusted_keys` |

> **The one that bites.** Identity, `authorized_keys` and the keystore default to
> a *shared* location regardless of `data_dir`. Two instances distinguished only
> by data dir and ports therefore read **one** `authorized_keys` — so
> `mae-daemon authorize --config daemon-staging.toml <key>` also grants that peer
> access to **production**. Set all three explicitly per instance.
>
> The shared default is deliberate for the ordinary one-host-one-identity case
> and is not changed: relocating an existing operator's identity key would lose
> access to every shared KB, with no recovery (§3).

Check it rather than trusting the table — `--compare-with` exits non-zero if the
two instances share anything, so it works in a deploy gate:

```bash
mae-daemon doctor --config ~/.config/mae/daemon-staging.toml \
                  --compare-with ~/.config/mae/daemon-prod.toml
#   side-by-side: OK — shares no resource with the compared instance
```

Each instance's own resources are listed by `mae-daemon doctor` and
`mae-daemon --check-config`, so two reports can also just be diffed.

---

### Running in a container (one instance)

MAE publishes a static musl `mae-daemon` in each release tarball (`mae-linux-x86_64.tar.gz` +
`.sha256`); a container image is a thin wrapper around it, built and deployed by digest. What the
image and its compose service must get right:

- **Identity in a named volume, never in an image layer.** `id_ed25519` is generated on first start
  (or by `mae-daemon identity`) and **cannot be recovered**: losing it loses every KB the instance
  shares. Mount `identity_dir` from a named volume (mode 0700) and back it up (§7: a backup
  sidecar running `mae-daemon backup create` against the same volumes covers it). `--check-config`
  does not create it, so validating a config as root cannot leave a root-owned key behind.
- **Run as a non-root user**, with `data_dir`, `identity_dir` and the KB socket inside volumes it
  owns. Logs go to stdout/stderr.
- **Key-mode auth, explicitly.** A non-loopback `collab.bind` requires it anyway; say so in config.
- **Health:** `mae-daemon ping --config …` is the liveness probe (exit 0 iff the instance answers on
  its KB socket) — use it as the `HEALTHCHECK`. `mae-daemon doctor` is the **readiness** gate for a
  deploy step, not a liveness probe (§6). A proxy or platform that probes over HTTPS uses the
  listener's unauthenticated `GET /api/health` instead (§6).
- **Exposure:** remote clients reach the daemon through its HTTPS listener (ADR-052), which may sit
  behind a TLS-terminating proxy with the upstream re-encrypted; keep the collab port on the
  stack's private network (§5, ADR-111).

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
# max_documents = 4096                          # working-set cap: ONE yrs doc per KB node
                                                #   (kb:{node}) + one kbc:{kb} doc. Set ABOVE
                                                #   your largest KB's node count to avoid
                                                #   reload churn during sync. LRU cap only —
                                                #   raising it costs memory only when exceeded.
max_update_size_bytes = 4194304                 # 4 MiB — a single update is REJECTED above this
                                                #   (DoS bound). Raise for KBs with large
                                                #   individual nodes (a node's full-state push
                                                #   on reseal/share must fit under it).
max_document_size_bytes = 10485760              # 10 MB — WARN-only (CRDT convergence; see §6)
```

> [!TIP]
> **Tuning for a large KB.** Each KB *node* is its own CRDT document, so a KB with N nodes
> is ~N+1 documents. For a multi-thousand-node KB set `max_documents` above N (default 4096
> covers a few thousand). If a large node fails to sync with an "update too large" error,
> raise `max_update_size_bytes`. Both are safe to raise — `max_documents` is a memory/LRU
> cap, `max_update_size_bytes` is a per-message allocation bound.

### Auth modes

| Mode | Mechanism | Use |
|------|-----------|-----|
| `none` | No auth | Trusted loopback only — **this is the default** |
| `psk` | Pre-shared key, HMAC-SHA256 mutual handshake | Quick shared-secret setups |
| `key` | **Ed25519 mTLS** + per-KB membership + TOFU pinning | **Recommended** (multi-user) |

> **The default is `none`, not `key`.** A `daemon.toml` that says nothing about
> `[collab.auth]` accepts any client that can reach the port. Combined with
> `--bind 0.0.0.0` (§5) that is an open server. `mae-daemon doctor` and
> `--check-config` both call this out explicitly; set `mode = "key"` before
> binding anywhere but loopback.

`none`/`psk` are **plaintext on the wire** — keep them on a trusted LAN or behind a VPN. Never put a
secret in `daemon.toml`; use `psk_command` / a keystore.

### The HTTPS listener (`[oauth]`, ADR-052/053/111)

The listener remote clients reach — the read-only `kb/query.*` surface, the optional HTML KB view,
and `GET /api/health`. Off by default. The keys operators touch:

```toml
[oauth]
enabled = true
bind = "127.0.0.1:9474"                         # also --oauth-bind
canonical_resource_uri = "https://mae.example.com"   # REQUIRED — every token's `aud`
cert_path = "/etc/mae/tls/tls.crt"              # PEM chain; re-read when it changes (below)
key_path  = "/etc/mae/tls/tls.key"
kb_query_enabled = true                         # the kb/query.* surface (needs [collab] enabled)
# jwks_url = "https://idp.example.com/jwks.json" # external issuer — OPTIONAL (below)
# issuer = "https://idp.example.com"
self_issued_tokens_enabled = true               # accept this daemon's own tokens (key mode only)
self_issued_token_ttl_secs = 3600               # default lifetime of `token mint`
self_issued_token_max_ttl_secs = 86400          # ceiling `token mint --ttl` refuses to exceed
# kb_query_max_scan_nodes = 500                 # search/agenda/health scan cap — see below
# webview_enabled = false                       # GET /kb/<id>/view
```

- **`jwks_url` is optional** when the daemon's own self-issued tokens are the only ones in use.
  With it unset no JWKS client exists: a token from an external issuer is refused with `401` and a
  reason naming `jwks_url` (not a `503` — nothing is temporarily unavailable). With **neither**
  `jwks_url` nor self-issued tokens available (they need `self_issued_tokens_enabled = true` and
  `collab.auth.mode = "key"`), no token could ever validate, and the listener is not started — the
  log says why.
- **Certificates are reloaded without a restart.** The listener stats `cert_path`/`key_path` on each
  TLS handshake and re-reads them when either file's mtime or size changes, so a renewal (atomic
  replace, symlink swap or in-place write) is served on the next connection. A pair that does not
  load — unparseable, empty, or a certificate that does not match the key, as mid-renewal — is
  **ignored with a warning** and the previous certificate stays in service; handshakes never fail
  because of it. The initial pair at startup must load.
- **Scan caps are reported, not hidden.** `kb/query.search` (like `links`, `titles`, `agenda` and
  `health`) examines at most `kb_query_max_scan_nodes` nodes and answers `"truncated": true` (plus
  the KB's `"total"`) when that cap, rather than the caller's result limit, ended the scan. A
  `RemoteHub` client reports such a result as partial. Raise the cap for KBs larger than it.
- **The HTML KB view takes its token in the URL fragment**:
  `https://<host>/kb/<kb_id>/view#access_token=<token>`. The fragment is never sent to the server,
  so it cannot land in a proxy access log; the page moves it into an `Authorization` header and
  removes it from the address bar. The old `?access_token=` form is refused with `400`. The page
  itself is a content-free shell; access is checked by its first request (`kb/query.capabilities`),
  and a principal without access sees that refusal and no node list.

#### Issuing a token: `mae-daemon token mint`

For a client without collab mTLS — a headless service account, a script, a person handed a view
link — the operator mints a self-issued token (EdDSA, signed by this daemon's identity, `aud` =
`canonical_resource_uri`):

```bash
mae-daemon token mint --config /etc/mae/daemon.toml --sub svc:indexer --ttl 8h \
  | pass insert -e mae/hub-token          # stdout is the token and nothing else
```

- `--sub` (required) is the principal the token maps to; `kb/query.*` authorizes it against each
  KB's membership exactly like any other principal. Add that principal to the KBs it should read.
- `--ttl` takes seconds or `s`/`m`/`h`/`d` (`900`, `15m`, `8h`, `1d`). Default:
  `self_issued_token_ttl_secs`. Above `self_issued_token_max_ttl_secs` the command **refuses**
  rather than clamping.
- It refuses — and prints nothing on stdout — when the listener would not accept the result
  (`oauth.enabled`, `self_issued_tokens_enabled`, `canonical_resource_uri`, key-mode collab), and
  when the daemon has no identity yet. It **never creates** an identity (start the daemon once, or
  run `mae-daemon identity`, first). Diagnostics go to stderr.
- It reads the daemon's private key, so it is an operator command only: it is deliberately not an
  MCP tool or an RPC. Over mTLS, a member can still obtain a token for **its own** fingerprint
  (`kb/query.self_token`).

**Revocation, honestly:** there is no per-token revocation list. A self-issued token is valid until
its `exp`, or until the daemon's identity key is replaced (§3, *Key rotation*) — which invalidates
**every** self-issued token at once and is a heavyweight operation in its own right, since every
client has pinned that key. Removing the `--sub` principal from a KB's membership stops that KB
being readable with the token immediately, but the token still authenticates. Keep lifetimes short
and re-mint on a schedule; that is what the maximum is for.

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

## 3b. P2P mesh (ADR-025) — daemon-to-daemon, no central hub

The mesh lets two daemons sync a KB **directly over iroh QUIC** — no shared relay server. The
daemon's node identity IS its `key`-mode Ed25519 identity (§3), so the same `authorize`/`revoke`
allow-list gates the mesh. **Beta** (validated two-daemon convergence; gossip/anti-entropy
multi-way sync is a follow-up, #89).

### Enable it

```toml
[collab]
bind = "127.0.0.1:9473"     # the editor still connects to ITS daemon over this TCP socket
[collab.auth]
mode = "key"                # the mesh has no PSK/anonymous path — key mode is required
[collab.p2p]
enabled = true
relay = "disabled"          # direct addressing (LAN / localhost). "default" = public iroh
                            # relays (NAT hole-punch, needs internet); or a self-hosted URL.
connection_gate = "authorized_keys"   # only authorized peer daemons (vs "open" TOFU)
```

`mae setup-collab --p2p` writes this for you. `relay = "disabled"` needs no external infra and
is ideal where peers can reach each other directly; use `"default"` to traverse NAT.

### Authorize the *peer daemon* (not just its editor)

The mesh dialer connects as the **daemon's** identity, so on each side `authorize` the OTHER
daemon's public key (read it with `mae-daemon identity`), in addition to your own editor:

```bash
mae-daemon identity                                   # this daemon's pubkey + fingerprint
mae-daemon authorize mae-ed25519 <peer-daemon-pubkey> peerB   # trust the peer daemon
```

### Share → join → approve

1. **Owner** (daemon A side): in the editor, `kb-share-p2p` — this establishes the mesh share
   (`establish_p2p_share` widens the KB's transport to include the mesh) and prints a
   `mae://join/…` **ticket** to `*Messages*`. (Two-step beta path: if the KB isn't on the daemon
   yet, `kb-share` it first; single-command upload is a follow-up.)
2. **Joiner** (daemon B side): `:kb-join-p2p mae://join/…` — daemon B's dialer connects to
   daemon A over iroh and requests the KB.
3. **Owner approves** the joining peer: the mesh join is owner-gated — approve the peer
   **daemon's fingerprint** (`kb-approve <kb> SHA256:… editor`), or set a `permissive` policy
   for auto-admit. The next dialer cycle (polls ~10s) pulls the KB; edits then sync live both
   ways, peer-verified (signed ops, epoch fence).

A full-process two-daemon convergence test is CI-gated: `scripts/collab-p2p-mesh-e2e.sh`.

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
- **Behind a TLS-terminating reverse proxy** the collab port cannot work: it authenticates by the
  client's certificate, which a terminating proxy consumes. Expose the HTTPS listener instead
  (ADR-052; re-encrypt upstream) and keep collab private — ADR-111 records why remote clients
  authenticate at the application layer rather than by mutual TLS.

---

## 6. Monitoring

```bash
mae-daemon doctor                 # diagnostics (config, resources, port, store, auth)
journalctl --user -u mae-daemon   # logs (or the file you redirect to)
ss -tln | grep 9473               # is the collab port listening?  (lsof/netstat fallback)
```

### Liveness, readiness and HTTP health

| Probe | Answers | Use |
|---|---|---|
| `mae-daemon ping` | Is the process serving its KB socket? (exit 0/1) | Container `HEALTHCHECK` / liveness |
| `mae-daemon doctor` | Is this instance configured to work? (exit 0/1) | Deploy gate / readiness |
| `GET /api/health` on the HTTPS listener | Is the HTTPS listener up? `200 {"status":"ok"}` | Proxy / platform HTTP probe |

`/api/health` is **unauthenticated** and returns the status and nothing else — no version, identity,
fingerprint or KB names. Only `GET`/`HEAD` on that exact path are exempt from authentication; every
other request to the listener still needs a bearer token.

### `doctor`'s exit code is a readiness verdict, not a liveness probe

`doctor` ends with `verdict: OK` (exit 0) or `verdict: N problem(s) — this instance
will not work as configured` (exit 1). Every problem it prints counts, including
`collab.auth.mode = 'key'` with an **empty** `authorized_keys` — an instance no client
can connect to. A collab port already bound does **not** count, so doctor can run
against a live instance.

Use it where the question is *"is this instance configured to work?"*: a deploy gate
(the Ansible role's verify step asserts on it), CI, or a pre-flight before restarting.
Use `mae-daemon ping` as the liveness probe. Do **not** use doctor as a container
`HEALTHCHECK` / liveness probe on its own: a freshly
deployed instance whose first client has not been authorized yet is correctly
reported as not ready, and a liveness probe that fails there would restart a process
that is running fine. Authorize the first client as part of the deploy (the role's
`collab_authorized_keys`, or `mae-daemon authorize`) so readiness is reached before
anything gates on it.

### How many clients are connected?

`daemon/status` (on the KB Unix socket) reports live connection counts per
listener under `connections`:

```json
"connections": {
  "kb_socket": {"active": 1, "max": 256},
  "collab":    {"active": 3, "max": 256, "sessions": 3}
}
```

- **`active`** — accepted, still-open connections on that listener.
- **`max`** — the configured cap (`0` = unlimited), so `3` reads as `3 of 256`.
- **`sessions`** (collab only) — clients that got *past authentication* and
  subscribed. A persistent gap between `sessions` and `active` across successive
  polls means clients are connecting and failing to authenticate: the first
  thing to check when a spoke can't reach the hub.

A listener that is not running is **absent**, not zero — a disabled collab
server and an idle one are different facts.

> `sessions` is currently reported only under `[collab.auth] mode = "key"`
> (issue #647): the broadcaster it is derived from is installed into daemon state
> only in that mode. `active` has no such gap.

From the editor: `collab-status` / `collab-doctor`, and `kb_health` for KB-level counts.

> **Known gap (#207):** CRDT op-set / membership-log growth is **not** yet surfaced by `doctor` /
> `kb_health`. Op-sets and the membership log are currently grow-only (no compaction of the CRDT
> state itself — see `E2E_ENCRYPTION.md` F8 / ADR-028), so disk + memory track *total-edits-ever*
> rather than live-content size. For a long-lived, high-churn KB, watch the `data_dir` size directly.

---

## 7. Backup & restore

### A whole daemon instance: `mae-daemon backup`

```bash
mae-daemon backup create /backups/mae-$(date +%F).tar   # safe while the daemon runs
mae-daemon backup restore /backups/mae-2026-06-30.tar /tmp/mae-restore   # into an EMPTY dir
mae-daemon backup verify /tmp/mae-restore               # key=number lines; exit 1 on any mismatch
```

`create` writes ONE archive holding every KB store (the daemon's own `daemon-kb.cozo` and every
store `kb-registry.toml` names), the collab store `state.db`, the registry, and the identity
(`id_ed25519`, `id_ed25519.pub`, `known_hosts`, `authorized_keys`, `trusted_keys`). It also writes a
`manifest.json` with every file's SHA-256 and each store's KB node count. Every SQLite file is
copied with `VACUUM INTO`, which reads inside one transaction and includes writes still in the WAL,
so no daemon downtime is needed. The archive is written as `<out>.parcial` and renamed, so a reader
never sees half of one; an existing `<out>` is never overwritten.

`restore` extracts only what the manifest lists: regular files, each once, all of them, never a
`-wal`/`-shm`, a symlink or a path outside the directory. `verify` re-hashes every file, runs
SQLite's `integrity_check`, re-counts each store's nodes, and prints

```
kb.<name>=<nodes>     one line per store (the daemon's own store is kb.daemon unless a registry row names it)
identity=<0|1>        1 when the private key loads and matches id_ed25519.pub
```

It exits non-zero if any hash, count or integrity check disagrees with the manifest. The lines are
printed even then, so a failure shows what WAS restored. This is the shape a scheduled restore
rehearsal can check (`validar`-style `key=number` output, a minimum of `identity=1`).

> **The archive contains the daemon's private key, unencrypted.** It is written mode 0600, but
> whatever stores the archive holds the key. Encrypt it before it leaves the host if the storage is
> not trusted to that level.

**To restore for real:** stop the daemon, `backup restore` into a staging directory, `backup verify`
it, then put each file back where the manifest's layout says. Delete any `-wal`/`-shm` beside a
store BEFORE copying it in (a stale WAL is replayed over the restored file on the next open,
silently re-applying writes the backup did not contain):

| In the archive | Goes to |
|---|---|
| `stores/daemon-kb.cozo` | `<data_dir>/daemon-kb.cozo` |
| `stores/<uuid>.sqlite` | the `db_path` of that uuid's row in the archived `kb-registry.toml` |
| `kb-registry.toml` | `<data_dir>/kb-registry.toml` |
| `collab/state.db` | `<collab data_dir>/state.db` |
| `identity/*` | the identity dir / `authorized_keys` / `keystore` paths from `daemon.toml` |

What it does **not** contain: the editor-side recovery material (`collections/`, `content_keys/`,
`recovery/`, below), which the daemon never reads; `daemon.toml` itself; the TLS certificate and key
for the HTTPS listener. Keep those in your configuration management.

`backup` is not `checkpoint`/`restore`: those export and replay ONE KB's CRDT documents (ADR-032),
not an instance.

### By hand

The SQLite store has live `-wal` / `-shm` sidecars and `secure_delete` churn, so **never `cp` the
live DB file** — you can capture a torn/stale state. Use SQLite's consistent online copy:

```bash
# Consistent snapshot of a running daemon's store (safe; SQLite walks a read transaction):
sqlite3 ~/.local/share/mae/<store>.cozo ".backup '/backups/mae-$(date +%F).cozo'"
# or:  sqlite3 <store>.cozo "VACUUM INTO '/backups/mae.cozo'"

# The collab trust material is NOT in the DB. What lives in the collab dir:
#   id_ed25519            — your identity seed (the root of all access; losing it = losing every KB)
#   authorized_keys       — the daemon's trust allow-list (key mode)
#   known_hosts           — host keys this peer has pinned (TOFU)
#   collections/          — per-KB key-blind collection op-logs (ADR-040 B2): required to RECOVER a
#                           lost identity on a new machine (the recovering key authors its Rebind
#                           against these without re-fetching from the daemon)
#   content_keys/         — recovered per-KB content keys (re-derivable from the op-log, but cached)
#   recovery/             — your registered OFFLINE recovery key, if you ran collab-register-recovery-key
#   state.db (+ -wal/-shm) — the collab store, when `collab.storage.data_dir` is unset (the default is
#                           <data_dir>/collab, the SAME directory). It is a live SQLite database:
#                           snapshot it like the store above, never copy it with the directory.
rsync -a --exclude 'state.db*' ~/.local/share/mae/collab/ /backups/mae-collab-$(date +%F)/
sqlite3 ~/.local/share/mae/collab/state.db "VACUUM INTO '/backups/mae-collab-$(date +%F)/state.db'"
# NOTE: for real key separation, keep `recovery/` on SEPARATE offline media from this backup —
# a backup holding BOTH your primary and your recovery key gives a thief either path in. The
# recovery key's purpose is to survive loss/compromise of the primary; co-locating them defeats it.

# Restore: stop the daemon, replace the store file + the collab dir, restart.
# Remove the leftover -wal/-shm FIRST (store AND state.db), for the reason given above.
systemctl --user stop mae-daemon
rm -f ~/.local/share/mae/<store>.cozo-wal ~/.local/share/mae/<store>.cozo-shm
rm -f ~/.local/share/mae/collab/state.db-wal ~/.local/share/mae/collab/state.db-shm
cp /backups/mae-2026-06-30.cozo ~/.local/share/mae/<store>.cozo
cp -a /backups/mae-collab-2026-06-30/. ~/.local/share/mae/collab/
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
| P2P mesh join stuck "pending" | the owner must approve the joining **peer daemon's** fingerprint (`kb-approve <kb> SHA256:… editor`) or set a `permissive` policy (§3b) |
| Peer daemon can't dial over the mesh | `authorize` the peer **daemon's** pubkey on each side (not just its editor); with `relay = "disabled"` peers must be directly reachable (LAN/localhost) — use `relay = "default"` to traverse NAT (§3b) |

See also: [`COLLABORATION.md`](COLLABORATION.md), [`E2E_ENCRYPTION.md`](E2E_ENCRYPTION.md),
[`SECURITY_REVIEW.md`](SECURITY_REVIEW.md), and ADR-035 (editor↔daemon boundary).
