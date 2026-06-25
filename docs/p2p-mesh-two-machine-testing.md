# P2P Mesh — Two-Machine (Alice ⇄ Bob) Testing Guide

> Live validation plan for the **P2P daemon mesh** (`feat/p2p-setup-and-mesh`).
> Each user runs their **own daemon** and they sync **peer-to-peer over iroh QUIC**
> — there is no shared central daemon (unlike the v0.14 hub model). Design:
> ADR-025/026/027/028. Status: [`P2P_MESH_STATUS.md`](P2P_MESH_STATUS.md).
>
> This guide is written so **two autonomous agents/operators ("alice" and "bob"),
> one per machine, can run the whole matrix, take notes, and coordinate** with
> minimal stop-and-go. It bakes in the reproducibility lessons from the v0.14 cycle
> (`docs/collab-kb-sync-testing-lessons.md`).

---

## Part 0 — Working Protocol for Alice & Bob

Read this first. It is how the two machines stay in sync without a screen share.

### 0.1 Roles & topology

- **Alice** = machine **A**, the KB **owner**. Runs a daemon (`mae-daemon`) with P2P
  enabled, and an editor. Alice mints join tickets and approves members.
- **Bob** = machine **B**, the **joiner**. Runs a daemon (`mae-daemon`) with P2P
  enabled, and an editor. Bob joins via a ticket.
- **Both run a daemon** — the mesh is daemon↔daemon (this is the headline difference
  from v0.14, where only one daemon existed). Each editor talks to its *local*
  daemon over the Unix/loopback socket; the daemons sync to each other over iroh.

```
 alice-editor ─unix─ alice-daemon ═══ iroh QUIC (node-id) ═══ bob-daemon ─unix─ bob-editor
                         (owner)                                 (joiner)
```

### 0.2 Note-taking — one file per machine, one shared format

- Alice writes only to `docs/p2p-test-notes-alice.md`; Bob writes only to
  `docs/p2p-test-notes-bob.md`. **Never edit the other machine's file** (avoids merge
  conflicts on the shared branch).
- **Do NOT touch** `docs/collab-test-notes-alice.md` / `-bob.md` — those are the
  v0.14 logs, preserved as case-study material.
