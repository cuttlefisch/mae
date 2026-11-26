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
max_document_size_bytes = 10485760              # 10 MB — an update that would GROW a document
                                                #   (a KB node's body, or a text doc) past this
                                                #   is REFUSED (`quota_exceeded`); a doc already
                                                #   past it may still shrink. The editor caps
                                                #   nodes lower first (`kb_max_node_bytes`,
                                                #   1 MiB). ADR-109 D6.
# max_message_bytes = 15029592                  # largest message on a collab session: a request
                                                #   read, and what a paged `kb/join` reply is
                                                #   filled to. Unset = derived so one document at
                                                #   max_document_size_bytes fits (base64 + 1 MiB,
                                                #   never under 10 MiB). ADR-022 Amendment 1.
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
# jwks_ca_file = "/etc/mae/idp-ca.pem"           # trust ONLY these roots for the JWKS fetch
# issuer = "https://idp.example.com"
self_issued_tokens_enabled = true               # accept this daemon's own tokens (key mode only)
self_issued_token_ttl_secs = 3600               # default lifetime of `token mint`
self_issued_token_max_ttl_secs = 86400          # ceiling `token mint --ttl` refuses to exceed
# kb_query_max_scan_nodes = 500                 # search/agenda/health scan cap — see below
# site_enabled = true                          # the live KB site, GET /kb/<kb-id>/<id> (ADR-119)
# attachments_enabled = true                   # a KB's attachments (ADR-115), below
# max_attachment_bytes = 10485760              # largest upload; refused before its body is read
# max_attachment_pixels = 40000000             # an image's width x height, checked before decoding
# attachment_idle_timeout_secs = 30            # longest gap in an upload's body (408 upload_timeout)
# attachment_upload_deadline_secs = 160        # whole body; unset = the cap at 64 KiB/s
# attachment_image_workers = 1                 # images processed at once (~230 MiB each at worst)
# default_storage_bytes = 1073741824           # attachments of principals in no [[tenant]]; 0 = unlimited
# webview_enabled: deprecated and ignored (ADR-119 Amendment 2 B4)
```

- **The JWKS fetch needs CA certificates.** By default it trusts the system's roots. An image with
  none (a minimal container) gets a startup error naming `jwks_url` and this fix, and the OAuth
  listener stays off: the daemon no longer panics. Install CA certificates, or set `jwks_ca_file` to
  a PEM bundle, which the fetch then trusts **instead of** the system's roots (also the way to
  reach an issuer behind a private CA). A file that is missing, holds no certificate, or cannot be
  used as roots is a startup error naming it.
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
- **`kb/query.my_kbs` lists the caller's own KBs.** It takes no `kb_id` and answers
  `{"kbs": [{"kb_id", "title", "role", "encryption"}]}`, sorted by `kb_id`: every KB on this daemon
  the token's principal may read as a member. `role` is `owner`, `editor` or `viewer`, the same
  strings as membership; `title` is the KB's name, or `null`. A permissive KB the caller has not
  joined is not listed, and nothing about other members is returned. Unlike the mTLS `kb/list`,
  it reads the stored KB set, so it answers the same after a restart.
- **A malformed parameter is refused, not ignored.** A `kb/query.*` parameter that is missing,
  of the wrong type or out of range gets JSON-RPC `-32602` with
  `data: {"kind": "invalid_params", "param": "<name>"}`; `null` counts as absent. Two ranges are
  clamped rather than refused, as before: `search`'s `limit` and `neighborhood`'s `depth` (1–3).
  `list`'s `limit` must be at least 1, and above `kb_query_max_scan_nodes` it is clamped to it.
- **Every refusal names its reason in `error.data.kind`** — see [Error kinds](#error-kinds) below.
- **Publication review (ADR-113), off per KB until its owner turns it on.** The same listener serves
  `kb/review.request`, `.withdraw` (by the requester), `.status`, `.pending`, `.set_policy`,
  `.set_code` and `.events` (the owner's paged read of the signed event chain), plus
  `kb/query.revisions` and `kb/query.get` with `revision`. Shapes are the fixtures in
  `docs/verification/fixtures/review-api/`. A request freezes each node at the version the
  requester saw, keeps its own copy of the frozen source and cuts a `review_freeze` history version
  that is never pruned. Every change is an event, hash-chained and signed with the daemon's identity
  (the key history heads use), so a daemon in `collab.auth.mode = "key"` is required to record any.
  `mae-daemon kb review export --kb-id <id> --out <file>` writes the chain (daemon stopped);
  `mae-daemon kb review verify --in <file> --key <base64> [--against <head-file>]` checks it with no
  configuration or database. Keep the head line an export prints somewhere the daemon's key and
  database do not reach: only `--against` it shows the chain's last events removed. A KB holding any
  revision cannot be switched to E2E (`revisions_hold_plaintext`), on any path, a mesh peer's
  included; a KB that needs E2E is created encrypted. A request names at most 50 nodes, and a
  `reason` is at most 2,048 bytes.
- **Attachments (ADR-115), when `attachments_enabled`.** Three routes beside JSON-RPC, each with a
  bearer token and the KB's own access (and `collab.enabled`, which that access needs):

  | Request | Who | Answer |
  |---|---|---|
  | `POST /kb/<kb-id>/blobs` with `Content-Length` (optionally `Content-Digest: sha-256=:<base64>:`) | an editor | `200 {"sha256", "size", "media_type", "width"?, "height"?}` |
  | `GET /kb/<kb-id>/blobs/<sha256>` | a reader | the blob, streamed |
  | `HEAD /kb/<kb-id>/blobs/<sha256>` | a reader | its headers |

  The daemon picks the address: it sniffs the type from the bytes (never the client's header),
  strips image metadata (EXIF and its location, XMP, comments, text chunks), applies an EXIF
  orientation to the pixels, sanitises SVG, decodes the result under `max_attachment_pixels` and a
  1000-frame limit, and hashes what it will store. AVIF is refused (`attachment_unsupported`). A PDF
  (`%PDF-` at the first byte) and any other file are stored exactly as sent, and served only as
  downloads. The
  same bytes in two KBs are two copies; nothing is shared across KBs. A download carries
  `Content-Type` (`image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/svg+xml`,
  `application/pdf`, else `application/octet-stream`), `X-Content-Type-Options: nosniff`,
  `Content-Security-Policy: sandbox; default-src 'none'`, `ETag: "<sha256>"` (a matching
  `If-None-Match` answers `304`) and `Cache-Control: private, max-age=31536000, immutable`; SVG and
  anything not a raster image also `Content-Disposition: attachment`. A caller who cannot read the
  KB gets one identical `403 not_member` whether or not the KB or the blob exists; a reader asking
  for an address the KB does not hold gets `404 blob_missing`. An end-to-end encrypted KB refuses
  attachments (`409 e2e_unsupported`). The bytes live under `<collab data_dir>/blobs/`
  (`blobs.db` plus one file per blob), counted against the uploader's tenant `max_storage_bytes`
  (`[tenant.quota]`, default 1 GiB, `0` = unlimited); a principal in no `[[tenant]]` (agents
  included, and everyone on a daemon with none configured) shares `default_storage_bytes` instead,
  so no uploader is unbounded. `backup` includes them. Nothing is deleted yet: collection arrives
  with the node's attachment map (ADR-115 P2).

  An upload's body must keep arriving: a gap over `attachment_idle_timeout_secs`, or a body still
  arriving after `attachment_upload_deadline_secs`, is refused `408 upload_timeout` with its
  temporary file deleted. Images are processed by `attachment_image_workers` workers; a tenant may
  have four uploads waiting for or holding one (a principal in no tenant counts alone), and an
  upload waits at most 30 s for a worker. Past either, `503 attachment_busy` with `Retry-After: 10`.
  Processing over 30 s is refused (`422 attachment_undecodable`) and the upload is never stored
  afterwards. Every refusal here stores and charges nothing. An SVG nested deeper than 256, with a
  `<use>` chain deeper than 8, or whose `<use>`s expand to over 200,000 elements is refused as
  undecodable; an attribute holding a CSS escape (a backslash) is dropped.

#### Identity-provider tokens and the binding table (ADR-098 Phase C)

An external identity provider's token never names a member by itself. Its `(iss, sub)` — the
subject is the `principal_claim`, default `sub` — reaches a member key only through the **binding
table**, daemon state outside the CRDT in `<collab data_dir>/identity-bindings.db`. Until it is
bound, the token is an *unbound subject*, reported by the bare probe as `idp-unbound:["<iss>","<sub>"]`:
a member of no KB, so every `kb/query.*` method refuses it with `not_member` and `kb/query.my_kbs`
answers `{"kbs": []}` — even if its `sub` spells a member's `SHA256:` fingerprint exactly. Bound, it
reads what that key's own self-issued token reads, no more (binding grants no membership; owners still
add members). Writes over these sessions are not served. A deployment that used to put fingerprints
in `principal_claim` must bind its subjects instead.

```toml
[oauth]
# groups_claim = "groups"                  # where the token lists its groups
# allowed_groups = ["mae-readers", "staff"] # D4 session gate: ANY ONE of them admits the token (exact match);
#                                          # empty (default) = no gate. Not applied to self-issued tokens.
# second_factor_amr = []                   # amr values that count as a second factor — NO default (below)
# second_factor_acr = []                   # acr values that count as one (exact match) — NO default
identity_binding_enabled = true            # POST /identity/challenge + /identity/bind (default false)
# identity_challenge_ttl_secs = 300        # a challenge is valid for minutes
# identity_max_pending_challenges = 4      # unused challenges one subject may hold
```

- **The group gate** is checked on every request, before the binding is looked up. Removing a person
  from every allowed group at the identity provider ends their session at their next token, with no CRDT write
  and no owner involved; `403`, kind `group_required`.
- **Who counts as a person.** A request is a `person` only when it carries an identity-provider token
  AND its binding is flagged `person`; everything else — self-issued tokens, mTLS sessions, unbound
  or unflagged IdP sessions — is an `agent`. A new binding is an `agent`. The bare probe reports
  `kind`. A person's own edits from native MAE count as agent until native clients sign in through
  the identity provider (ADR-111 P4).
- **Binding by proof of possession** (`identity_binding_enabled`), both routes POST with an IdP
  bearer token (a self-issued one is refused, `issuer_not_supported`):
  1. `POST /identity/challenge` `{"fingerprint": "SHA256:…"}` → `{"challenge_id", "challenge",
     "expires_in"}`. `challenge` is a JSON text naming the domain tag `mae-identity-binding-v1`,
     `iss`, `sub`, this listener's `aud`, the fingerprint, a random `nonce` (= `challenge_id`) and
     `expires_at`. Show the person the `iss`/`sub` before they sign.
  2. On the machine holding the member key (the editor's collab identity, `mae --collab-identity`),
     run `mae --sign-identity-challenge '<answer>' --issuer <your iss> --subject <your sub>
     --audience <this daemon's canonical_resource_uri>` (or
     `-` to read the answer from stdin), with either the `challenge` text or the whole answer from
     step 1. `--issuer` and `--subject` are the key holder's OWN identity-provider identity: the
     daemon issues a challenge for any key to any signed-in subject, so a challenge naming your key
     says nothing about who asked for it, and one issued to anyone else is refused (both must match
     the challenge byte for byte). `--audience` is the daemon you mean to bind at, compared byte for
     byte with the challenge's `aud`, so a challenge another daemon issued and relayed to you is
     refused; so is one valid for more than an hour from now (signers cap a challenge's remaining
     lifetime at 3600 s; keep `identity_challenge_ttl_secs` well under it). It prints the issuer, subject, audience and key on stderr before
     signing, and the bind body on stdout. It signs nothing but a canonical, unexpired challenge
     naming its own key and the stated issuer, subject and audience, and never prints the secret key. (The signed bytes
     are the UTF-8 of `challenge`, Ed25519.)
  3. `POST /identity/bind` `{"challenge_id", "public_key": "<base64 of the 32 key bytes>",
     "signature": "<base64>"}` with a token of the same `(iss, sub)` → `{"principal", "kind",
     "bound_by": "proof"}`. A challenge is single use, whatever the outcome. One subject has at most
     one key and one key at most one subject: `binding_conflict` until an administrator unbinds.
