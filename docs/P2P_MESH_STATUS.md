# P2P Decentralized KB Sync — Status

> Status of the **P2P daemon-mesh** initiative on branch `feat/p2p-setup-and-mesh`.
> Design: ADR-025 (transport) / ADR-026 (peer-verifiable integrity) / ADR-027
> (observability) / ADR-028 (data lifecycle). Tracker: issue #96.
> Last updated: 2026-06-25.

## TL;DR

**End-to-end P2P onboarding *and* live collaboration work, with no central server
and cryptographically peer-verified membership.**

```
Owner A:   mae kb-share-p2p <kb>   →   mae://join/… ticket   (shared out-of-band)
Joiner B:  mae kb-join <ticket>
           → B's daemon dials A by node-id, verifies (anti-spoof), anchors,
             pulls the KB, peer-verifies A's membership from the signed op-log,
             and stays LIVE-SYNCED both ways:
                A edits  →  B sees it          (inbound apply)
                B edits  →  A sees it          (outbound forward)
             reconnecting with bounded backoff on any drop (mobility).
```

The remaining substantive design work (quorum in the daemon gate, signed *content*
ops, E2E encryption) is deferred and ADR-tracked. The pure-crypto membership layer
(`mae-sync`) already implements all of ADR-026 (op-log, resolver, cascade,
blocklist, quorum) and is fully tested.

## What works today

| Capability | State | Where |
|---|---|---|
| iroh QUIC transport, node-id = trusted-peer Ed25519 key | ✅ | `daemon/src/p2p.rs` |
| Per-KB transport policy (Hub / P2p / Both), owner-bypass | ✅ | `shared/sync/src/kb.rs`, `daemon/src/collab_handler.rs` |
| Live-reload mesh access gate + `connection_gate` (open / authorized_keys) | ✅ | `daemon/src/p2p.rs` |
| Join "magnet link" ticket (`mae://join/…`), mint + parse | ✅ | `daemon/src/ticket.rs` |
| **Signed membership op-log** (append-only CRDT set, derived validity) | ✅ | `shared/sync/src/membership.rs`, `kb.rs` |
| **p2panda strong-removal resolver** (concurrent, mutual, re-add, cascade) | ✅ | `shared/sync/src/membership.rs` |
| **Local blocklist** (block even the owner) + **quorum governance** (m-of-n) | ✅ | `shared/sync/src/membership.rs` |
| Daemon signs membership ops on mutate (owned KBs) | ✅ | `daemon/src/collab_handler.rs` |
| `kb_access` peer-verifies derived membership for anchored (joined) KBs | ✅ | `daemon/src/collab_handler.rs` |
| **Dialer**: dial by node-id, anti-spoof verify, anchor, pull | ✅ | `daemon/src/dialer.rs` |
| **Live bidirectional sync** (inbound apply + outbound forward, echo-safe) | ✅ | `daemon/src/dialer.rs` |
| Reconnect + bounded exponential backoff (mobility) | ✅ | `daemon/src/dialer.rs` |
| `kb-join` full 4-surface parity (CLI / command / Scheme / MCP) | ✅ | editor + `shared/mcp/src/daemon_client.rs` |
| **`kb-share-p2p` establishes the share *then* mints** (`p2p/share_kb` control method) | ✅ | `daemon/src/handler.rs`, editor + CLI |
| `kb-share-p2p` full 4-surface parity (CLI / command / Scheme / MCP) | ✅ | editor + daemon |
| `mae setup-collab --p2p` | ✅ (prior) | `crates/mae/src/main.rs` |

### Deferred / next (ADR-tracked)

