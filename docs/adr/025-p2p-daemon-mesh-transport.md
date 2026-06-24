# ADR-025: P2P daemon-mesh transport (iroh)

**Status:** Accepted (design). Phased implementation tracked separately (P2P epic, Phase 1-2).
**Extends:** ADR-001 (server-client protocol), ADR-006 (collaborative state engine / transport),
ADR-014 (binary architecture — the daemon workspace), ADR-017 (mTLS-as-identity).
**Feeds:** ADR-026 (peer-verifiable integrity), ADR-027 (collaboration observability).

## Context

v0.14 collaboration is **daemon-as-central-hub**: the daemon is strictly a listener
(`daemon/src/main.rs` `accept()` loops only — zero outbound dial), each client dials *one* daemon,
and fan-out is single-daemon (`shared/mcp/src/broadcast.rs`). To let globally-distributed peers
maintain shared KBs **without a central server**, the daemon must become a **peer**: each user runs a
local daemon, and daemons sync directly in a mesh. ("P2P **via the daemon**" — editors keep talking to
their local daemon over the existing Unix socket; the *daemon* joins the mesh.)

Two facts make this tractable rather than a rewrite:
- The wire protocol is **already transport-agnostic**: `read_message`/`write_framed`
  (`shared/mcp/src/lib.rs:328,383`) and `handle_client_with_auth` (`daemon/src/collab_handler.rs:39-49`)
  are generic over any `tokio::io::AsyncRead/AsyncWrite/AsyncBufRead` duplex stream. A new substrate
  drops in behind an adapter; the JSON-RPC method catalog (`sync/*`, `kb/*`) is unchanged.
- Peer **identity already exists**: Ed25519 keypairs + fingerprints + a trusted-peer keystore
  (`shared/mcp/src/{identity,keystore}.rs`, ADR-017).

The hard requirement is **global reach** (peers behind NATs, not LAN/VPN-only), under **pure-Rust +
cross-OS (macOS + Linux)** constraints, with a 5-year maintainability horizon.

## Decision

Adopt **iroh** (n0) as the daemon-to-daemon transport substrate.

**Why iroh (the clear winner):**
- **QUIC-native** (quinn) with **built-in NAT hole-punching + relay fallback** — the robustness crux
  for "global peers." Direct-path when hole-punching succeeds; transparent relay otherwise; no
  application change either way.
- **Node IDs *are* Ed25519 public keys** — composes directly with MAE's trusted-peer fingerprints
  (ADR-017/018). No `PeerId`-multihash indirection; a discovered/known peer's KB-membership principal
  and its transport identity are the **same key**. Trusted-peer auth + rotation (ADR-026 D3) carry over.
- **Ordered, reliable bidirectional streams** (`open_bi()` → send/recv) that satisfy the
  `AsyncRead + AsyncWrite` contract — a **thin (~tens-of-lines) adapter** feeds them into the existing
  Content-Length framing + `handle_client_with_auth`. QUIC stream multiplexing removes head-of-line
  blocking between docs.
- **Pure Rust, cross-OS, no C/FFI.** Global discovery via iroh DNS/Pkarr; **mDNS
  (`crates/mae/src/mdns_discovery.rs`) stays the LAN fast-path** (orthogonal, no conflict).

**Rejected alternatives:**
- **rust-libp2p** — capable (Circuit-Relay-v2 + DCUtR hole-punching + Kademlia), but heavier dependency
  weight, a `PeerId` multihash that does **not** map 1:1 to MAE's raw Ed25519 fingerprints (extra
  indirection at the security boundary), and `Swarm`/`NetworkBehaviour` event-loop integration friction
  for what is fundamentally point-to-point RPC. Over-spec'd unless we later want a global DHT/gossip
  overlay.
- **raw quinn + custom signaling/hole-punching/relay** — minimal deps and full control, but home-grown
  NAT traversal is notoriously brittle (10-20% real-world failure without heavy investment) and the
  relay server becomes ours to operate. ~2-3 engineer-months of non-core work better spent elsewhere.
- **WebRTC-rs / magic-wormhole** — built for one-shot/browser/media flows, not a long-running trusted
  daemon mesh.

**Architecture (additive, feature-flagged):**