- **An external token must carry `iss`.** One with a missing or empty `iss` is refused (`401`),
  as one with an empty subject is: `(iss, sub)` keys the binding table, even when `oauth.issuer` is
  unset and the issuer is otherwise not checked.
- **How a person signed in** (ADR-098, for ADR-113's approval checks). For an identity-provider
  token the daemon reads `amr` (an array of strings), `acr` (a string), `auth_time` (unix seconds)
  and the groups in `groups_claim`; the bare probe reports them as `sign_in`, with
  `second_factor: {"claim": "amr"|"acr", "value": …}` naming the configured value the sign-in
  meets, or `null`. A self-issued token reports `sign_in: null`: it is no sign-in. A claim of the
  wrong type (`amr` as one string or holding a non-string, `acr` not a string, `auth_time` not a
  non-negative integer) is treated as absent, never as a pass. Nothing is enforced yet; ADR-113
  will refuse its controlled acts on this answer.
- **The second-factor rule has no default.** `second_factor_amr` and `second_factor_acr` are empty
  unless you set them, because the values differ by identity provider: measure what yours emits
  (sign in with and without the second factor and read the probe's `sign_in`) and list the values
  that only a second factor produces. A sign-in meets the rule when any listed `amr` value is in its
  `amr`, or its `acr` equals a listed `acr` value. With both lists empty no sign-in meets it, so a
  requirement switched on with nothing configured refuses every act (fail closed). Some identity
  providers emit a constant `acr` that does not reflect a second factor: configure `amr` values for
  those, never that `acr`. A session started from a recovery link may carry no `amr` at all, and so
  never meets an `amr` rule. A sign-in is *fresh* when its `auth_time` is within the act's window;
  no `auth_time` is never fresh, and one ahead of the daemon's clock by more than the token
  validator's 60 s leeway is refused.
- **Administration is local only** — no network method, MCP tool or Scheme primitive unbinds,
  rebinds or sets `kind`. `mae-daemon binding` works while the daemon serves; a change holds on the
  listener's next request:

  ```bash
  mae-daemon binding list                       # issuer subject fingerprint kind bound_by bound_at
  mae-daemon binding bind --issuer <iss> --subject <sub> --fingerprint SHA256:…   # no proof: bound_by=admin
  mae-daemon binding unbind --fingerprint SHA256:…        # or --issuer <iss> --subject <sub>
  mae-daemon binding set-kind --fingerprint SHA256:… --kind person|agent
  ```

  An administrator's `bind` needs no proof (development, tests, recovery): whoever runs it can
  already `token mint` for any fingerprint. `list` shows `admin` or `proof` for each binding.

#### Error kinds

Every error the daemon returns on the collab listener's `kb/*`, `sync/*` and `docs/*` methods, and
on `kb/query.*` (either listener), carries `error.data.kind`: a stable snake_case string. **A client
branches on `kind`, never on the message text** — messages are for people and logs and may change.
The JSON-RPC `code` is the one each refusal had before kinds existed (mostly `-32603`), so a client
keyed on codes sees no change, but the code does not distinguish refusals; `kind` does. The list is
`mae_mcp::protocol::ERROR_KINDS`, and a daemon test fails if this table and that list disagree.

| `kind` | Meaning | Methods | Extra `data` |
|--------|---------|---------|--------------|
| `invalid_params` | A parameter is missing, of the wrong type, malformed (bad base64, an undecodable update) or out of range. Code `-32602`. On an upload: no `Content-Length` (HTTP `411`), a body that does not match it, or a malformed `Content-Digest` (HTTP `400`). | all | `param` |
| `quota_exceeded` | The update would grow a document past `max_document_size_bytes`. Nothing was applied. Code `-32006`. On an upload: a declared `Content-Length` over `max_attachment_bytes`, refused before the body is read; HTTP `413`, `error: "payload_too_large"`. | `kb/node_update`, `sync/update`, `sync/share`, `POST /kb/<kb-id>/blobs` | `size`, `limit` |
| `update_too_large` | One update is over `max_update_size_bytes`. Send smaller increments. | `kb/node_update`, `kb/collection_op`, `sync/update` | `size`, `limit` |
| `request_too_large` | A request is over the session's request cap (one document at `max_document_size_bytes`, as base64, plus 1 MiB; never under 10 MiB). Answered with `id` `null`, then the session ends. Send smaller requests: `kb/share` takes node batches. | any method, on the collab listener | `size`, `limit` |
| `invalid_field` | A node update would leave a known field of the node document with the wrong type (for example `title` not text, `tags` not an array of strings, `org_header` malformed). A field already mistyped before the update, and any unknown key, are left alone. Nothing was applied. Not checked on E2E KBs, whose nodes the daemon cannot read, nor on updates a mesh peer relays (the daemon that accepted the write from its client checked it; refusing a relayed one would only diverge this replica). | `kb/node_update`, `sync/update` on a node document | `field` |
| `tenant_quota_exceeded` | The caller's tenant is over its request quota or connection cap (ADR-060). Back off. On an upload it can also mean the tenant's attachments would pass `max_storage_bytes`: HTTP `507`, `error: "insufficient_storage"` (a request-quota refusal on the blob routes is `429`). | every method, both listeners; the blob routes | — |
| `too_many_open_docs` | A sync-WebSocket session already holds `ws_max_open_docs` documents and the request names another (ADR-099 B4). Nothing ran. | any method naming a document, on `/ws` | `limit` |
| `credential_in_url` | A query string on `/ws` or `/ws/ticket`; credentials never travel in a URL. HTTP `400`, `error: "invalid_request"`. | `GET /ws`, `POST /ws/ticket` | — |
| `too_many_tickets` | A principal already holds 8 unused connect tickets. Use one or let it expire (30 s). HTTP `429`, `error: "too_many_requests"`. | `POST /ws/ticket` | `limit` |
| `awareness_not_owned` | A `sync/awareness` update in the y-protocols encoding names a client id another session on the same document already owns (ADR-099 B5). Nothing was relayed. | `sync/awareness` | — |
| `ticket_invalid` | A connect ticket that is unknown, used, expired, outlived by its token, bound to another origin, or whose IdP binding changed — one identical refusal for all. HTTP `401`, `error: "invalid_token"`, with `WWW-Authenticate`; on a socket, a JSON-RPC error to a ticket `session/renew` (code `-32600`), and the old deadline stands. | `GET /ws`, `session/renew` | — |
| `issuer_not_supported` | A token not issued by this daemon, while ADR-099 B6 holds (on `/ws`: whenever `ws_accept_bound_idp` is off). HTTP `401`, `error: "invalid_token"`, with `WWW-Authenticate`; on a socket, a JSON-RPC error to `session/renew`. On the binding routes the reverse: a self-issued token, whose principal already is a fingerprint, with HTTP `403`, `error: "access_denied"`. | `GET /ws`, `POST /ws/ticket`, `session/renew`, `POST /identity/challenge`, `POST /identity/bind` | — |
| `invalid_token` | A `session/renew` token that does not validate or has already expired. The old deadline stands. | `session/renew` | — |
| `protocol_unsupported` | No protocol version in common at the upgrade. HTTP `400`, `error: "invalid_request"`. | `GET /ws` | `supported` (beside `kind`) |
| `origin_not_allowed` | A browser `Origin` not exactly in `ws_allowed_origins`. HTTP `403`, `error: "access_denied"`. | `GET /ws`, `POST /ws/ticket` | — |
| `kb_id_owned_by_another` | `kb/share` of an id another principal already owns. Mint a new id. Code `-32005`. | `kb/share` | — |
| `not_member` | The caller is not a member of the KB. A principal on this daemon's local blocklist is not a member. On the blob routes, HTTP `403`, `error: "access_denied"`, one identical answer for every caller the KB does not admit, whatever exists. | every `kb/*` method that names a KB, `kb/query.*`, the self-gated `sync/*` methods on a KB document, and the blob routes | — |
| `role_forbidden` | A member whose role does not permit the operation. | writes and `POST /kb/<kb-id>/blobs` (`editor`); membership, policy, governance, `kb/collection_op`, reseal and rebirth (`owner`) | `required` |
| `transport_forbidden` | The KB is not shared over the transport the request came in on. | as `not_member` | `transport` (`hub`/`p2p`) |
| `replication_restricted` | A live-query-only member (ADR-067) asked to replicate the KB. Use `kb/query.*`. | `kb/join` | — |
| `kb_not_found` | No KB with this id is hosted here. | as `not_member` | — |
| `collection_unavailable` | The KB's collection document exists but cannot be read or decoded. | as `not_member`, `kb/share` | — |
| `node_not_in_kb` | The caller may read the KB, and its manifest does not list the node. | `kb/node_fetch`, `kb/query.get`, `kb/query.links`, `kb/query.neighborhood` | — |
| `epoch_fenced` | The write carries an op from a stale authorization epoch (ADR-023). Adopt the authoritative state (`kb/node_fetch`) and re-author under `current_epoch`. | `kb/node_update`, `sync/update` | `current_epoch`; `caller_epoch` for a signed op |
| `signature_invalid` | A signed content op or its header does not verify. | `kb/node_update`, `sync/update` | — |
| `signature_required` | An unsigned content op where signatures are required: always on the P2P mesh, and on the hub once `[collab.auth] require_signed_content_ops` is on. | `kb/node_update`, `sync/update` | — |
| `not_author` | A signed content op whose author is not the session that sent it (ADR-036 Amendment 1 F4). A session may send only ops it authored; ops a mesh peer relays are exempt. Checked after the signature and before membership. | `kb/node_update`, `sync/update` | — |
| `rebirth_unverified` | A rebirth not bound to an owner-signed `Rebirth` op (ADR-107). | `kb/node_update` | — |
| `doc_unavailable` | A KB document named on a raw method that cannot serve it (any node document; a collection on `sync/share` and `docs/*`). Use the `kb/*` methods. | `sync/*`, `docs/*` | — |
| `doc_kb_mismatch` | A KB node document whose address names a different KB from the request's `kb_id`. Every check runs against `kb_id`, so the write is refused. Send the KB the document belongs to. | `sync/update` | — |
| `doc_not_found` | A document that must already exist does not. | `sync/*`, `docs/*` | — |
| `cannot_remove_owner` | `kb/remove_member` named the owner. | `kb/remove_member` | — |
| `signer_unavailable` | This daemon cannot sign the membership op: no signing identity, not the KB's owner, or no signed log. | `kb/set_governance`, `kb/revoke` | — |
| `subject_not_member` | `kb/revoke` named someone who is not a current member. | `kb/revoke` | — |
| `not_authenticated` | The method needs an authenticated principal. | `kb/query.self_token`, `kb/query.my_wrapped_key` | — |
| `not_operator` | A daemon-local administrative method called by a principal other than this daemon's operator. | `kb/blocklist`, `kb/block_principal`, `kb/unblock_principal` | — |
| `self_issue_disabled` | This daemon does not issue tokens. | `kb/query.self_token` | — |
| `no_content_anchor` | An E2E KB with no signed content anchor. | `kb/query.my_wrapped_key` | — |
| `kb_query_unavailable` | The HTTPS listener does not serve `kb/query.*` here (`kb_query_enabled` off, or collab disabled). | `kb/query.*` (HTTPS) | — |
| `e2e_unsupported` | A server-side operation the daemon cannot perform on an E2E KB. Code `-32600`. On the blob routes, HTTP `409`, `error: "conflict"`: an E2E KB cannot hold attachments yet (ADR-115 D8). Every publication-review method refuses an E2E KB (ADR-113 D1), checked after membership. | `kb/query.search`, the blob routes, `kb/review.*`, `kb/query.revisions`, `kb/query.get` with `revision` | — |
| `history_holds_plaintext` | Switching the KB to E2E is refused while its version history (ADR-116) holds plaintext content, which the switch would leave readable. Code `-32600`. A mesh peer's update that would switch it is refused whole and counted. | any write to the KB's collection: `kb/collection_op`, `sync/update`, `sync/share`, `kb/share` | — |
| `group_required` | An identity-provider token carrying none of the groups `oauth.allowed_groups` names (ADR-098 D4). Checked before the binding is looked up; nothing is read or written. HTTP `403`, `error: "insufficient_scope"`; on a socket, a JSON-RPC error to `session/renew`, and the old deadline stands. | every authenticated route on the HTTPS listener, `session/renew` | — |
| `challenge_invalid` | A binding challenge that is unknown, already presented or expired — one refusal for all three. Ask for a new one. HTTP `403`, `error: "access_denied"`. | `POST /identity/bind` | — |
| `challenge_mismatch` | The challenge was issued to another subject or issuer than the token presenting it, or for another audience. HTTP `403`, `error: "access_denied"`. | `POST /identity/bind` | — |
| `proof_invalid` | The key's fingerprint is not the one the challenge names, or the signature over the challenge does not verify. HTTP `403`, `error: "access_denied"`. | `POST /identity/bind` | — |
| `binding_conflict` | The subject already has a key, or the key already has a subject. An administrator must `binding unbind` first. HTTP `409`, `error: "conflict"`. | `POST /identity/bind` | `field` (`subject`/`fingerprint`) |
| `too_many_challenges` | The subject already holds `identity_max_pending_challenges` unused challenges. Use one or let it expire. HTTP `429`, `error: "too_many_requests"`. | `POST /identity/challenge` | `limit` |
| `identity_not_bound` | An identity-provider token whose `(iss, sub)` is bound to no member key, on the sync WebSocket with `ws_accept_bound_idp` on (ADR-099 Amendment 2). It is never admitted as its raw subject; bind it first (proof of possession, or `mae-daemon binding bind`). HTTP `403`, `error: "access_denied"`. At a renewal, the subject was unbound since the socket opened: the socket closes `4403 principal_changed` instead. | `GET /ws`, `POST /ws/ticket` | — |
| `blob_missing` | The KB holds no blob at that address: never uploaded, or collected. Only a caller who may read the KB gets it; anyone else gets `not_member` whether or not the blob exists (ADR-115 D2). HTTP `404`, `error: "not_found"`. | `GET`/`HEAD /kb/<kb-id>/blobs/<sha256>` | — |
| `blob_hash_mismatch` | The upload's `Content-Digest` does not match the bytes that arrived. Nothing was stored; retry the upload. HTTP `400`, `error: "invalid_request"`. | `POST /kb/<kb-id>/blobs` | — |
| `attachment_too_many_pixels` | An image whose header declares more than `max_attachment_pixels`, or an animation over 1000 frames or four times that many pixels in all. Refused before decoding. HTTP `413`, `error: "payload_too_large"`. | `POST /kb/<kb-id>/blobs` | `limit` |
| `attachment_undecodable` | An image that is malformed, fails to decode within its limits, or takes over 30 seconds to process. Nothing was stored. HTTP `422`, `error: "unprocessable"`. | `POST /kb/<kb-id>/blobs` | — |
| `attachment_unsupported` | A type the daemon recognises but cannot process: AVIF (its decoders are C), or an animation carrying an orientation tag. HTTP `415`, `error: "unsupported_media_type"`. | `POST /kb/<kb-id>/blobs` | — |
| `upload_timeout` | The upload's body stopped arriving for longer than `attachment_idle_timeout_secs`, or was still arriving after `attachment_upload_deadline_secs`. Nothing was stored or charged. HTTP `408`, `error: "request_timeout"`. | `POST /kb/<kb-id>/blobs` | — |
| `attachment_busy` | No image worker was free within 30 s, or the caller's tenant already has four uploads waiting or processing. Nothing was stored or charged; retry after `retry_after_secs` (also sent as `Retry-After`). HTTP `503`, `error: "temporarily_unavailable"`. | `POST /kb/<kb-id>/blobs` | `retry_after_secs` |
| `review_not_enabled` | Publication review (ADR-113) is off for this KB; its owner turns it on with `kb/review.set_policy`. | every `kb/review.*` method but `set_policy` | — |
| `stale_version` | The working copy is no longer at the `version` the request named: the requester froze what they saw, and must look again. Nothing was written. | `kb/review.request` | `node_id`, `current` |
| `invalid_transition` | The act is not allowed from the revision's state (ADR-113 E1): a request for a node already in review, a withdrawal of an ended request, an act out of order. Also the delete guard (E7): removing a node that holds a revision, on any path, is `act: "delete"`, `reason: "has_revisions"` (no `from`). | the `kb/review.*` acts, `kb/collection_node_remove`, any write to the KB's collection | `from`, `act`, `reason` |
| `not_approver` | The caller is not on the KB's approver list (a named member or an IdP group their sign-in carries). | `kb/review.approve`, `.reject`, `.enter_into_force`, `.derogate`, `.withdraw` | — |
| `not_reviewer` | The caller is not on the KB's reviewer list. | `kb/review.act` | — |
| `author_cannot_approve` | The caller is an author of the revision (an editor since its last approval, or its requester). A soft block: acknowledged with `acknowledge_self_approval: true` and an `override_reason`, the act goes ahead and records the override. | every `kb/review.*` act | — |
| `authors_unknown` | Who wrote the revision cannot be established (an unattested seed or another unattributed write). A soft block: `acknowledge_unknown_authors: true` and an `override_reason`. | every `kb/review.*` act | — |
| `agent_forbidden` | An agent never acts on a revision, acknowledged or not. | every `kb/review.*` act | — |
| `performer_not_person` | A named member on a reviewer or approver list is not bound as a person. | `kb/review.set_policy` | `member` |
| `set_incomplete` | A set enters into force whole: some members are not approved yet. | `kb/review.enter_into_force` | `missing` |
| `signed_pdf_required` | Under the `signed_pdf` approval record, entry into force waits for the signed PDF of each member. | `kb/review.enter_into_force` | `missing` |
| `source_mismatch` | A rendered PDF declares a source that is not the revision's frozen source (`expected`). | `kb/review.attach_rendered_pdf` | `expected` |
| `attachment_exists` | The revision already holds a current attachment of that kind, with other bytes. A wrong one is annulled first (`kb/review.annul_attachment`); the same bytes again answer the first attach. | `kb/review.attach_rendered_pdf`, `.attach_signed_pdf` | `current` |
| `attachment_type` | A revision attachment must be a PDF: stored as `application/pdf` and beginning `%PDF-`. | `kb/review.attach_rendered_pdf`, `.attach_signed_pdf` | — |
| `rendered_pdf_required` | The KB requires an approval to name the revision's rendered PDF (`approval_requires_rendered_pdf`), and it has none. | `kb/review.approve` | — |
| `rendered_pdf_mismatch` | The approval names another rendered PDF than the revision's current one. | `kb/review.approve` | `current` |
| `unknown_request` | This KB holds no request with that id. | `kb/review.withdraw` | — |
| `unknown_revision` | This KB holds no revision with that id (or it is another node's). | `kb/query.get` with `revision` | — |
| `revisions_hold_plaintext` | Switching the KB to E2E is refused while it holds any publication-review revision, whose frozen source is plaintext and is kept for good (ADR-113 O17). Code `-32600`. A mesh peer's update that would switch it is refused whole and counted. | any write to the KB's collection: `kb/collection_op`, `sync/update`, `sync/share`, `kb/share` | — |
| `doc_code_taken` | That document code is assigned to another node of the KB; a code once given is never given to another. | `kb/review.set_code` | `holder` |
| `activity_record_unavailable` | The activity record (ADR-120) could not commit this request's event within `activity.commit_timeout_ms`, or its writer has failed: nothing of the answer was sent. HTTP `503`, `error: "temporarily_unavailable"`, with `Retry-After`. Retry; the health check is `503` until the record commits again. | every recorded route of the HTTPS listener, with `[activity] enabled` | — |
| `internal` | The daemon failed (storage, persistence, decoding its own state). Not a decision about the caller; a retry may succeed. | any | — |

What a kind does **not** reveal is deliberate. A caller that is not a member gets `not_member` for any
question about a KB's content — `kb/node_fetch` or `kb/query.get` of a node that exists, of one that
does not, and of another KB's node all return the same error — so `node_not_in_kb` reaches members
only. A KB document on a raw `sync/*` path is `doc_unavailable` whether or not it exists. Being on a
daemon's local blocklist is reported as `not_member`, not as a kind of its own.

On a **signed** content op (ADR-036), `not_member`, `role_forbidden` and `epoch_fenced` describe the
op's signed *author* — the caller, since a session may send only ops it authored (`not_author`);
only a mesh peer relays others' ops. The author is examined only after the caller itself is cleared
to write the KB: a caller that is not gets its own refusal whatever header it sends, and a cleared
caller sending someone else's captured op gets `not_author` before that author's standing is read,
so replaying it tells the caller nothing about them.

`kb/join` also returns the caller's identity and fence input in its result: `principal` (the
authenticated principal the daemon authorizes with; `null` with `auth.mode = none`) and `epoch` (the
caller's ADR-023 authorization epoch in that KB — the value the write fence compares against, so a
client authors under `derive_kb_client_id(principal, epoch)`; `null` without a principal). Epochs are
per member, not per KB; one changes when that member's role changes or it is removed and re-admitted,
and a client then re-joins (or reads `epoch_fenced`'s `current_epoch`). A `pending` join returns
`principal` only.

A whole KB moves in pages (ADR-022 Amendment 1), because no single message can bound a KB:
- **`kb/join` with `"paged": true`** answers nodes in id order, filling each reply up to
  `max_message_bytes`, and adds `"next_after": "<node id>"` while nodes remain; the client repeats
  the join with `"after"` set to it, sending the same `node_svs`, until a reply has none. A node is
  subscribed when its page is served. Without `paged` (an older client) the reply carries every node,
  as before.
- **`kb/share` takes node batches**: each request carries the full `collection_state` and some of the
  node states; the daemon keeps the collection from the first and merges each node.
- **`initialize` reports the limits** as `result.limits.{max_frame_bytes, max_outbound_bytes}`, so a
  client sizes its batches; a daemon that does not report them reads requests at 10 MiB.
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

#### The sync WebSocket (`/ws`, ADR-099) — development, single-operator, and a pilot switch

`GET /ws` upgrades to a WebSocket that carries the collab envelope (`sync/*`, `kb/*`), one JSON-RPC
message per text frame, any number of documents per socket. **This cut is for development and for
single-operator deployments.** Until hub signature verification (#727) lands, it accepts by default
**only this daemon's own tokens**, and the principal is the token's `--sub`, taken literally as the
KB membership principal: every writer on it is someone the operator minted a token for. A token from
an external issuer is refused here (`issuer_not_supported`) even though `kb/query.*` accepts it,
unless the pilot switch below is on. Off by default.

```toml
[oauth]
ws_enabled = true                               # default false; also needs:
self_issued_tokens_enabled = true               #   only kid="self" tokens are accepted on /ws
                                                #   ([collab] enabled, auth.mode = "key")
ws_allowed_origins = ["https://mae.example.com"]  # browser Origins, EXACT match; default [] (none)
# ws_ticket_ttl_secs = 30                       # connect-ticket lifetime, 1..=30: may shorten B1's 30 s, never extend it;
                                                #   outside 1..=30 /ws is disabled (logged at error), not clamped
# ws_renew_lead_secs = 60                       # session/renew_needed this long before the deadline
# ws_ping_secs = 30                             # keepalive ping period (under proxy idle timeouts); 0 = off
# ws_max_open_docs = 1024                       # documents one socket may hold (0 = unlimited)
# ws_max_frame_bytes = 1048576                  # largest inbound frame; unset = max_request_body_bytes
# ws_max_outbound_bytes = 15029592              # largest message sent; unset = derived from max_document_size_bytes
# ws_accept_bound_idp = false                   # pilot: also accept BOUND identity-provider tokens (below)
# ws_max_idp_session_secs = 900                 # a bound-IdP session renews at least this often; 0 = no cap
```

**`ws_accept_bound_idp` (ADR-099 Amendment 2) is for a pilot with short IdP token lifetimes.** With
it on, `/ws` and `/ws/ticket` also accept an identity-provider token (`jwks_url`, `issuer`) whose
`(iss, sub)` is bound to a member key (*Identity binding* above). The session's principal is the
**bound fingerprint**, exactly as a self-issued token for that fingerprint would carry, so
membership, roles, the epoch fence and per-frame authorization apply unchanged; a ticket minted with
such a token is bound to that fingerprint, and the binding is read again when the ticket is
presented: unbound or rebound inside its 30 s, it opens nothing (the usual `ticket_invalid`; a fresh
mint then says `identity_not_bound`). The group gate (`allowed_groups`) runs first, as on every
route (`403`, `group_required`). A token whose subject is **not bound** is refused (`403`, kind
`identity_not_bound`) and no socket exists; it is never admitted as its raw subject. The binding is
resolved again at every `session/renew`: once the subject is unbound, or bound to another key, the
next renewal closes the socket `4403 principal_changed`, and a token that lost its group gets
`group_required` and the session ends at its deadline (`4401`). A bound-IdP session's deadline is
the token's `exp` or `ws_max_idp_session_secs` from now (default 900), whichever is sooner, at the
open and at every renewal, so the client renews at least that often with the token it holds. An
unbind, a rebind or a group removal therefore reaches an open socket within the cap (15 minutes by
default) however long the IdP's tokens live. The cap must exceed `ws_renew_lead_secs` (else `/ws` is
left disabled and the reason logged); `0` removes it, leaving the token's own lifetime as the
bound. Self-issued sessions are not capped. **Still keep IdP access-token lifetimes short**
(minutes, not hours): the cap bounds the socket, not what else the token opens.

What this does not protect against until #727 is enforced: a write over such a session is trusted
on the channel (TLS, the IdP token, the binding), not on a signature over the op. A stolen IdP
session can write as its member until its token expires or a renewal is refused; the hub cannot
later prove which ops the member's key authored; and the hub accepts an op whose signed header names
any current editor of the KB as its author, from any member session (true of every session today).
The daemon logs a warning at startup while the switch is on.

A missing prerequisite (collab disabled, or self-issued tokens unavailable) leaves the route
absent and logs why at startup. With `ws_enabled = false`, `/ws` and `/ws/ticket` are ordinary
paths. To try it, mint a token for the principal you want to connect as — normally the fingerprint
of an existing KB member — and open the socket with it:

```bash
mae-daemon token mint --config daemon.toml --sub 'SHA256:…member-fingerprint…' --ttl 15m
```

- **Credentials.** Exactly one, never in the URL: `Authorization: Bearer <token>` (native and
  headless clients), or a connect ticket for browsers, which cannot set that header: `POST
  /ws/ticket` with the bearer header returns `{"ticket", "expires_in": 30}`, and the browser opens
  `new WebSocket(url, ["mae.v1", "mae.ticket." + ticket])`. A ticket is 256 random bits written as
  43 base64url characters, single-use (spent on first
  presentation, whatever happens next), valid 30 s, held in memory only (a restart voids it),
  bound to the minting token's identity and expiry and to the mint request's `Origin`, and at most
  8 may be unused per principal (`429`, `kind: "too_many_tickets"`). Unknown, used and expired tickets all get
  the same `401`, `kind: "ticket_invalid"`. Any query string on `/ws` or `/ws/ticket` is refused
  (`400`, `kind: "credential_in_url"`).
- **Every refusal happens before the upgrade**, as an ordinary response in the listener's
  `{error, error_description}` body plus a `kind` (table above): a bad token gets exactly the `401`
  and `WWW-Authenticate` every other route gives it; a disallowed `Origin` is `403`, `kind:
  "origin_not_allowed"`. The protocol version is the `mae.v1` subprotocol; with none in common the
  answer is `400`, `kind: "protocol_unsupported"`, with a `supported` list. The `101` selects
  `mae.v1` only.
- **The session.** The first frame is the notification `session/hello {protocol, principal,
  auth_deadline, limits: {max_frame_bytes, max_open_docs}}`; `auth_deadline`, everywhere it
  appears, is an RFC 3339 UTC string. Every frame is then authorized against its own document's KB
  by the same checks as on the collab listener; the socket holds no grant of its own.
  `kb/query.self_token` is refused on a socket (`self_issue_disabled`): a bearer session must not
  mint its own successor. Requests are charged to the principal's tenant as on the other listeners.
- **Staying authenticated.** `auth_deadline` is the token's expiry. `ws_renew_lead_secs` before it
  the daemon sends the notification `session/renew_needed`; the client answers `session/renew {"token": …}` with a
  fresh token for the **same issuer and subject** (a different one closes the socket `4403
  principal_changed`; a token that does not validate is answered with an error, `data.kind`
  `invalid_token` or `issuer_not_supported`, and the old deadline stands). On success the deadline
  moves and every open document is re-authorized: one the principal can no longer read is closed
  with the notification `session/doc_closed {doc, kb_id, node_id, reason: "not_member"}` (`kb_id`
  for any KB document, `node_id` for a node document) and the rest of the session continues. A missed deadline closes `4401
  token_expired`, and no frame is applied after it. So removing a member takes effect on their open
  sockets at their next renewal at the latest — keep token lifetimes short.
- **Renewing with a ticket (ADR-099 Amendment 3).** A browser whose tokens a confidential backend
  holds renews with `session/renew {"ticket": …}` instead: the backend mints a fresh ticket with the
  person's token (`POST /ws/ticket`, sending the browser page's `Origin`) and gives the browser only
  the ticket. Exactly one of `token` and `ticket` must be present, else `invalid_params`. The ticket
  is spent exactly as at the upgrade — single use across both, its token unexpired, minted for the
  `Origin` this socket was opened with, a bound IdP binding re-read — and must carry the socket's
  issuer, subject and principal (else `4403 principal_changed`). Any refusal of the ticket itself is
  `ticket_invalid` with the old deadline standing. The new deadline is the minting token's expiry,
  capped by `ws_max_idp_session_secs` for a bound IdP token.
  **Revocation through a ticket renewal waits for the capped deadline.** After `binding unbind` (or
  a rebind to another key), a socket renewing with a **token** closes at once, `4403
  principal_changed`; one renewing with a **ticket** minted before the change gets `ticket_invalid`
  and keeps its old deadline — at most `ws_max_idp_session_secs` (900 s by default) after its binding
  was last read, or the token's own expiry when that cap is `0`. Keep the cap short if unbinding must
  take effect quickly for browser clients behind a backend.
- **Limits.** A frame over `ws_max_frame_bytes` closes `1009`. A reply over `ws_max_outbound_bytes`
  closes `1011` with reason `response_too_large`; by default that bound holds one document at
  `max_document_size_bytes` as base64 plus 1 MiB (15,029,592 at the stock 10 MB), never less than
  10 MiB, and `session/hello` advertises it as `limits.max_outbound_bytes`. `ws_max_open_docs` counts units: a
  joined KB is one unit however many nodes it has, and any other document is one. A session at the
  cap is refused a request opening a new unit (`too_many_open_docs`). A client that stops reading for
  5 s is dropped. If an update for a document does not fit a session's event queue, that document
  is closed for the session (`session/doc_closed`, reason `slow_consumer`) rather than the update
  being lost; the client resyncs it (`sync/resync`, or `kb/join` for a KB).
- **Presence.** `sync/awareness {kb_id, node_id, update}` (or `{doc, update}`) carries a
  y-protocols awareness update for one document, after the same read check as the document itself.
  Every other session on that document receives `sync/awareness {doc, kb_id, node_id, update,
  principals}`, where `principals` maps each client id in the update to the sender's principal as
  the daemon authenticated it; label a cursor by that, never by the update's own `user` fields. A
  client id another principal already uses on that document is refused (`awareness_not_owned`).
- **Keepalive.** The daemon pings every `ws_ping_secs` (default 30) so a proxy's idle timeout never
  closes a quiet session.
- **CORS.** `POST /ws/ticket` answers a browser preflight from an origin in `ws_allowed_origins`
  (POST, with `Authorization` and `Content-Type`), and names that origin, exactly, in
  `Access-Control-Allow-Origin`. No other origin gets a CORS header.
- **`max_connections` counts sockets, not documents.** One socket is one connection however many
  documents it carries; `daemon/status` reports the listener under `connections.oauth`.

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

With the activity record on (`[activity] enabled`, below), the health check also answers for the
record: `503 {"status":"unavailable"}` while its writer has failed or an event has waited past
`activity.commit_timeout_ms` without a commit since, and `200` again on the first commit that
succeeds, with no restart. A daemon that cannot open `activity.db` at start-up answers `503` from its
first request. With the record off, it is `200` whenever the listener is up, as before.

### The activity record (`[activity]`, ADR-120)

A record of who did what through the HTTPS listener: one event per request (JSON-RPC, the live
site, blobs, `/ws/ticket` and the `/ws` upgrade, the binding routes), with the principal, the KB and
nodes, the route's template, the outcome, and the time. Discovery metadata, `/api/health` and the
site's static assets are not recorded. **Off by default**; a daemon serving one editor has no reason
to keep a record of its user. `--check-config` warns when the HTTPS listener is on and the record is
off. This release records the HTTPS listener only: collab sessions (mTLS, the `/ws` session itself,
the mesh) and the KB socket are not recorded yet, and the record's heads are not signed yet.

```toml
[activity]
enabled = false            # off by default (D11)
queue_capacity = 4096      # events waiting for the writer
commit_timeout_ms = 2000   # longest a request waits for space and for its event's commit
refusal_window_secs = 60   # repeated refusals are coalesced per window (below)
retention_long_days = 730  # floor 730: may be lengthened, never shortened
retention_short_days = 365 # floor 365
trusted_proxies = []       # peers whose X-Forwarded-For is recorded, e.g. ["10.0.0.5"]
# head_interval_secs = 10, span_close_secs = 60: accepted, used by later phases
```

A period under its floor stops the daemon at start-up, and so does `--check-config`.

**Fail closed.** No response leaves the daemon until its request's event has committed
(`synchronous=FULL`). If the record cannot take it within `commit_timeout_ms` (a full disk, an I/O
error, a held lock), the request is refused `503`, kind `activity_record_unavailable`, with
`Retry-After`, and nothing of its answer is sent. When the record's disk fails, the daemon stops
serving: that is the requirement working as intended.

**A deployment that enables the record MUST alert on it.** Wire `GET /api/health` (or `mae-daemon
activity status`, exit 1) into monitoring with an alert that reaches a person; a fail-closed stop
that nobody hears about is not acceptable. The log carries two stable lines a collector may also
alert on: `ERROR … activity_record_unavailable` on the transition to unhealthy and
`INFO … activity_record_recovered` on recovery. The health check is the authoritative signal.

```bash
mae-daemon activity status          # exit 0 healthy, 1 unhealthy, 2 the daemon is not answering
mae-daemon activity status --json   # {"healthy", "unavailable_total", "queue_depth",
                                    #  "last_commit_age_ms", "last_head_age_s" (null until signed
                                    #  heads), "chains": {"long": {"seq", "entry_hash"}, "short": …}}
```

`unavailable_total` counts requests refused because the record could not take them. An idle daemon
writes nothing, so a growing `last_commit_age_ms` is not an alarm by itself.

**Where it lives.** `<collab data_dir>/activity.db`, mode `0600`, beside `state.db`, one file per
daemon; `activity-session.key` (32 random bytes, mode `0600`) beside it derives the session ids.
Triggers refuse every `UPDATE` and every `DELETE`; two hash chains (`long`: refusals, sign-ins,
sessions, writes, administrative and daemon events, kept 2 years; `short`: successful reads and
requests, 12 months) make an edited, removed or reordered row visible to `verify`. Disposal at the
end of retention is not built yet; start-up logs an error once the oldest event is 330 days old.
`mae-daemon backup` includes the file.

**Addresses.** A client's address is recorded only on a sign-in, a session start and a refusal; any
other event carries the session id instead. `X-Forwarded-For` is recorded, apart from the peer's
own address, only from a peer listed in `trusted_proxies`.

**The session header.** Every response to a request whose bearer token validated carries
`Mae-Activity-Session: <id>` (26 characters, `[a-z2-7]`), the id the record uses for that token (a
new token, a new id); the `/ws` upgrade carries its connection's id. A `401` carries none. A relaying
server may log it beside its own session to join the two records. It is an identifier, never a
credential: nothing accepts it.

**Refusals are coalesced, never dropped.** The first refusal of a `(principal or address, route,
kind)` in each `refusal_window_secs` is recorded before it is answered; the rest are counted and
recorded as one event with `count`, `first_at` and `last_at` when the window ends (and at a clean
shutdown). Past half the queue, refusals made before authentication are counted per kind without
addresses.

**Reading it.** Only on the host, as the operator; no agent, editor tool or MCP method reads it.

```bash
mae-daemon activity export [--principal ID] [--kb ID] [--kind K] [--outcome ok|refused|error|KIND]
                           [--session ID] [--since T] [--until T] [--no-proof] --out FILE
mae-daemon activity verify --in FILE   # an export, offline: no config or database needed
mae-daemon activity verify             # the stored record itself, indexed columns included
```

An export records its own run first, then writes JSON Lines covering the whole record: the events
that match, and a stub (the link only) for each that does not, so `verify` can see that nothing is
missing. `--no-proof` leaves the stubs out, for a person reviewing patterns; `verify` refuses it as
unverifiable. Times are RFC 3339 UTC (`2026-11-25`, `2026-11-25T10:30:00Z`). Until signed heads
exist, `verify` checks the chains only: it does not detect a record rewritten by someone able to
recompute its hashes, nor a chain's last rows removed from the database.

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
  "collab":    {"active": 3, "max": 256, "sessions": 3},
  "oauth":     {"active": 2, "max": 256}
}
```

- **`active`** — accepted, still-open connections on that listener.
- **`max`** — the configured cap (`0` = unlimited), so `3` reads as `3 of 256`.
- **`oauth`** — the HTTPS listener. A sync WebSocket counts once however many documents it
  carries (ADR-099).
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
store `kb-registry.toml` names), the collab store `state.db`, the attachment store (its `blobs.db`,
snapshotted, then exactly the blob files that snapshot's rows name), the registry, and the identity
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
| `collab/identity-bindings.db` | `<collab data_dir>/identity-bindings.db` (ADR-098 bindings) |
| `collab/activity.db` | `<collab data_dir>/activity.db` (ADR-120 activity record) |
| `collab/blobs/` | `<collab data_dir>/blobs/` (ADR-115 attachments: `blobs.db` and the files its rows name) |
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
#   state.db.lock         — the store lock (§7b); empty, safe to leave out of a backup
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

## 7b. Seeding a native KB from an org repository

`mae-daemon kb seed` turns one tagged commit of an org repository into a native KB on this daemon
(contract `org-literal-v1`, see `daemon/src/seed/mod.rs`): one node per `.org` file, the file drawer,
`#+title` and `#+filetags` as typed fields, the rest of the file kept byte for byte. The KB is owned
by the daemon's own identity (key mode), served over the hub, and marked `org-literal-v1`, so MAE's
own node writer refuses to write it. Members are added by the operator with `kb member` (below)
until ADR-098 binds directory identities to members.

`kb seed` needs **git 2.40 or later** (it uses `git check-attr --source`). Debian bookworm ships 2.39
and its backports do not carry git, so run the seed on a host with a newer git (Debian trixie has
2.47); the daemon binary itself does not need it.

```bash
# 1. Plan: reads git only, writes nothing. Review the refusals and note plan_hash.
mae-daemon kb seed --dry-run --git /srv/notes --rev v1.0
# 2. Apply exactly that plan. Stop the daemon first.
systemctl --user stop mae-daemon
mae-daemon kb seed --git /srv/notes --rev v1.0 --kb-id notes --plan-hash <plan_hash>
systemctl --user start mae-daemon
```

Apply recomputes the plan from git with the same options and refuses unless it has no refusal and
its hash is the `--plan-hash` you reviewed. It is safe to re-run:

- After a crash or an error part-way, a re-run with the same plan finishes the KB. Readers see
  nothing until every node exists (the collection is written last).
- After success, a re-run writes nothing (`already applied`).
- A different plan for an existing `--kb-id`, or an id that exists without having been seeded, is
  refused (to move a seeded KB to a new revision, use `--update`, below). A node document that
  already exists with other content is never overwritten.

The seed's provenance (repository, revision, commit, tree, plan hash, contract, excluded paths, start
and finish times) and each node's source path, blob, N(F) digest and git times are kept in this
daemon's `state.db` (`kb_seed`, `kb_seed_nodes`). They are not synced to mesh peers.