| Item | Notes |
|---|---|
| Quorum governance in the **daemon gate** | mae-sync layer ready; `kb_access` uses single-owner `derive_valid_members`. Switch to `derive_valid_members_governed` once governance is stored owner-signed; add `kb/admin`/`kb/revoke` handlers. |
| Signed **content** ops | ADR-026 part 2. Today content is epoch-fenced (ADR-023); membership is peer-verified, content authorship is not yet. |
| E2E content encryption | A relay still sees plaintext CRDT. BeeKEM/Noise, own ADR. |
| Key/identity rotation propagation | #92. |
| Dedicated mesh **e2e shell script** | The in-process daemon tests already run a real two-endpoint loopback mesh; a full-process `scripts/collab-p2p-mesh-e2e.sh` is the follow-up (the two-machine manual run covers it now). |
| **Node-content seeding in `p2p/share_kb`** | The control-socket share establishes the `kbc:` collection (owner/membership/policy/**transport exposure**) so a peer can join + converge it, but does not yet copy the KB's **node docs** from the daemon KB store into the collab doc_store. Full node content currently flows from the editor's `:kb-share` (which uploads node states over the collab session); a follow-up seeds nodes from `state.store` so the headless CLI share is content-complete too. |
| Data lifecycle (ADR-028) | Signed membership checkpoints + compaction/backup/rollback. |

## Architecture (one paragraph)

Each user runs their own daemon; the daemon's key-mode Ed25519 **identity** is both
its iroh node-id and its membership **signer**. A KB owner signs every membership
mutation into an append-only **op-log** on the `kbc:` collection doc (genesis = the
owner self-admit). Any peer **derives** current membership by replaying that log
against an **external trust anchor** — the owner's node-id from the join ticket —
so a relaying daemon can never forge membership (ADR-026). The dialer turns the
daemon into a sync *client*: it dials the owner by node-id, asserts the
handshake-proven `remote_id()` matches the ticket (addresses are routing hints
only — identity is the key), registers the anchor (which flips `kb_access` to the
derived path for that KB), pulls the KB, and maintains a persistent reconnecting
session that streams edits both ways. Membership ≠ connectivity: an offline peer
stays a member; removal is an explicit signed op.

## Test coverage

All green on `feat/p2p-setup-and-mesh`:

- **`mae-sync`** — 200 lib tests, incl. 29 membership tests (op-log append/converge,
  derivation, strong-removal resolver oracles — concurrent/mutual/re-add/tiebreak,
  cascade, blocklist, quorum). Run: `cargo test -p mae-sync --lib`.
- **daemon** — 95 lib + 42 bin tests, incl. the real two-endpoint **loopback mesh**
  dialer tests (pull + peer-verify, node-id-mismatch reject, **inbound live apply**,
  **outbound forward**), the signed-op-log handler tests, `kb_access` derived-path
  tests, and the **`p2p/share_kb`** control-method tests (create / widen-to-Both /
  no-collab error). Run: `cd daemon && cargo test`.
- **mae-mcp** — broadcast `add_event_sub` (the join-subscribe-window close);
  `DaemonClient` join/mint/**share**. Run from `check` job.
- **editor** — `kb_state` join/share backend delegation tests (`share_p2p` now
  shares-then-mints).

### Validated on two real daemon processes (2026-06-25)

A two-isolated-daemon precheck (alice + bob, separate XDG dirs / identities /
sockets / collab ports, real iroh QUIC) ran the **full onboarding** end to end and
**found + fixed** the load-bearing gap that `kb-share-p2p` previously only *minted*
a ticket without establishing the share (a dialing peer hit *"KB is not shared over
the P2P mesh"* — nothing to pull). With the `p2p/share_kb` fix:

1. alice `kb-share-p2p collabtest --policy permissive` → fresh `kbc:collabtest` on
   the mesh + ticket;
2. bob `kb-join <ticket>` → bob's dialer dials alice, anti-spoof-verifies, **auto-joins
   (permissive) and pulls + persists** the collection;
3. alice widens transport → **propagates live** to bob over the open session.

(Node *content* sync between two real daemons is exercised by the in-process
dialer tests; the editor-driven two-machine run is the manual acceptance gate.)

### CI coverage (what the PR will exercise)

The existing `.github/workflows/ci.yml` already covers the bulk of this work:

| CI job | Covers |
|---|---|
| `check` (stable + nightly) | `cargo test --workspace` → mae-sync membership, mae-mcp, mae-core, mae-scheme, mae-ai + clippy `-D` + fmt |
| **`daemon`** | `cd daemon && cargo test` → **all dialer / p2p / ticket / membership-wiring / collab_handler tests** + clippy + fmt |
| `server-client` | daemon collab tests (MAE_TCP_E2E) + mae-mcp + KB WAL |
| `e2e` | scheme tests + `collab-mtls-e2e.sh` + `collab-membership-e2e.sh` (real daemon spawn) |
| `code-map` | public-API map freshness |

**Coverage gap (follow-up):** no dedicated full-process *mesh* e2e script yet — the
mesh dial/pull/live-sync is covered by the in-process loopback-mesh daemon tests
(real iroh endpoints), not a shell e2e. Tracked above.

## How to test it (two machines)

See **[`docs/p2p-mesh-two-machine-testing.md`](p2p-mesh-two-machine-testing.md)** —
the alice/bob test plan with setup, fixtures, scenarios, acceptance criteria, and
the working protocol for the two machines.

## Source notes / case-study material

The v0.14 two-machine validation cycle (trusted-peer hub KB sharing) produced
detailed working logs, **preserved in git** for a methodology case study — do not
overwrite them:

- `docs/collab-test-notes-alice.md`, `docs/collab-test-notes-bob.md` — raw run logs
- `docs/collab-kb-sync-testing-lessons.md` — the bug-chain + testing-methodology lessons
- `docs/collab-testing-plan.md` — the T0–T7 hub test plan this builds on

The new P2P cycle uses fresh note files (`docs/p2p-test-notes-alice.md` / `-bob.md`)
so the v0.14 logs stay intact as case-study source.
