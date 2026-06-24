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

## Adversarial / robustness review

- **Untrusted/revoked peer dials in** → rejected at the iroh-identity↔`authorized_keys` check (same
  principal model as ADR-017); transport auth is necessary but **not sufficient** — content/membership
  authority is enforced by ADR-026, never by mere connectivity.
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