`kb/query.get` and `kb/query.list` report, for every node, `created_at` and `modified_at` (RFC 3339
UTC, or null), `modified_at_basis` (`seed`: the git time; `edit`: a later edit recorded by this
daemon; `unknown`), and `property_order` with `org_header` (`property_order`, `has_title_line`,
`has_filetags_line`): what a reader needs to lay the header out as the file had it.

**Aliases in an `org-literal-v1` KB come from `ROAM_ALIASES`.** A node's aliases there are derived
on read from its `ROAM_ALIASES` property, kept verbatim as the file had it and read the way
org-roam reads it (Emacs' `split-string-and-unquote`: space-separated, double-quoted when an alias
holds whitespace, backslash escapes inside quotes). The property is the only place to write them;
the node's `aliases` array must stay empty, and `kb export` refuses a node whose array is set. In a
native KB the array is the source, as before. One rule (`mae_sync::kb::aliases`) feeds
`kb/query.get`/`list` (`aliases`) and the Cozo projection the daemon's KB search uses. A value
Emacs could not read (an unterminated quote, a bad escape) gives the node no aliases, as org-roam
gets none, and `get`/`list` add `aliases_problems` saying why; so does an array found set anyway,
whose values are not reported. Nothing copies the property into the array.

### Photos and other files in a seeded repository (ADR-115 P4)