1. **`Endpoint` per daemon**, built from the daemon's existing Ed25519 secret key
   (`shared/mcp::identity`). Advertises via iroh discovery (DNS/Pkarr) + mDNS on LAN.
2. **Inbound:** accepted peer streams are wrapped by the `AsyncRead/AsyncWrite` adapter and handed to
   the *same* `handle_client_with_auth` path. Peer authorization reuses `authorized_keys`
   (the iroh node ID = the authorized fingerprint), so an unknown/revoked peer is rejected at handshake.
3. **Outbound (the new capability):** the daemon dials known peer node-IDs for each shared KB and opens
   a stream per peer; mesh fan-out + gossip + anti-entropy are ADR-026/Phase-2 concerns layered on top.
4. **Editor↔local-daemon is unchanged** (Unix socket). Only the daemon learns to be a peer.
5. **Relay:** support a **configurable relay map** from day one with a documented **self-hosted relay +
   mDNS-only fallback**, so the deployment never hard-depends on third-party relay infrastructure.

The whole substrate sits behind a `collab.p2p` config gate; v0.14 hub mode remains the default until the
mesh is proven.

## Configuration, install & activation

P2P must be a **configurable, opt-in capability the user enables once**, after which the daemon
**activates the required features itself** — no manual wiring. Three surfaces, all reusing existing
infrastructure:

1. **Daemon config** — a new `[collab.p2p]` table in `daemon.toml` (alongside the existing
   `[collab]`/`[collab.auth]`/`[collab.storage]`/`[collab.sync]`, `daemon/src/config.rs`):
   `enabled` (bool, default false), `relay` (`default` | `self:<url>` | `none`), `discovery`
   (`dns-pkarr` | `mdns` | `both` | `manual`), and optional explicit peer node-IDs for the `manual`
   case. **Identity is *not* duplicated** — the mesh reuses the daemon's `[collab.auth]` `key`-mode
   Ed25519 identity + `authorized_keys`/`trusted_keys` keystore (ADR-017). `--check-config` validates
   the table; legacy configs without it default to disabled.
2. **Editor enablement + wizard** — a `collab_p2p` editor option (OptionRegistry → init.scm, principle
   #8) and an extended **`mae setup-collab --p2p`** (the existing idempotent key-mode wizard,
   `crates/mae/src/main.rs:330`): it generates/loads the Ed25519 identity, writes `[collab.p2p]` +ON,
   and ensures the daemon is installed/running (composing with `setup-daemon`). One command takes a user
   from zero to a P2P-ready daemon.
3. **Install / service** — `make install-daemon-service` + `assets/mae-daemon.service` (the systemd user
   unit) gain P2P-aware defaults (outbound network is required; document the relay/self-host choice);
   `assets/install.sh` notes the P2P opt-in. No extra binary — iroh is linked into `mae-daemon`.

**Activation (daemon-side):** on startup, `mae-daemon` branches on `collab.p2p.enabled`. When on, it
builds the iroh `Endpoint` from the key-mode identity and **activates listener + dialer + discovery +
relay**, dialing known peer node-IDs for each shared KB — *in addition to* (not replacing) the local
Unix-socket + the v0.14 TCP listener, so the editor and hub-mode clients are unaffected. When off, none
of the mesh machinery starts (zero overhead). `mae-daemon doctor` reports P2P status (enabled, endpoint,
discovery, relay reachability, peer count) — the ADR-027 visibility surface, available at the CLI for
headless deployments.

**Hub + mesh coexistence (a supported, expected configuration — invariant).** Enabling the mesh ADDS a
transport; it never replaces hub-mode sharing. When `collab.p2p.enabled`, the daemon runs the iroh
endpoint **and** the v0.14 TCP hub listener concurrently, **over one shared `DocStore` + broadcaster + the
same `authorized_keys`/membership (`KbCollectionDoc`, ADR-018)**. Consequences that MUST hold:
- A KB can be shared **simultaneously** over the hub and the mesh; it is **one CRDT document** — a
  hub-connected peer and a mesh peer edit the same `YDoc` and converge. No fork, no per-transport copy.
- **Membership is transport-agnostic**: a member admitted via the hub is a member for mesh access and vice
  versa (one trust set + one membership doc). A removal/epoch-fence applies to both transports at once.