- Each note file starts from the template in [§0.6](#06-note-file-template).
- **Every action is one table row** with these columns (copy the v0.14 convention):

  | Field | Example |
  |---|---|
  | **Scenario** | `S4` (see Part 2) — ties the entry to a reproducible step |
  | **Action** | the exact command(s): `mae kb-join mae://join/…`, `kill -9 $(pgrep -n mae-daemon)` |
  | **Expected** | `B's node converges to [ALICE-S4]` |
  | **Actual** | what happened, with evidence (a log line, `kb_get` output, a hash) |
  | **Status** | `✅ pass` / `❌ fail` / `⚠️ unexpected` / `🔧 worked-around` / `⏳ pending peer` |
  | **Evidence** | daemon-log grep, `(introspect)`, `kb_get`, or a content hash |

- **Issue ids:** `P-NN` for P2P-mesh bugs found this cycle (e.g. `P-1`). Record a
  finding as **Symptom → Evidence → Diagnosis (file:line) → Fix direction → Proof
  (commit + test)**. Mark `✅ FIXED (<sha>)` when a test pins it.
- **Probe slugs** tag every edit so it's greppable end-to-end:
  `[<WHO>-<SCENARIO>[-<RUN#>]]`, UPPERCASE, no spaces — e.g. `[ALICE-S4]`,
  `[BOB-S5-2]`, `[A-S6]`/`[B-S6]` for a concurrent pair. Put the slug in the node
  **title** you edit so it appears verbatim in the daemon log + `kb_get`.
- **Daemon-log markers:** before each scenario, append a marker line you can grep
  for, and note the log line number:
  `echo "===== S4 $(date -u +%FT%TZ) =====" >> ~/.local/share/mae/daemon.log` (or
  your `RUST_LOG` sink). Then `grep -n` from that marker.
- **Rotate when the file gets large.** A multi-machine round can run for **a week+**
  (the v0.14 cycle did, and its bob log hit ~2.4k lines) — a single growing file
  becomes costly for an AI tester to re-read each turn. When your active note file
  feels heavy (rule of thumb: **> ~800 lines**), rotate:
  1. Move **resolved `P-NN` issues** and **completed/✅ scenario rows** into a dated
     archive beside it — `docs/p2p-test-notes-<you>.archive-YYYYMMDD.md` — under a
     `## Archived <date>` heading. Commit it (nothing is lost — it's still in git +
     the archive, for the case study).
  2. Keep the **active** file lean: the current `Environment` block, **OPEN** issues,
     in-progress / not-yet-run scenarios, and recent handoffs.
  This keeps each turn's read cheap without losing the audit trail. Archive files are
  never edited again (append-only history); the active file is the working surface.

### 0.3 Information sharing — the branch is the channel

1. **Code + notes** flow through the shared branch `feat/p2p-setup-and-mesh`.
   After any code fix or a batch of note entries: `git add -A && git commit && git
   push`. The other machine `git fetch && git pull`.
2. **Identities, tickets, fingerprints** are exchanged **out-of-band** (Signal,
   email, paste) — they are short. Record them in your note file's *Environment*
   block so both machines have a copy. The mesh **never** needs you to type a long
   secret into the other's terminal.
3. **Handoff blocks.** When you finish a step the other must act on, append a clearly
   marked block to your notes and push, e.g.:

   ```
   ## → BOB: pickup at S2 (branch @ <sha>)
   1. git fetch && git pull && make build && make verify-binary && make install
   2. Join with this ticket:  mae://join/AAAA…   (KB: collabtest)
   3. Expected: your daemon dials alice, you land "pending" (invite policy).
   4. Ping me; I'll approve, then you re-check.
   ```

4. **Daemon logs are the shared ground truth.** Each side tails its *own* daemon log;
   when a claim is cross-machine ("alice received bob's edit"), paste the relevant
   `received → applied wal_seq=…` lines into your notes so the other can verify
   without your screen.
5. **Clocks:** run NTP on both machines. The op-log carries `issued_at`/`expires_at`
   (invite timeboxes); large skew can prematurely expire invites or muddy log
   correlation.

### 0.4 Reproducibility checklist (do this every rebuild)

These are the four things that cost the v0.14 cycle the most time. Non-negotiable:

1. **Verify the running binary is the one you just built** (the #1 time-sink last
   cycle — a stale `~/.local/bin/mae` silently failed fix verification):
   ```bash
   make build && make verify-binary && make install
   # verify-binary fails loudly if a running mae/mae-daemon != the fresh build.
   ```
   Manual equivalent: `sha256sum ./target/release/mae-daemon` vs
   `sha256sum /proc/$(pgrep -n mae-daemon)/exe` (Linux) — must match before retesting.
2. **Reset state between runs without bricking the setup** (move aside, don't `rm`):
   ```bash
   scripts/reset-collab-state.sh        # backs up collab/ + kb/ to *.backup.<ts>
   ```
3. **Pin the commit per scenario.** Record `git rev-parse --short HEAD` in the note
   row. If alice and bob are on different commits, **stop and resync** — version
   drift between daemons causes opaque protocol mismatches.
4. **Control connect timing.** Set `collab_auto_connect=false` (env wins over config)
   when a scenario needs to observe a pre-connect window (offline edits, crash
   recovery); connect explicitly with `:collab-connect` / the dialer poll. Otherwise
   auto-reconnect (~250ms) can drain the queue before you snapshot it.

### 0.5 Fixtures

Reuse the committed KB fixture from the v0.14 cycle (real content beats empty KBs):

- **`tests/fixtures/kb/collabtest/`** — 3 org nodes with unique sentinels:
  - `collabtest:overview` — sentinel `ZEPHYRINE`
  - `collabtest:alpha` — sentinel `QUOKKA`, links to overview
  - `collabtest:beta` — sentinel `NARWHAL`
- Sentinels let either side confirm replication with `kb_search ZEPHYRINE` /
  `kb_get collabtest:overview` regardless of edits.
- **Network values to agree up front** (put in both Environment blocks):
  - iroh ALPN is fixed (`mae-sync/0`); the daemon binds a UDP endpoint.
  - relay mode: `default` (uses n0 relays for NAT traversal) for a real cross-network
    test; `disabled` only works on the same LAN with direct addresses.
  - If on the same LAN behind one NAT, direct dial works; across networks you need
    the relay (or a self-hosted one) — see §1.4.

### 0.6 Note-file template

Create `docs/p2p-test-notes-<alice|bob>.md` with this header (templates are provided
in the repo; copy and fill the Environment block):

```markdown
# P2P Mesh Test Notes — <ALICE|BOB> (machine <A|B>)

> Live log for the feat/p2p-setup-and-mesh two-machine cycle. Companion:
> docs/p2p-mesh-two-machine-testing.md. Sibling: -<bob|alice>.md.

## Environment (this machine)
- Host / OS / IP:
- Branch @ commit:
- mae / mae-daemon version + binary sha256:
- Daemon node-id (fingerprint):
- This editor's identity (fingerprint):
- Peer's daemon node-id (from out-of-band):
- KB fixture: tests/fixtures/kb/collabtest  (sentinels ZEPHYRINE/QUOKKA/NARWHAL)
- iroh relay mode: default | disabled | custom <url>
- Ports/firewall notes:

## Issues (P-NN)
<Symptom → Evidence → Diagnosis (file:line) → Fix → Proof (commit+test)>

## Scenario log
| Scenario | Action | Expected | Actual | Status | Evidence |
|---|---|---|---|---|---|
```

---

## Part 1 — Setup

### 1.1 Build, verify, install (both machines, same commit)

```bash
git fetch && git checkout feat/p2p-setup-and-mesh && git pull
make build              # editor (GUI by default)
make build-daemon       # or: cd daemon && cargo build --release
make verify-binary      # FAILS if a running mae/mae-daemon != the fresh build
make install            # → ~/.local/bin
mae --version && mae-daemon --version    # confirm equal on both machines
```

> If `make verify-binary` / `make build-daemon` don't exist on your checkout, use the
> manual hash check in §0.4 and `cd daemon && cargo build --release && cp
> daemon/target/release/mae-daemon ~/.local/bin/`.

### 1.2 Identities & authorized peers

Each daemon's key-mode identity is **both** its iroh node-id and its membership
signer. The mesh gates inbound peers on `authorized_keys` (unless
`connection_gate=open`).

```bash
# On EACH machine — generate/show the daemon identity:
mae-daemon identity            # prints node-id fingerprint (SHA256:…) — share OOB

# Authorize the OTHER daemon's node-id so the gate admits it:
#   alice authorizes bob's daemon node-id, and vice-versa.
mae-daemon authorize mae-ed25519 <peer-daemon-pubkey-b64> <label>
mae-daemon authorized          # list — confirm the peer is present
```

> **TOFU alternative for first contact:** set `collab.p2p.connection_gate = "open"`
> in `daemon.toml` to admit any iroh-authenticated peer as a bare fingerprint
> (per-KB membership still gates access). Use `authorized_keys` (default) for the
> security-forward path; record which you chose.

### 1.3 Daemon P2P config (`~/.config/mae/daemon.toml`)

```bash
mae setup-collab --p2p          # writes [collab.p2p].enabled=true + key auth, idempotent
```

Resulting `daemon.toml` (verify):

```toml
[collab]
enabled = true                  # hub TCP listener (can coexist with mesh)

[collab.auth]
mode = "key"                    # required for the mesh (Ed25519 identity)

[collab.p2p]
enabled = true
relay = "default"               # "default" (n0 relays) | "disabled" | a custom URL
connection_gate = "authorized_keys"   # or "open" for TOFU
```

Start the daemon (tail its log to a known file for grepping):

```bash
RUST_LOG="mae_daemon=debug,mae_sync=debug,warn" mae-daemon 2>&1 | tee ~/mae-daemon-live.log
```

### 1.4 Network / NAT / relay

iroh is QUIC-over-UDP with hole-punching + relay fallback:

- **Same LAN:** direct dial works; `relay = "disabled"` is fine if both have routable
  LAN addrs. Open the UDP port if a host firewall blocks it.
- **Across networks / behind NAT:** keep `relay = "default"` (n0 public relays) so
  the dial can rendezvous + hole-punch. No inbound port-forward needed.
- The ticket carries the owner's `EndpointAddr` (node-id + relay + direct addrs);
  **identity is the node-id**, addresses are only hints. A dial verifies the
  handshake-proven `remote_id()` against the ticket node-id.

### 1.5 Seed the KB fixture (alice)

```bash
# On alice, register/import the fixture KB so it has the 3 sentinel nodes:
mae   # then:  :kb-ingest tests/fixtures/kb/collabtest    (or your import flow)
#   confirm: kb_get collabtest:overview  → contains ZEPHYRINE
```

---

## Part 2 — Scenarios & Acceptance Criteria

Run in order. Each scenario: **Setup → Action → Expected → Acceptance → Evidence →
Failure mode.** Log one row per action in your note file with a probe slug.

### S1 — Share over the mesh + mint a ticket (alice)

- **Action:** `mae kb-share-p2p collabtest` (or `:kb-share-p2p`, `(kb-share-p2p
  "collabtest")`, MCP `kb_share_p2p`). The command now **establishes the mesh share
  first** (creates/exposes `kbc:collabtest` over p2p, default join policy = invite)
  **then mints** — share the printed `mae://join/…` ticket with bob out-of-band.
  Add `--policy permissive` to let bob auto-join (skip the approval step), or
  `--policy invite` (default) to require alice's approval in S3.
- **Expected:** a `mae://join/…` string is printed (stdout); a share confirmation on
  stderr; the KB's transport policy now exposes p2p.
- **Note (content):** the CLI share establishes the *collection* (membership/policy/
  transport). For real KB **node content** (the `ZEPHYRINE` check in S2), share from
  an editor that has `collabtest` loaded — `:kb-share collabtest` uploads the node
  states — then `:kb-share-p2p` widens it to the mesh. (Headless node-seeding is a
  tracked follow-up; see P2P_MESH_STATUS.md.)
- **Acceptance:** ticket parses (`mae://join/` prefix); alice daemon logs the mesh
  endpoint bound; `kb_sharing_status` shows the KB as p2p-shared.
- **Evidence:** the ticket; daemon log `P2P mesh endpoint bound`.

### S2 — Join: dial, anti-spoof verify, anchor, pull (bob)

- **Action:** `mae kb-join <ticket>` (or `:kb-join-p2p <ticket>`, `(kb-join-ticket
  …)`, MCP `kb_join_p2p`).
- **Expected:** bob's daemon dials alice **by node-id**, verifies `remote_id ==
  ticket node-id`, registers the anchor, and either pulls the KB (if bob is already a
  member / permissive policy) or lands **pending** (invite policy) until alice
  approves.
- **Acceptance (after approval if needed):** bob's daemon has `kbc:collabtest` +
  `kb:collabtest:*`; `kb_get collabtest:overview` on bob contains `ZEPHYRINE`
  byte-identical to alice's.
- **Evidence:** bob daemon log `mesh peer: synced KB nodes=N`; `kb_search ZEPHYRINE`
  on bob.
- **Failure mode:** if bob's daemon isn't authorized on alice and the gate is closed,
  the dial is refused at alice's accept gate (`rejecting mesh peer`) — authorize it
  (§1.2) or use `connection_gate=open`.

### S3 — Peer-verify membership (bob, no relay trust)

- **Action:** on bob, inspect the derived membership of the pulled KB
  (`kb_sharing_status collabtest`, or the `*KB Sharing*` buffer).
- **Expected:** bob derives **alice = Owner** (and any other members) by replaying
  alice's **signed op-log**, anchored on alice's node-id — not by trusting whatever
  membership a relay supplied.
- **Acceptance:** bob's derived owner == alice's node-id fingerprint; a *forged*
  collection (different anchor) would derive nothing. This is the ADR-026 guarantee.
- **Evidence:** `kb_sharing_status` member list on bob == alice's; daemon registered
  the anchor (`set_kb_anchor`).

### S4 — Inbound live sync (alice edits → bob sees it)

- **Action:** alice edits `collabtest:overview` title to `[ALICE-S4]` and saves.
- **Expected:** bob's copy converges live (no manual re-pull).
- **Acceptance:** within a few seconds `kb_get collabtest:overview` on bob shows
  `[ALICE-S4]`; the content hash matches alice's.
- **Evidence:** alice daemon `kb/node_update applied wal_seq=…`; bob daemon `mesh:
  applied remote update` for `kb:collabtest:overview`; bob editor surfaces it.
- **Failure mode:** if bob doesn't converge, check bob subscribed (kb/join auto-subs
  sync_update as of the snapshot — 2c-3c) and the persistent session is up (no
  `backing off`).

### S5 — Outbound live sync (bob edits → alice sees it)

- **Action:** bob (as an editor/member) edits `collabtest:beta` title to `[BOB-S5]`.
- **Expected:** alice's copy converges live.
- **Acceptance:** `kb_get collabtest:beta` on alice shows `[BOB-S5]`, byte-identical.
- **Evidence:** bob daemon forwards `sync/update`; alice daemon `kb/node_update
  received → applied`; `changed=true` (an honest CRDT merge, not a no-op).
- **Failure mode:** a `changed=false` on a delivered update is the lineage-bug
  fingerprint (see lessons B-14/B-16) — capture it as a `P-NN`.

### S6 — Concurrent edits converge (both)

- **Action:** both go offline-ish (or edit within the same second); alice sets
  `collabtest:alpha` title `[A-S6]`, bob sets it `[B-S6]`; both reconnect/settle.
- **Expected:** CRDT convergence to **one byte-identical value on both peers** (both
  slugs interleaved, no split-brain).
- **Acceptance:** `kb_get collabtest:alpha` on alice == on bob, and contains BOTH
  `[A-S6]` and `[B-S6]`.
- **Evidence:** identical content hash on both sides.
- **Failure mode:** alice shows only `[A-S6]` and bob only `[B-S6]` ⇒ divergence
  (would indicate a client_id/lineage regression).

### S7 — Reconnect / mobility

- **S7a (owner restart):** alice restarts her daemon; bob's session drops and
  **reconnects with backoff**, re-verifies identity, re-anchors, re-syncs. Edits made
  on either side during the gap converge on reconnect.
- **S7b (joiner offline edit):** bob disconnects (kill the daemon or drop the
  network), edits `collabtest:beta` `[BOB-S7B]`, reconnects → the edit reaches alice
  (SV-reconcile, non-destructive merge — no clobber of either side).
- **S7c (network switch):** if feasible, move bob to a different network (Wi-Fi →
  hotspot). The node-id is stable; the session re-resolves + reconnects. **Security is
  re-established on every reconnect** — a member revoked while bob was offline is
  denied on bob's next connect.
- **Acceptance:** after each, both peers converge; no data loss; no duplicate apply;
  a revoke during an offline window is enforced on reconnect (S7c).
- **Evidence:** bob daemon `mesh peer session ended; backing off` → `synced KB`;
  monotonic `wal_seq`; one apply per edit.

### S8 — Membership change, peer-verified (alice manages)

- **S8a (add):** alice adds a third identity (or bob if not yet a member) as Editor.
  Bob re-derives membership and sees the new member; the new member can edit.
- **S8b (remove):** alice removes a member. That member's derived role disappears on
  every peer; their subsequent edits are **denied at the gate** (peer-verified, not
  client-trusted).
- **Acceptance:** the removed member's `kb/node_update` is rejected
  (`DENIED reason=role`/not-a-member) at the *receiving* daemon; their entry is absent
  from bob's derived set.
- **Evidence:** signed Remove op in the op-log; daemon `DENIED`.

### S9 — Revocation: local blocklist (and quorum)

- **S9a (blocklist):** bob locally blocks a compromised principal (even the owner, per
  ADR-026). Bob's daemon stops accepting that principal's ops and drops them from
  bob's derived set — unilateral, immediate, no consensus.
- **Acceptance:** after blocking, bob's derived membership excludes the blocked
  principal; their ops are ignored locally. Blocking the *owner* collapses bob's view
  of that KB (severs locally) — expected.
- **S9b (quorum):** *deferred* — quorum (m-of-n) removal is implemented in `mae-sync`
  but not yet wired into the daemon gate. Mark `⏳ deferred` and note it; cover it via
  the unit oracles for now (`cargo test -p mae-sync --lib quorum`).

### S10 — Spoofing is rejected

- **Action:** construct a ticket pointing at alice's **address** but a **different
  node-id** (or tamper the address bytes) and have bob attempt the join.
- **Expected:** the dial never yields a successful join — iroh dials the (wrong)
  node-id and won't reach alice under that key; the `remote_id != ticket node-id`
  check rejects it. **No anchor is registered on failure.**
- **Acceptance:** bob gets a dial/identity-mismatch error; no `kbc:` doc appears; no
  anchor set.
- **Evidence:** bob daemon `remote identity … != ticket node-id … (spoofed
  address?)`.

### S11 — Transport policy enforcement

- **Action:** alice shares a *different* KB over **hub only** (not p2p); bob tries to
  reach it over the mesh.
- **Expected:** the hub-only KB is **not** mesh-reachable for a non-owner; a p2p/both
  KB is.
- **Acceptance:** bob's mesh join of the hub-only KB is denied (`not shared over the
  P2P mesh`); the owner still reaches their own KB locally (owner-bypass).
- **Evidence:** daemon `kb_access` deny for transport.

### S12 — Unauthorized peer rejected (negative auth)

- **Action:** a third daemon whose node-id is **not** authorized (and gate ==
  `authorized_keys`) dials alice.
- **Expected:** rejected at alice's accept gate before any KB access.
- **Acceptance:** alice daemon `rejecting mesh peer (closed gate, not in
  authorized_keys)`; the peer gets no KB.
- **Evidence:** the reject log line.

### S13 — mDNS LAN discovery (the fast-path)

- **Setup:** alice and bob on the **same LAN**; both editors started with collab on
  (each registers a `_mae-sync._tcp.local` mDNS service via `collab-start`).
- **Action:** on bob, `:collab-discover` (MCP `collab_discover`).
- **Expected:** bob's discovered-peers list includes **alice** (her `user`, the
  resolved `host:port`, `kb_count` from the TXT record); each side **filters out its
  own** service.
- **Acceptance:** alice appears in bob's discovery list with the correct port +
  kb_count; bob never lists himself. Verify the reverse (alice discovers bob).
- **Evidence:** `:collab-discover` output; the daemon's `registered mDNS service` /
  `discovered MAE peer via mDNS` log lines.
- **Note (scope):** mDNS today advertises the **hub** TCP endpoint + `user_name` (it
  predates the mesh). It is the LAN *discovery* fast-path (ADR-025); wiring a
  discovered peer's **iroh node-id** into a mesh dial (so a LAN join needs no manual
  ticket exchange) is a follow-up — for now, exchange the ticket out-of-band (S1/S2)
  even on a LAN. The register→browse→**resolve** round-trip itself is covered by the
  gated `mdns_round_trip_discovers_a_registered_peer` test (`MAE_MDNS_E2E=1`).

### Matrix summary

| # | Scenario | Headline assertion |
|---|---|---|
| S1 | Share + ticket | ticket mints; KB p2p-exposed |
| S2 | Join: dial/verify/anchor/pull | bob pulls KB; `remote_id` verified; anchor set |
| S3 | Peer-verify membership | bob derives owner from signed op-log (anchored), not relay |
| S4 | Inbound live | alice edit → bob converges live |
| S5 | Outbound live | bob edit → alice converges live (`changed=true`) |
| S6 | Concurrent | both converge byte-identical (both slugs) |
| S7 | Reconnect/mobility | converge after restart/offline/network-switch; revoke enforced on reconnect |
| S8 | Membership change | add/remove peer-verified; removed denied at gate |
| S9 | Revocation | local blocklist drops principal (even owner); quorum deferred |
| S10 | Spoofing | wrong node-id never joins; no anchor on failure |
| S11 | Transport policy | hub-only not mesh-reachable; owner-bypass |
| S12 | Unauthorized | non-authorized peer rejected at gate |
| S13 | mDNS LAN discovery | each editor discovers the other's `_mae-sync._tcp` service (self-filtered) |

A scenario **passes** only when the assertion is confirmed on **both** machines with
evidence (a peer's content changed / a deny logged) — never just "an update was
emitted" (lessons §2.5: assert convergence, not enqueue).

---

## Part 3 — Reproducibility helpers

### `scripts/reset-collab-state.sh`

Moves collab + KB state aside (timestamped backup, never deletes), so a poisoned
store from a prior build can't silently stall the next run (lessons B-5). Cross-OS
(XDG-first; honors macOS `Library` fallback). Run between scenarios that need a clean
slate (S2 fresh join, S7 crash recovery).

### `make verify-binary`

Fails loudly if a running `mae`/`mae-daemon` process's on-disk image differs from the
freshly built `target/release` binary — the single highest-value guard (lessons §4.4
cost ~30 min/occurrence). Always run `make build && make verify-binary && make
install` before retesting a fix.

### Daemon-log discipline

Tail to a file (`tee ~/mae-daemon-live.log`), write a `===== S<N> <UTC> =====` marker
before each scenario, and grep from it. Useful greps:

```bash
grep -nE "mesh peer: synced|backing off|rejecting mesh peer" ~/mae-daemon-live.log
grep -nE "kb/node_update (received|applied)|DENIED|REBASE" ~/mae-daemon-live.log
grep -nE "remote identity .* != ticket node-id"            ~/mae-daemon-live.log
```

### What "good" looks like

A clean two-machine run is a **confirmation, not a discovery** — the in-process
loopback-mesh daemon tests already cover dial/pull/inbound/outbound/anti-spoof. The
two-machine run exists to catch what no in-process test can model: real NAT/relay
traversal, real network drops/switches, cross-OS path/clock differences, and the
human/AI-peer UX of the four `kb-share-p2p`/`kb-join` surfaces. File anything new as
a `P-NN`, pin it with a unit/integration test, and the next run won't rediscover it.