A page's images and files seed with it when the repository is laid out as Emacs's org-attach lays
it out with ID-based directories, and `oauth.attachments_enabled` is on (a plan with attachments is
otherwise refused by name, never seeded without them):

- **Link each file as `[[attachment:NAME]]`.** A `file:` link or a bare path is not an attachment and
  is not carried.
- **Put the file at `data/<ID[0:2]>/<ID[2:]>/NAME` next to the page**, where ID is the nearest `:ID:`
  up the outline from the link: the headline's own if it has one, else its parent's, up to the file's
  drawer. For `:ID: 0a1b…` on `notes/page.org`, a link under it names `notes/data/0a/1b…/NAME`. This
  is what `org-attach` writes with `org-attach-id-dir` at its default `data/` and
  `org-attach-use-inheritance` on. `:DIR:` and `:ATTACH_DIR:` are not supported.
- **A NAME is one portable file name:** at most 255 bytes (decomposed, too), no `/` or `\`, no
  control or invisible format character, none of `< > : " | ? *`, no `%`, not ending in a dot or a
  space, not a Windows device name (`CON`, `nul.png`, …), and not `.` or `..`. Two NAMEs of one page
  that differ only in case or Unicode normalization (`Foto.JPG`, `foto.jpg`) are one file and are
  refused. The rule, in check order, with examples, is
  `docs/verification/fixtures/kb-attach/name_rule.json`.