- `setup-collab --p2p` only flips `[collab.p2p].enabled`; `[collab].enabled` (the hub) is independent and
  untouched, so the two compose by configuration.
- *Current constraint (not a design limit):* the mesh is activated within the hub server path, so it
  presently requires `collab.enabled = true`. "Both" is fully supported; mesh-only (hub off) is a small
  follow-up refactor, out of scope here.

## Mesh access gate — live-reload + configurable connection-trust

The accept gate (`p2p::serve` / `resolve_mesh_peer`) has two properties beyond the Phase-1
identity check:

- **Live-reload (I-10).** `serve` is passed the `authorized_keys` **path** and re-reads it on
  **every accept** (not a startup snapshot), mirroring the TLS path's `ReloadingAuthorizedKeys`.
  So `mae-daemon authorize`/`revoke` — and the Phase-2 TOFU/approve flow that adds a peer to
  `authorized_keys` on join — take effect on the running mesh with no restart. Fail-secure: a
  missing/unreadable file loads empty ⇒ deny.
- **Configurable connection-trust** — `collab.p2p.connection_gate ∈ {authorized_keys, open}`
  (the user's "config between hard-rejection and the invite/approve flow"):
  - `authorized_keys` (**default**, security-forward): hard-reject any peer not already in
    `authorized_keys` at connect — an admin-managed closed mesh.
  - `open`: admit any iroh-authenticated peer to *connect* (we always know *who* via the verified
    `remote_id`) with a **bare-fingerprint** identity; per-KB access stays fully mediated by
    `kb_access` (membership + JoinPolicy), so an unknown peer gets a connection but no KB access
    it isn't entitled to. Enables the frictionless magnet-link join. The default stays closed
    until the pending-approve + signed-invite handling (ADR-026) lands.

Transport is necessary, never sufficient: every applied op/membership change still passes the
ADR-018 gate (incl. the per-KB transport policy) + ADR-026 verification.

## Driving surfaces — CLI, editor & AI-peer parity

Every P2P lifecycle action MUST be drivable from **all** of the surfaces below, so a
human can work from the editor *or* the shell and an **AI peer can drive the whole
flow with no CLI access** (principle #3 — the AI is a peer, not a plugin). This
extends the v0.14 KB-sharing parity (Scheme `(kb-…)` primitives + `kb_*` MCP tools +
the `*KB Sharing*` buffer) to the mesh.

For each action — **enable** (`setup-collab --p2p`), **share / mint a join ticket**,
**join from a ticket**, **approve a pending join**, **leave**, **status** — four
surfaces sit over **one** backend:

| Surface | Form |
|---|---|
| CLI | `mae setup-collab --p2p`; `mae-daemon` control verbs |
| Editor command | leader keymap (`SPC C K …`) + the magit-style `*KB Sharing*` buffer |
| Scheme primitive | `(kb-share-p2p …)`, `(kb-join-ticket …)`, `(kb-p2p-status)` … |
| MCP tool (AI peer) | `kb_share` (p2p), `kb_join`, `kb_approve`, `kb_p2p_status` … |

**DRY / single source of truth (#8):** all four resolve to the **same daemon control
method** over the existing Unix control socket (e.g. `p2p/mint_ticket`,
`p2p/join_ticket`). The editor command, the Scheme primitive, and the MCP tool are
thin shims over **one** editor-side action that issues the `DaemonClient` call; the
CLI issues the same JSON-RPC directly. No surface carries logic the others lack —
adding an action is one backend method + four trivial bindings, so the human and the
AI peer can never diverge. Authorization for mint/join is the **daemon owner's**
(local control socket); remote trust stays at the mesh accept-gate + pending-approve.
The read/introspection half of parity (the `*Mesh*` buffer, `collab-doctor`,
`kb_sharing_status` mesh fields) is ADR-027.

## Discovery, join tickets & connectivity lifecycle

**Identity-addressed, never IP-addressed.** A peer is its **node-id** (the Ed25519 fingerprint, =
`authorized_keys` principal); every IP/relay address is only a *routing hint*. The QUIC/TLS handshake
proves the dialed key, surfaced as `Connection::remote_id()` — so we **always dial/accept by node-id and
gate on `remote_id() ∈ authorized_keys`, and never derive identity or trust from an address.** This is
the invariant the whole section rests on.

**Bootstrap — the MAE KB-join ticket ("magnet link").** iroh-base 1.0 dropped the built-in node ticket,
but `EndpointAddr` (node-id + relay URL + direct addrs) is serde-serializable, so a ticket is a small MAE
construct that can also carry what an iroh ticket couldn't: the KB-id and an invite. A ticket is a
base32/URL string (`mae://join/<blob>`) the owner mints with `kb-share --p2p <kb>` (or the `*KB Sharing*`
buffer) and shares out-of-band (Signal, email, …):

- the owner daemon's **`EndpointAddr`** — reachable *immediately*, including behind NAT via the relay,
  before any DNS/Pkarr propagation;
- the **KB-id** to join;
- *(layered, see below)* an optional **owner-signed, expiring invite grant** (role + scope).

First-join trust is **layered (decision):** *Phase 2* ships the address-only ticket — the joiner dials by
node-id and lands in the existing pending-join queue (JoinPolicy / `kb_approve`, ADR-018), the owner
approves, and the joiner is written into `authorized_keys` + the membership doc. *Phase 3* (ADR-026) adds
the owner-signed expiring invite *inside* the ticket for pre-authorized one-step joins, verified against
the signed-membership chain — no TOFU window, no manual approve. Tickets are **onboarding only**; after
first contact the node-id is persisted and re-discovery is automatic.

**Staying connected over time (dynamic residential IPs).** The node-id is stable (persisted key); only
routing hints churn on a DHCP-lease renewal or ISP reassignment. Continuity comes from, in order:

1. **Pkarr republish** — each daemon periodically re-publishes its current `EndpointAddr`, *signed by its
   own key*, keyed by node-id, to the Pkarr DHT (n0's `address_lookup/pkarr`). An IP change → republish →
   peers resolving the node-id get the fresh address. Primary "findable over time" mechanism.
2. **Relay as stable rendezvous** — the home-relay URL is stable even when every direct addr dies, so a
   reachable path always exists; iroh hole-punches back to direct when it can. Survives NAT rebinding.
3. **Reconnect + anti-entropy** — the Phase-2 dialer holds a per-peer connection with backoff; on any drop
   (sleep, IP change, outage) it re-resolves the node-id and runs **SV-reconcile (ADR-022)** to catch up.
   Local-first: edits made while disconnected converge on reconnect.
4. **Membership ≠ connectivity** — being a *member* lives in the signed, epoch-fenced `KbCollectionDoc`
   (ADR-018/023/026), **not** in any connection. A peer offline for a week with two IP changes is still a
   member; it reconnects by node-id and syncs. **Removal is an explicit signed membership op, never a
   disconnect/idle timeout** — the v0.14 idle eviction frees *document memory* and MUST NOT be read as
   membership loss; the mesh must not drop members on disconnect. (A *key* change is the only thing that
   needs action — the Phase-5 signed old→new rotation link — because an IP change needs nothing.)

**Mobility — frequent network switching (laptop: work → café → home).** A traveling peer changes
IP/NAT/relay several times a day and is offline between hops. The design makes this **smooth** *and*
keeps it **secure**, with no shortcut at the security boundary:

- **Smooth (the sync layer).** Identity is the stable node-id; only routing churns, so each hop just
  triggers a **Pkarr republish** (short discovery TTL so peers find the new address fast) and, where direct
  paths die mid-hop, falls back to the **home relay rendezvous** while iroh re-hole-punches. Edits made on
  the train (no connectivity) queue locally — yrs is the source of truth — and converge on reconnect via
  **SV-reconcile anti-entropy (ADR-022)**: no spinner, automatic. Reconnect uses **bounded exponential
  backoff**, so unstable café wifi flapping up/down doesn't thrash the dialer; the relay path covers the gap.
- **Secure (re-established on *every* reconnect — never bypassed).** Each reconnect, on *every* network,
  redoes the full iroh QUIC/TLS handshake (`remote_id()` re-verified against `authorized_keys`), passes the
  **live-reloaded mesh gate** (the gate re-reads `authorized_keys` per accept — §Mesh access gate), and
  re-derives membership via `derive_valid_members` (ADR-026). **Consequence that matters: a revoke/removal
  that happened while the laptop was offline is enforced on its very next connect** — the offline window
  cannot be used to slip past a revocation, replay a stale grant (epoch fence + invite timebox), or revive
  a removed peer. A network switch is *never* an authentication event in the attacker's favor: changing
  networks grants nothing that being on the original network wouldn't.

## Adversarial / robustness review

- **Untrusted/revoked peer dials in** → rejected at the iroh-identity↔`authorized_keys` check (same
  principal model as ADR-017); transport auth is necessary but **not sufficient** — content/membership
  authority is enforced by ADR-026, never by mere connectivity.
- **Address / IP spoofing (a peer puts a victim's IP in a ticket, or poisons DNS/Pkarr to advertise its
  own IP under a victim's node-id)** → cannot impersonate: identity is the key, not the address, and the
  TLS handshake yields a `remote_id()` that won't match the expected fingerprint (gate drops it). Pkarr
  records are *self-signed by the node key*, so an attacker cannot publish a record for a node-id it does
  not own. Residual impact is nuisance, not compromise: (a) a wasted RTT dialing a bad hint for a trusted
  node-id (TLS fails → reject → retry via Pkarr/relay); (b) a redirect/**amplification** attempt (listing
  a victim IP as a "direct address") which QUIC path validation (PATH_CHALLENGE before bulk data) blunts.
  No content disclosure or injection — QUIC/TLS is end-to-end; the **relay sees only ciphertext** (it
  learns metadata — which node-ids talk, when — but never content). Invariant: **never derive trust from
  an address; dial node-ids, verify `remote_id()`.**
- **Relay operator is hostile / offline** → relay sees only QUIC-encrypted bytes (and, post-ADR-026,
  signed/verifiable payloads); self-host + mDNS fallback removes the availability dependency. (Content
  confidentiality from a relay is ADR-007/E2E's job, deferred.)
- **iroh API churn / version risk** → the integration surface is a thin adapter over `Endpoint` +
  `Connection` only (no exotic features); pin a version, validate the current API + relay self-host
  story at kickoff. QUIC is an IETF standard — a substrate swap is bounded.
- **Cross-OS divergence (#13)** → no `uname`-gated logic; hole-punch/relay behavior is iroh's, identical
  on macOS + Linux; CI exercises both.

## Consequences

- The daemon gains a peer role; the editor and the JSON-RPC method catalog are unchanged.
- One new significant dependency (iroh + its QUIC stack), offset by **deleting** the need to build/operate
  NAT traversal, relays, and global discovery ourselves.
- Hub assumptions that the mesh invalidates (single `sharer_session_id`, local `connected_clients`
  counting, idle eviction) are re-derived in the Phase-2 mesh work, not here.
- Reviewer guardrail: transport reachability must **never** be treated as authorization — every applied
  op/membership change still passes the ADR-026 verification gate.

## Verification

Unit: the stream adapter round-trips framed JSON-RPC over an in-memory iroh pair; an unauthorized node-ID
is refused. Integration: two daemons on one host establish a direct mesh link and converge a shared KB
(reusing the `sync/*` catalog). NAT/relay: simulated symmetric-NAT pair converges via relay then upgrades
to direct. Cross-OS: macOS + Linux CI for connect + discovery. Gate: v0.14 hub-mode collab tests stay
green with `collab.p2p` disabled (no-regression).

Discovery/lifecycle/spoofing: a join ticket round-trips (mint → parse → `EndpointAddr` + KB-id, no
addr loss); a dial to a hint whose `remote_id()` ≠ the expected/authorized node-id is **rejected** (proves
identity-over-address — extends the Phase-1 `authorize_peer`/`remote_id` tests); a member persists across a
disconnect **and** a simulated address change, reconnecting by node-id and converging via SV-reconcile
without re-approval (proves membership ≠ connectivity); idle *document* eviction does not drop membership.

Surface parity: each lifecycle action resolves to the same daemon control method regardless of surface —
a test drives mint/join via the control method directly and asserts the Scheme primitive + MCP tool reach
the identical backend (no surface-specific logic); CLI and editor paths issue the same JSON-RPC.