- **The file is committed as itself:** not a symlink (nor under a symlinked directory), not a
  submodule, not a Git LFS pointer or any path a checkout attribute changes, at most
  `oauth.max_attachment_bytes`, and, for an image, at most `oauth.max_attachment_pixels`. AVIF is
  refused.

The dry run lists every file under `attachments` with the sha256 and size of what will be stored,
and every file it cannot carry under `refusals`, each with its reason. `plan_hash` covers those
processed hashes, so a changed image is a changed plan. Images are stored as an upload stores them:
metadata (EXIF, GPS, XMP, comments) stripped and an orientation tag applied to the pixels. So `kb
export` writes back the stored file: a PDF or a metadata-free PNG byte for byte, a photo with its
metadata gone (and, if it carried an orientation tag, re-encoded upright). Seeding the export again
binds the same hashes. `--update` stores new and changed files before any node binds them; a file a
page no longer links stays in the store until the sweep (ADR-115 D5) finds no version that holds it.

`kb/query.get` and `kb/query.list` report a node's files as `attachments`: one
`{name, sha256, size, media_type, url}` per NAME, `url` being `/kb/<kb-id>/blobs/<sha256>` on the
HTTPS listener, which checks the reader again. The live site shows an image the store holds at that
URL, and a link whose file it lacks as a labelled `missing` placeholder.

### Refreshing a seeded KB to a new revision: `kb seed --update`

A seeded KB is a one-way replica of its repository at one revision. To move it to a newer one, plan
the update against the KB as applied, review the difference, then apply exactly that plan. Both
steps open the store, so the daemon is stopped for both.

```bash
systemctl --user stop mae-daemon
# 1. Dry run: the new plan and its plan_hash, then how it differs from the KB as applied.
mae-daemon kb seed --update --dry-run --git /srv/notes --rev v1.1 --kb-id notes   # --json for a gate
# 2. Review the diff (below). Then apply exactly that plan.
mae-daemon kb seed --update --git /srv/notes --rev v1.1 --kb-id notes --plan-hash <plan_hash>
#    (add --discard-edits <edits_hash> only if the dry run listed edits outside the seed, below)
systemctl --user start mae-daemon
```

The dry run prints the plan report, then `== update of KB 'notes' ==` with the plan it was seeded
from, and these sections, one node per line (`<id> <path>`):

- `added`: new pages. A page that an earlier update removed and the repository restored under the
  same `:ID:` is marked `(its document returns, with its history)`.
- `changed`: pages whose content or path changes, with the fields that change (`title`, `body`,
  `tags`, `properties`, `org_header`, …) and `(moved from <path>)` for a move.
- `removed`: pages the revision no longer has.
- `edited outside the seed since its last apply`: nodes whose content is not what the seed last
  wrote, with the fields that differ and who wrote them (from the daemon's edit record), then
  `edits_hash`, the token that discards exactly these edits. A returning page whose document was
  written while it was out of the KB is listed here too (`returns: …`).

Its exit status is 0 when the apply would go ahead and 1 when it would refuse. `--json` carries the
same as `update.edits_hash`, and per edit its `version` and whether the apply `discarded` it.

The update keeps the KB id, its owner, members, policy and signed membership log, every identity
binding, and every node's document, address (so its page URL) and history. A changed node is
brought to the new content field by field through the CRDT (the same diff a human edit makes), so
peers converge and the content it replaced is kept as a version (`kind: seed`); a new page is
written as a first seed writes it; a removed page leaves the KB's node list (its document and
history are kept, and its last content is kept as a version, `kind: delete`).

**Edits outside the seed.** If any node was edited after the last apply, the update refuses and names
them. To overwrite them with the repository's content, give `--discard-edits <edits_hash>` from the
dry run; each edit stays in its node's history, so nothing is lost. The hash covers each listed node
at the content the dry run saw, so if anything is edited between the dry run and the apply (the
daemon may have run in between), the apply refuses with `the edits changed since the dry run: run it
again`: an edit is never discarded without having been shown. An edit is detected by comparing the
node with what the seed wrote (a digest kept in the seed record; for a KB seeded before this
release, the page is rebuilt from the blob recorded for it, so keep the repository's history
available), never by times. A page an earlier update removed is compared, when it returns, with its
content at the removal, and one the seed never wrote with the new revision's content.

The plan must come from the same history the KB was seeded from: the commit last applied must be
the new revision or in its history, in the repository given to `--git`. A path is not an identity, so
a repository moved on disk, or a clone of it, is accepted (the record then takes the new path). An
unrelated repository, or a history rewritten past the applied commit (a force-push), is refused; seed
it under a new kb-id instead. Because the refresh needs the applied commit, keep the history: don't
seed from a shallow clone that drops it.

Like the first apply, the update is safe to re-run: its intent is recorded before the first write,
and a re-run with the same plan after a crash finishes it, checking first that no node was written
in the meantime. If one was, the resume refuses until you run the dry run again (it judges each node
against the cut-short update, so pages it already wrote are not reported as edits) and give its
`edits_hash`. A page written after the update already wrote it is listed as `kept by the update`:
the resume does not overwrite it, and the next update reports it. Readers see each node old or new, and the KB's node list switches from the old
set to the new one in one step. Until the update finishes, another plan is refused and so is
`kb export`. After it finishes, a re-run writes nothing (`already at this plan`). An end-to-end
encrypted KB, a KB no seed made and a seed whose first apply never finished are refused.

A page whose node holds a publication-review revision (ADR-113) is never removed: if the new revision
drops its file, the update is refused before it writes anything, naming the node. Restore the file,
or derogate the document through the review and keep the file.

### Who wrote the seeded content: `--author`

A seed writes as the daemon, which names no person, so a publication review (ADR-113) cannot tell
who wrote seeded content: approving it then needs the approver to acknowledge unknown authors. When
you know who wrote it, give `--author <SHA256:…>` (the member's key fingerprint) to the apply or the
update. Each node the seed writes is recorded with that author, and the KB's review log gains an
`authorship_attested` event naming the node, the author, the commit and you (the operator, from
`$USER`). The review then counts that member as an author of the seeded content, so their own
approval is a self-approval and anyone else's is not.

The attestation covers only the writes it was given on. An `--update` without `--author` makes the
authors of every node it writes unknown again, until the next approval. Attest only what you know:
the record says who attested it, and it cannot be withdrawn.

### Members of a seeded KB: `mae-daemon kb member`

```bash
systemctl --user stop mae-daemon
mae-daemon kb member add    --kb-id notes --member SHA256:… --role viewer   # or editor; --label <name>
mae-daemon kb member remove --kb-id notes --member SHA256:…
mae-daemon kb member list   --kb-id notes
systemctl --user start mae-daemon
```

The operator's bridge until ADR-098: it changes membership of a KB **this daemon owns** (a seeded KB),
offline under the store lock like `kb seed`, through the same write path as `kb/add_member`, and signs
the change into the KB's membership op-log as the daemon. It refuses, writing nothing: a KB another
identity owns (its owner manages it), a role other than `viewer`/`editor` (ownership is not
transferred here), the owner as the member, and a member that is not a key fingerprint. Like
`token mint`, it acts as the KB's owner: it is a command for the operator only, never exposed as an
MCP tool or an RPC.

### A KB's content anchor: `mae-daemon kb anchor`

```bash
systemctl --user stop mae-daemon
mae-daemon kb anchor --kb-id notes                          # show it
mae-daemon kb anchor --kb-id notes --public-key <base64>    # record it
systemctl --user start mae-daemon
```

A KB's anchor is the owner public key its signed membership op-log is verified against (ADR-036
Amendment 1 F1). Once a KB has an anchor and a signed op-log rooted at a genesis that key signed,
its membership — for access and for admitting signed content ops alike — is read from that log.
Otherwise it depends on where the anchor came from. For an anchor this daemon recorded (below), it
is read from the KB's member list, so a log without that root, such as one a member's key rotation
started, never empties a KB's membership. A KB joined over the mesh fails closed: its member list
came from a relay, so without that genesis it has no members.
The daemon records it by itself, durably: the sharing session's key when a KB is first shared, and,
for a KB shared before this, the key of its owner's next share or write. `kb anchor` without
`--public-key` prints the key and its source (`hub_recorded` or `joined`). Use the command only for a
KB whose owner will not reconnect. It records the key whose fingerprint the KB names as owner and
no other, once: a different key than the recorded one is refused. Operator-only, like `kb member`.
Nothing replaces an anchor: a mesh join ticket naming a KB this daemon anchored is refused, and the
dial stops.

**Members the signed log leaves out.** Where the log governs, a KB's member list no longer grants
access: an entry the log does not admit has none. Such entries are reported, never dropped silently:
the daemon logs a warning with their number when it records the anchor, `kb anchor` prints them when
it records one, `kb anchor --kb-id X` (show) prints which source governs and lists them, and
`mae-daemon kb anchor` with no arguments lists them for every anchored KB (offline, under the store
lock, like the rest of `kb anchor`). `mae-daemon doctor` does not: it never opens the store as a
writer, and deriving membership needs one, so it points at that command instead. The owner admits each
one with a signed membership op. On such a KB, `kb member add|remove`, `kb/add_member`,
`kb/remove_member`, `kb/approve_member` and a permissive join's auto-admit are refused
(`signer_unavailable`) unless this daemon signs the change into the log as the KB's owner, with the
key the KB is anchored to. One consequence: a log-governed KB with a `permissive` policy admits no
one by itself any more. A permissive join never signs, so it is refused, and the owner admits
joiners with signed membership ops. This fails closed; it trades the automatic admission for not
granting access the log never recorded.

**A squatted KB id.** The first session to share an id owns it, and its key becomes that KB's
anchor, durably. No command clears an anchor. To recover: the legitimate owner shares under a fresh
id (MAE does this by itself when it is told `kb_id_owned_by_another`), and you revoke the squatter's
access (`mae-daemon revoke <fingerprint>` for a key; its tokens or binding for a bearer identity).
The squatted id stays taken.

### Exporting it back: `mae-daemon kb export`

```bash
systemctl --user stop mae-daemon
mae-daemon kb export --kb-id notes --out /srv/export     # an EMPTY (or new) directory
mae-daemon kb export --kb-id notes --out /srv/export --force   # write into a non-empty one
```

Each node is written to its source path as N(F): unedited since the seed, a canonical file comes back
byte for byte. Properties keep their recorded order; one added since goes after them in sorted order,
one removed is skipped, and both are printed as `note:` lines. Every file is checked first: a node
whose text would seed back differently (a `#+title:` or a drawer typed into its body, an edited
`ID`) is printed as `refused:` and not written, and the exit code is 1.

A node that holds something the file cannot carry is refused the same way (`unrepresentable`),
naming the fields, rather than written without it. The check builds the node the exported file
would seed back to and compares it with the node, field by field: any difference refuses. In
practice that is an `aliases` list (an org-literal-v1 alias is the `ROAM_ALIASES` property, which is
exported, but the seed never fills the list), a todo state, a priority, structured links, a source
version, or a `kind` that is not the node's `KIND` property value (one changed without the other).
Fix the node, for example by moving the aliases into `ROAM_ALIASES` and clearing the list, and export
again. Three fields are exempt because the seed sets them itself rather than reading them from the
file: the provenance (always `user_org`), the creation time (from git history, kept in the seed
record), and the header shape (the property order and whether the title and filetags lines exist,
which the export writes from the node and reports in its notes). Symlinks in the target are
never followed. Nodes created after the seed have no source path and are refused. Two source paths
that are one file on a case- or normalization-insensitive filesystem (`A.org` and `a.org`, or NFC
and NFD spellings) are both refused (`path_collision`), so neither overwrites the other on APFS,
NTFS or a folding mount; the seed planner refuses such paths too. An existing file is replaced
through a temporary file and a rename, so a crash leaves the old file or the new one.

### Store access: stop the daemon

`kb seed` (apply), `kb export` and `restore` open the collab store offline. Each takes the store lock
(`state.db.lock` in the collab directory, an OS lock released when its holder exits) and refuses if
any process holds it or a daemon answers on the control socket. The serving daemon takes the same
lock when it opens its store. Started while an operator command holds it, it **exits** with status
75 (`EX_TEMPFAIL`) and says why, rather than serving with collab and `kb/query` silently off. A
supervisor that restarts on failure (`Restart=on-failure`) brings it up once the command is done.

## 7c. Version history: signed heads and keeping one elsewhere

Every KB node's versions form a hash chain (ADR-116). The daemon signs each KB's **history head**
(every node's latest version) with its own key on the maintenance tick, only when it changed, into
an append-only log in `state.db`. `kb/history.verify` checks the chains against the latest one,
which shows a removed last version or a history re-sealed from an earlier point.

What no log on the daemon can show is a rewrite by whoever holds both the daemon's key and its
database: they can re-sign. Keep a head somewhere they cannot reach, and check against it:

```bash
mae-daemon kb history head   --kb-id notes --out /secure/notes-head-2026-11-12.json
mae-daemon kb history verify --kb-id notes --against /secure/notes-head-2026-11-12.json
```

- `head` signs the head now (or takes the latest, if nothing changed) and writes it; it never
  overwrites an existing file. Without `--out` it prints the JSON (`mae-history-head/v1`).
- `verify` checks every chain, the latest signed head and, with `--against`, the kept one. It
  prints `verified` and exits 0, or names the first failing node and version, prints
  `NOT VERIFIED` and exits 1. A KB with no history is refused rather than reported as verified, so
  a mistyped id cannot pass.
- A head counts only if it is signed with this daemon's key.
- `backup create` signs every KB's head first when the daemon is stopped. With it running, the
  backup carries the latest head the maintenance tick signed.
- Both commands are operator-only and offline (stop the daemon), like `kb member`.

### Reading a node's history

A KB reader lists a node's versions with `kb/history.list {kb_id, node_id, limit?, before_seq?}`.
It returns `{versions, next}`, **newest first**, without content. `limit` is 1 to 500 (default
100); a value outside that is refused, not adjusted. To read further back, call again with
`before_seq` set to the `next` value; `next` is `null` on the page that reaches version 1. A seq
never changes and new versions always take a higher one, so paging this way returns every version
exactly once even while edits continue. `kb/history.get {kb_id, node_id, seq}` returns one version
with its content and the chain's verification.

Every version record carries:

- `kind`: `edit` (cut when the node passes from one editor to another), `named` (a checkpoint cut
  by `kb/history.checkpoint`; `name` is set), `rollback` (`restored_from` is set), `import`,
  `seed`, `rebirth`, `delete`, or `pruned` (an audit entry of a pruning pass, with no content).
  A client should show a kind it does not know as that kind, never as an edit.
- `editors`: `[{kind, principal}]`, everyone who edited the node in the range the version covers.
  `kind` is `principal` (an authenticated principal: a key fingerprint, `psk:<keyid>` or an OAuth
  subject), `signed-relay` (a relay whose update was signed; the signer has its own `principal`
  entry), `mesh-peer` (a relay with no verified signer: the author is unknown),
  `unauthenticated`, or `internal` (the daemon itself). A named version's `editors` are the
  editors since the previous version; who named it is in `summary` (`named by <principal>`).
- `created_at`: when the daemon cut the version, RFC 3339, UTC, to the second.

The daemon does not record whether a principal is a person or an agent: nothing in its identity
model marks one (ADR-113 relies on a marking ADR-098 has not designed), so no field claims it.

## 8. Troubleshooting

| Symptom | Check |
|---------|-------|
| Daemon exits with status 75, "the collab store is in use by another process" | an operator `kb seed`, `kb export` or `restore` is running; let it finish, then start the daemon |
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
