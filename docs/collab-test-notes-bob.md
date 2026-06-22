# Collab Test Notes — bob (E, macOS)

Running log from the **machine-E ("bob")** side of the two-machine ADR-017 collab
validation (`feat/crdt-collab-validation`). **Update + commit as we go** so D sees findings.

See [collab-testing-plan.md](collab-testing-plan.md) for the tiers/steps referenced below.

## Logging convention

Every entry is tagged with **where in the test plan** it happened, so issues are
reproducible and we know which code path was under stress:

- **Step** — tier + step from the plan (e.g. `T2.5` = Tier 2 Step 5 "buffer converges";
  `T0` = Tier 0 automated; `T2.4` = Step 4 connect/TOFU).
- **Action** — exactly what was done (command / MCP call / keystrokes).
- **Expected** vs **Actual**.
- **Status** — ✅ pass · ❌ fail · ⚠️ unexpected/needs-investigation · 🔧 worked-around.
- **Repro** — minimal steps + any data that triggered it (e.g. multibyte content).

## Environment

- **E = bob:** macOS (`Marthas-MacBook-Pro`), `192.168.1.132`, dev **GUI** build (`make build`), 0.13.12.
- **D = alice + daemon:** `framework`, daemon `192.168.1.137:9480`, key-mode mTLS.
- **D daemon fingerprint (pinned):** `SHA256:07aWfiNGm690ZcPzxEWvCSTYgkIz+Dw7Db0RPOKK7Ls`
- Policy: `collab_host_key_policy = accept-new` (workaround for #66).
- **Test data in play:** `/tmp/mae-collab-run/collab-demo.txt` — contains an **em-dash `—`
  (U+2014, multibyte UTF-8 / 1 UTF-16 unit)**. Relevant to offset-conversion bugs.

## Run 1 — 2026-06-16 (this session)

Chronological; each row is one observation tied to a plan step.

| # | Step | Action | Expected | Actual | Status |
|---|------|--------|----------|--------|--------|
| 1 | T0 | `make test-collab-{mtls,membership}-e2e` on macOS | green | failed — daemon ignored XDG on mac (`dirs`), scripts used `ss`/`timeout` | ✅ **fixed `a8ac842`** |
| 2 | T0 | re-run after fix + unit tests | green | mTLS 7/7, membership 7/7+7/7, mae-mcp 121, daemon 9, mae --bins collab 94 | ✅ |
| 3 | T2.4 | launch `mae -nw` after `setup-collab` (policy `prompt`) | TOFU prompt → connect | editor froze ~120s then failed | ❌ → **issue [#66]** |
| 4 | T2.4 | switch to `accept-new`, relaunch (GUI) | connect + auto-pin | connected, D key auto-pinned | 🔧 (workaround) |
| 5 | T2.4 | compute pinned fingerprint vs D's `mae-daemon identity` | match | `SHA256:07aWf…7Ls` (awaiting D confirm) | ⏳ |
| 6 | T2.3/4 | `collab-status` after connect | authenticated peer | `connected`, mTLS auth as host (peer reached 2) | ✅ bob authorized |
| 7 | T2.4/5 | observe link during share | stable | **flapping**: `peer closed connection without TLS close_notify` → reconnect (×N) | ⚠️ correlated w/ alice crashes |
| 8 | T2.5 | `collab-list` → join `file:…collab-demo.txt` (`execute-ex`) | buffer appears w/ alice content | joined; `synced_docs:1`; buffer = `collab demo — line from alice (D)` | ✅ **alice→bob receive** |
| 9 | T2.5 | edit bob: `move-to-last-line`→insert→normal (MCP `eval_scheme buffer-insert`) | bob line appears + propagates | inserted line **not visible** on read-back — **twice** (pre- and post-alice-crash) | ⚠️ see I-2 |
| 10 | T2.5 | (during bob edit propagation) | alice shows bob's line | **alice panicked (rope) & crashed** | ❌ see I-1 |

## Issues — detail + repro

## Run 2 — 2026-06-16 (after fix `a57455f`, from scratch)

| # | Step | Action | Result | Status |
|---|------|--------|--------|--------|
| 1 | pre | pull `a57455f`, rebuild GUI, relaunch bob (PID 51874), reconnect MCP | fixed binary, regression tests pass | ✅ |
| 2 | T2.4 | reconnect + re-pin; fingerprint vs D | `SHA256:07aWf…7Ls` **matches** prior pin | ✅ no MITM |
| 3 | T2.5 | join `…collab-demo2.txt` | buffer = `run2: line from alice (D)` | ✅ **alice→bob** |
| 4 | T2.5 | **I-2 probe**: edit bob — found active buffer was `*AI:claude*`, switched (separate step), verified active, inserted | bob's line rendered locally | ✅ **I-2 was a driving artifact, not a bug** |
| 5 | T2.5 | bob's edit propagates to alice | alice shows `run2: line from bob (E)`; **alice did NOT crash** | ✅ **bob→alice** + I-1 fix holds |
| 6 | T2.4/5 | watch link stability | no flapping, no disconnect | ✅ I-7 was a symptom of I-1 |

**Run 2 headline: full bidirectional CRDT sync over mTLS, two machines, confirmed.**

## Issues — detail + repro

### I-1 ✅ FIXED (`a57455f`) — rope panic on double-click word-select  ·  Step T2.5  ·  task #18
- **Actual root cause (not the CRDT path):** double-click word-select in the right pane
  of a **split window** (or past EOL) produced a screen `text_col` far beyond the line
  (live: char index **138 into a 34-char rope**); `char_offset_at` → out-of-bounds offset
  → `word_start_backward`'s `rope.char(p)` panicked. The collab/multibyte angle was a
  red herring — it was unclamped mouse column math.
- **Fix:** clamp `text_col` to the clicked line in `mouse_ops.rs` + guard
  `word_start_backward` (clamp `pos` to `len`) in `word.rs` + 2 regression tests.
- **Verified:** regression tests pass in bob's build; **Run 2 had no crash** after bob→alice.

### I-2 ✅ RESOLVED (not a product bug) — bob edit "not visible"  ·  Step T2.5
- **Cause:** when driving via MCP, the active buffer is `*AI:claude*`, so `buffer-insert`
  targeted the wrong buffer; `switch-to-buffer` in the same burst didn't take before the
  insert. **Fix (test procedure):** `switch-to-buffer` as its own step, verify `active`
  via `list_buffers`, then edit. Confirmed working in Run 2.

### I-2 reconciliation with alice's notes  ·  Step T2.5
- alice independently reattributed I-2 to "`eval_scheme buffer-insert` skips the
  event-loop post-edit flush, so it never reaches the CRDT" (she saw **0 session-7
  updates** from bob's eval insert in Run 1).
- **Run 2 evidence reconciles it:** bob's Run-2 edits *were* `eval_scheme buffer-insert`
  and **did propagate to alice** (user-confirmed: `run2: line from bob (E)` + the SIMUL
  line). So eval edits *do* reach the CRDT once they target the correct buffer.
- **Unified cause:** Run-1's "0 updates / not visible" was the **wrong active buffer**
  (`*AI:claude*`, not shared → nothing to flush). In the live GUI the event loop flushes
  eval edits on the next tick. Net: **not a collab bug**; testing caveat = ensure the
  collab doc is the verified-active buffer before editing via MCP.
- *(Optional polish alice flagged: have MCP `eval_scheme buffer-insert` run the post-edit
  collab flush synchronously for parity with real input — file separately if wanted.)*

### I-3 ⚠️ follow-up (from alice) — split-window clicks use raw, not window-relative coords  ·  Step T2.5
- When `pixel_to_buffer_position` returns `None`, the fallback `handle_mouse_click(row,col)`
  gets **raw screen** coords; in a split the column isn't offset by the pane's x-origin, so
  right-pane clicks map to the wrong column. The I-1 clamp makes it **safe** (no panic; lands
  at line end), but it's a latent correctness bug. Fix idea: subtract focused window
  `area_col`/`area_row` (or resolve via the focused window's fresh layout). Low severity.

### I-7 ✅ RESOLVED — connection flapping was a symptom of I-1  ·  Step T2.4/5
- With the I-1 crash gone, no flapping in Run 2. The earlier `peer closed connection
  without TLS close_notify` churn was alice crashing/restarting, not an independent bug.

### (historical) I-1 original notes
- alice rope panic crash on remote update  ·  Step T2.5  ·  task #18
- **What:** alice's editor panics (rope-related) and crashes when a remote update
  arrives during buffer convergence. Seen ≥2× this run.
- **Where in pipeline:** T2.5 (buffer convergence), on **alice receiving bob's edit**.
- **Scoped:** `shared/sync/text.rs` bridge is clamped/safe (rebuilds rope via
  `Rope::from_str`); suspect **editor-side apply-remote path** (cursor/viewport/selection
  bounds after rope rebuild) in `crates/core/buffer.rs` / `collab_bridge`.
- **Likely trigger:** multibyte `—` (U+2014) offset mismatch (char vs UTF-16 vs byte).
- **Repro (to confirm w/ backtrace):** bob joins shared doc, bob edits a line containing
  `—`, edit propagates to alice → alice panics. Capture on D:
  `RUST_BACKTRACE=1 ./target/release/mae 2>/tmp/alice-crash.log` → `grep -A40 'panicked at'`.
- **Blocks:** clean T2.5 round-trip. **Needs:** D's backtrace.

### I-2 ⚠️ — bob's local edit to a joined buffer not visible on read-back  ·  Step T2.5
- **What:** `buffer-insert` on the joined doc didn't appear in `buffer-string` (2×).
- **Candidate causes (unconfirmed):** (a) edit lost on reconnect/resync rope rebuild
  (link was flapping, I-7); (b) joined-buffer local-edit path; (c) MCP `eval_scheme`
  insert not targeting the joined buffer (note: `(buffer-name)` is undefined in the
  runtime — diagnostic was incomplete; use `get-buffer-by-name`/`buffer-string`).
- **Repro:** join doc, `(switch-to-buffer (get-buffer-by-name "…demo.txt"))`,
  `move-to-last-line`→`enter-insert-mode`→`buffer-insert "x\n"`→`enter-normal-mode`,
  then `buffer-read` → line absent.
- **Note:** may be coupled to I-1 (same CRDT-rope path) and/or I-7 (resync). Re-test
  early in a clean run, **without** flapping, before concluding.

### I-7 ⚠️ — connection flapping  ·  Step T2.4/5
- **What:** repeated `Collab disconnected: connection lost: peer closed connection
  without sending TLS close_notify` → `Connected (0 peers)`.
- **Correlation:** strongly tracks alice crashing/restarting; daemon (separate process)
  stayed up + reachable throughout. **Open Q:** does it reproduce with a stable alice?
- **Repro:** watch `read_messages` during a session; **only conclude a bug if it flaps
  while alice is NOT crashing.**

### Filed
- **[#66] T2.4 — interactive `prompt` TOFU deadlocks TUI / `HostKeyPrompt` unwired.**
  Workaround: `accept-new` (both editors). https://github.com/cuttlefisch/mae/issues/66

## Run 3 — 2026-06-16 (ADR-018 identity-anchored KB access; T2.6)

Both machines rebuilt daemon + editor for ADR-018 (`863d854`→`2ce3ebf`). Membership now
keys on the **key fingerprint**; default join policy **`invite`**; roles owner⊇editor⊇viewer.
Tier-0 re-validated locally first: `collab-membership-e2e.sh` **alice 8/8, bob 7/7**, daemon
log `kb/join: pending → kb/approve_member (editor) → kb/join: complete (3 nodes)`.

| # | Step | Action | Expected | Actual | Status |
|---|------|--------|----------|--------|--------|
| 1 | pre | rebuild daemon+editor (ADR-018), relaunch bob (PID 56128), reconnect MCP | clean | fingerprint `07aW…7Ls` unchanged (no re-TOFU); KB clean | ✅ |
| 2 | T2.6 | bob `kb_join collabtest` (not yet a member) | PENDING (invite) | editor said "Joined (0 nodes)"; daemon recorded **pending**; no local instance | ✅ (see B-1 UX) |
| 3 | T2.6 | (alice `:kb-pending` shows bob's fp → `:kb-approve … editor`) | bob now member | approved by fingerprint | ✅ |
| 4 | T2.6 | bob `kb_join collabtest` again | ALLOWED + 3 nodes | "Joined (3 nodes)" | ✅ **invite→pending→approve→allowed** |
| 5 | T2.6 | `kb_search "ZEPHYRINE"` | → `collabtest:overview` | resolves to overview (+ over-matched alpha, B-2) | ✅ **replication proven** |
| 6 | T2.6 | editor-role write: `kb_update collabtest:overview` (title marker) | allowed (editor⊇edit) | succeeded; returned node w/ full body | ✅ **editor write allowed** |
| 7 | T2.6 | propagation editor→owner | alice sees `[bob edit]` title | ⏳ alice confirming | ⏳ |
| 8 | T2.6 | viewer-role write (after alice demotes bob → viewer) | **rejected** (read-only) | ⏳ not reached | ⏳ |

## Issues — Run 3 (ADR-018 / T2.6)

### B-1 ⚠️ CONFIRMED UX bug — editor shows "Joined (0 nodes)" for pending AND denied  ·  Step T2.6
- The editor status says **"Joined KB 'collabtest' (0 nodes)"** for **three distinct** daemon
  outcomes: (a) pending owner approval (invite), (b) **denied** (restrictive + non-member),
  and (c) a genuine empty join. A user cannot tell access was refused or deferred.
- Confirmed live: bob's `kb-join` after alice **revoked bob + set policy restrictive** showed
  the same "Joined (0 nodes)" even though the daemon **denied** it (alice's daemon log:
  `kb/join denied … collabtest`).
- **Fix:** surface the daemon's decision in the editor — distinct messages for
  pending / denied / joined(N), and don't say "Joined" when access was refused.
- Daemon-side enforcement is correct; this is editor-side wording only.

### B-5 🐛 MED (robustness + concurrency) — `kb_join` stalls the main thread on a malformed KB row  ·  Step T2.6 (Run 4)
- On the clean-restart run, `kb_join collabtest` triggered:
  `failed to load user nodes from primary store error=CozoDB: The tuple bound by variable
  'title' is too short: index 1, length 1`, then **`WATCHDOG: main thread stall ... 10s`** →
  join aborted (`synced_docs:0`, no outcome).
- **Trigger:** stale `collabtest` data persisted in bob's primary store from the prior run
  (B-4 — revoke didn't wipe it; bob's `[bob edit]` title was written by the *pre-I-9 broken*
  write path, likely producing the malformed row). Survives editor relaunch.
- **Two defects:** (1) a malformed KB row makes the load **error** instead of skipping/repairing;
  (2) the failing CozoDB query runs **on the main thread** and **stalls the event loop ~10s**
  (concurrency-principle violation — KB I/O must be off the UI thread).
- **Repro:** have a bad-arity row in `primary.cozo`, then `kb_join` (or any primary-store load).
- **Workaround (this run):** moved `primary.cozo` + `shared/collabtest/` aside
  (`*.malformed.<ts>` / `*.stale.<ts>` under `~/Library/Application Support/mae/kb/`) → fresh KB.

### B-6 🐛 (principle #13) — editor KB store path is NOT XDG-first  ·  cross-platform parity
- Editor primary KB lives at macOS **`~/Library/Application Support/mae/kb/primary.cozo`**
  (via `dirs::data_dir()`), while the editor's **collab identity** is XDG-first
  (`~/.local/share/mae/collab/`). Same inconsistency class as the **daemon XDG bug we fixed
  in `a8ac842`** (CLAUDE.md principle #13): KB data should be XDG-first too, or env-var
  isolation + Linux/macOS parity silently diverge. Latent (not the current blocker), but it's
  the same root cause we already committed a principle about.

### B-4 ℹ️ NOTE (likely intended) — revoked member keeps the local KB copy  ·  Step T2.6
- After alice revoked bob, bob still has the 3 collabtest nodes locally (searchable, incl.
  bob's own `[bob edit]` title). Expected **local-first** behavior — revoke stops future sync
  but doesn't wipe already-replicated data (mirrors `kb_leave` "local copy preserved"). Access
  control is about *future* sync + *write propagation*, not local erasure. Flagging so it's a
  conscious decision, not a surprise (a "forget on revoke" option could be future work).

### B-2 ⚠️ low — `kb_search "ZEPHYRINE"` over-matches `collabtest:alpha`  ·  Step T2.6
- Sentinel `ZEPHYRINE` is unique to `collabtest:overview` (fixture invariant), but search
  returns **overview AND alpha**. alpha links to overview — likely link/neighbor weighting in
  the relevance ranking. Doesn't break the replication proof (overview is the top hit) but
  weakens the "unique sentinel" assertion. Excerpt shown was `:PROPERTIES:` (matched metadata?).

### B-3 ⚠️ MED — joined KB nodes: searchable + writable by id, but NOT in `kb_instances` and `kb_get`-by-id fails  ·  Step T2.6
- After `kb_join collabtest` (3 nodes): `kb_search` finds the nodes with **`instance: null`**;
  `kb_instances` reports **"no external instances registered"**; `kb_get collabtest:overview`
  → **"No KB node"**; yet `kb_update collabtest:overview` **succeeds** (resolves + returns the node).
- ⇒ Inconsistent joined-peer representation: the **read path** (`kb_get`) and the **write path**
  (`kb_update`) resolve joined nodes differently, and the joined KB isn't registered as a tracked
  instance. Open Q for alice (ADR-018 author): should a joined KB surface as a federated
  `collabtest` instance (addressable by id, edits sync back) or merge into local? Needs alignment;
  affects how role/edit-propagation tests are driven.

## Run 4 — 2026-06-16 (after I-9/I-10/B-1 fixes + fresh KB; clean T2.6 from top)

Both rebuilt (`9b72494`→`9dc858e`); bob's malformed KB reset (B-5 workaround); display-rule
QoL detour (#67). Clean re-run:

| # | Step | Action | Result | Status |
|---|------|--------|--------|--------|
| 1 | pre | relaunch bob (PID 59974) fresh KB; fingerprint `07aW…7Ls` | no CozoDB error (B-5 gone); a transient watchdog 10s stall seen (B-7?) | ✅ / ⚠️ |
| 2 | T2.6 | bob `kb_join collabtest` (non-member) | `*Collab Status*`: **"join request sent — pending owner approval"** | ✅ **B-1 fix verified** (distinct pending msg) |
| 3 | T2.6 | (alice `:kb-approve … editor`) → bob `kb_join` again | bob has 3 nodes; `kb_search ZEPHYRINE` → overview+alpha (fresh content) | ✅ **approve→allowed + replication** |
| 4 | T2.6 | `kb_get collabtest:overview` | resolves + returns node (failed pre-I-9) | ✅ **B-3 read path FIXED by I-9** |
| 5 | T2.6 | editor write: `kb_update` title → `[bob editor edit]` | applied locally | ✅ write |
| 6 | T2.6 | propagation editor→owner | **alice found bugs — paused to plan fixes** | ⏳ blocked |

Minor follow-ups seen Run 4:
- **`*Collab Status*` not refreshed on success** — stayed "pending owner approval" after the
  re-join succeeded (B-1-adjacent; success should clear/replace the pending StatusReport).
- **B-7? watchdog 10s stall** on startup/connect (no CozoDB error this time) — distinct from B-5;
  watch whether it's the collab connect blocking the main thread on a fresh KB. Not yet root-caused.
- **B-3 partial:** `kb_get`/`kb_update` now resolve joined nodes, but `kb_instances` still shows
  none + search `instance: null` — joined KB merges into primary rather than a tracked instance
  (may be intended). Read/write paths fixed; only instance-listing remains.

## Run 5 — 2026-06-17 (ADR-019 durable/reconstruction-capable KB sync)

Both rebuilt (`23b73f1`→`5d903d3`); bob KB reset clean again (Run-4 leftovers aside). Used
the new ADR-019 `introspect` (`collaboration`/`kb` sections) to diagnose live.

| # | Step | Action | Result | Status |
|---|------|--------|--------|--------|
| 1 | pre | relaunch bob (PID 63383) fresh KB | `introspect`: shared_kbs=[], stall_count=0 | ✅ (B-7 stall gone) |
| 2 | T2.6 | `kb_join` (non-member) | "join request sent — pending owner approval" | ✅ B-1 |
| 3 | T2.6 | (alice approve editor) → `kb_join` | allowed | ✅ |
| 4 | T2.6 | `kb_instances` | **`collabtest [18b9da6e]: 3 nodes, enabled`** | ✅ **B-3 RESOLVED** (ADR-019 P2 first-class instance) |
| 5 | T2.6 | `kb_search "ZEPHYRINE"` | `instance: "collabtest"` (not null) | ✅ replication + proper attribution |
| 6 | T2.6 | editor write: `kb_update` title + `kb-save` | local change applied; **alice sees no `kb/node_update`** | ❌ **B-8** |
| 7 | T2.6 | disambig: `kb-edit-source collabtest:overview` | **no source buffer opened** (joined KB has no source file) | ⚠️ B-9 |

### B-8 🐛 (critical, P4 frontier) — editor KB-node edit does not enqueue/propagate  ·  Step T2.6
- bob (editor member) `kb_update collabtest:overview` → title changes **locally** (`kb_get`/
  `kb_update` both return the new title), `kb-save` run, but **no `kb/node_update` reaches the
  daemon** → alice never sees it.
- **ADR-019 introspect pinpoints it:** `owning_instances[collabtest].gate_present = true`
  (P1 durable emit gate IS set), but **`pending_kb_updates = 0`** after `kb_update` + `kb-save`
  — the edit is **never enqueued** for emission. So nothing flushes on save.
- **Two hypotheses (for alice):** (1) MCP/AI `kb_update` bypasses the editor's
  KB-edit→collab-emit path (an "AI is a peer" gap — AI edits should emit like human edits);
  (2) shared-KB local edits don't enqueue at all on the normal path. Disambiguation via a
  human-style edit was blocked by B-9 (no source buffer for joined KBs).
- **Suggested next probe:** test the **receive** direction (alice edits a node → does bob
  receive it? ADR-019 P4 `kb_apply_remote_update`). If receive works but emit doesn't, the bug
  is isolated to bob's local-edit **enqueue/emit** path.

### B-9 ⚠️ — `kb-edit-source <joined-node>` opens no buffer  ·  Step T2.6
- `(execute-ex "kb-edit-source collabtest:overview")` produced no source buffer. Joined KBs
  arrive over the wire with no on-disk source file, so `kb-edit-source` has nothing to open —
  blocks the human-style edit path for joined KBs (also blocked the B-8 disambiguation).

### B-10 🐛 (CRITICAL — likely the B-8 root cause too) — joined KB instance has empty `dir`; nodes don't survive restart  ·  Step T2.6 restart-survival
- **Smoking gun (bob startup log, `MAE_LOG=kb_sync=debug,collab=debug`):**
  ```
  "KB instance dir missing, skipping"  name=collabtest  dir=""
  "reconnect: re-subscribing shared KBs"  count=1
  "joining KB"  kb=collabtest        ← no "complete"/snapshot follows; 0 nodes restored
  ```
- After relaunch: `kb_instances` → `collabtest [18b9da6e]: 0 nodes, enabled, dir=` — the
  **instance registration survives** (uuid/enabled/marker) but the **`dir` is empty**, so the
  local node store can't be loaded ("dir missing, skipping") and the reconnect re-subscribe
  **did not restore the 3 nodes** → `kb_get`/`kb_update collabtest:*` now fail ("No KB node").
- **This unifies B-8 + restart-survival under one root cause:** a collab-**joined** instance is
  created with **`dir=""`** (no durable on-disk backing), unlike a **`kb_register`ed** instance
  (real dir) — exactly the difference between alice's *passing* B-8 repro and the *live* failure.
  A dir-less/degraded instance plausibly (a) fails the emit-enqueue (**B-8**) and (b) loses its
  nodes on restart (**B-10**). **Fix direction for alice:** give collab-joined instances a real
  durable `dir` (like `kb_register` does) so they persist + emit; and the reconnect re-subscribe
  must actually re-fetch the node snapshot from the daemon when the local store is empty.
- **Blocks bob's own gate-trace capture:** with 0 nodes, bob can't `kb_update` to fire the
  broadcast-gate trace — relying on alice's trace + this `dir=""` structural lead.

### B-11 ⚠️ UX — `*Collab Status*` buffer takes over the window on launch  ·  startup
- On launch (collab auto-connect), `*Collab Status*` is displayed/focused **instead of the
  dashboard** — seen on **both** machines. alice's `5d903d3` ("reconnect re-subscribe skips
  primary KB — Collab Status launch popup") addressed part of it, but it still pops up. The
  status buffer shouldn't auto-show on launch — it should only appear on explicit
  `:collab-status`. Likely the auto-connect status report force-displays the buffer.

## Convergence + membership scorecard

| Capability | Step | Result |
|-----------|------|--------|
| alice → bob (receive) | T2.5 | ✅ Run 1 + Run 2 |
| bob → alice (send) | T2.5 | ✅ Run 2 (no crash) |
| simultaneous edit | T2.5 | ✅ Run 2 (replicas identical) |
| KB membership: invite→pending→approve→allowed | T2.6 | ✅ Run 3–5 (by fingerprint, mTLS) |
| KB replication to approved peer | T2.6 | ✅ Run 3–5 (ZEPHYRINE) |
| joined KB is a first-class instance (`kb_instances`) | T2.6 | ✅ **Run 5** (ADR-019 P2 — B-3 resolved) |
| joined-node read/write by id (`kb_get`/`kb_update`) | T2.6 | ✅ Run 4–5 |
| editor-role write allowed (local) | T2.6 | ✅ Run 3–5 |
| editor KB edit **propagates** to owner | T2.6 | ❌ **Run 5: B-8** (edit not enqueued; `pending_kb_updates=0` despite `gate_present=true`) |
| owner edit propagates to member (receive) | T2.6 | ⏳ next probe (localize B-8) |
| revoke + restrictive → join denied | T2.6 | ✅ Run 3 |
| viewer-role write rejected | T2.6 | ⏳ not run |
| restart survival (ADR-019) | T2.6 | ⏳ not reached |
| security checks | T2.7 | ⏳ not reached |

## Next run (from scratch)

1. D captures rope panic backtrace (I-1) → fix in `crates/core` → push.
2. Both `git pull --rebase` → rebuild both binaries.
3. Restart daemon (key, `0.0.0.0:9480`, authorize bob) + alice (accept-new) + bob.
4. Re-run **T2.4 → T2.7**; re-test **I-2 early** with a stable link.
5. Log every step's outcome here with the convention above.

---

# Holistic design guidance — shared KB as a durable, replicated CRDT artifact (for alice)

> **Whose insight:** bob = the **peer/joiner** (sees the empty-`dir` instance, restart loss,
> guest-side emit failure); alice = the **owner/creator + daemon** (sees the share path, the
> broadcast gate, daemon storage/broadcast, membership). The holistic fix spans both sides —
> this section is bob's peer-side findings + the target model so alice can drive the repair.
> Grounded in a source read of `kb_ops.rs`, `shared/sync/src/kb.rs`, `shared/sync/src/text.rs`,
> `collab_bridge.rs`, `shared/kb/src/federation.rs`, ADR-019/006/005.

## Target model (the contract we're missing)

A shared KB should be a **propagated artifact replicated on every member's device**, synced
**bidirectionally** through each member's daemon — the *same* model that already works for text
buffers (T2.5 ✅). Per principles **#11 (CRDT-first — "KB nodes are yrs documents")** and **#12
(local-first — daemon is an optimization, not the source of truth)**:

1. Each member holds a **durable local replica** (own on-disk store), usable offline + across restart.
2. Any member's edit → yrs txn → **propagates both ways** via the daemon relay to all members.
3. The daemon is a **sync hub + persistence/discovery** optimization, not required for collab.
4. Reconnect/restart **reconciles** local + remote via **state-vector diff** (merge, not replace).

## ⭐ Replication is a CONFIGURABLE behavioral trait (key design point)

There are **two legitimate, distinct behaviors** — and today's bug is that we silently produce a
broken third state. Make this an explicit, configurable per-KB (owner default) and/or per-member option:

| Mode | Behavior | Use case |
|------|----------|----------|
| **`replicated`** (local-first default) | full durable local copy on the member's device; bidirectional CRDT sync; offline + restart survival | normal shared KBs |
| **`hosted` / remote-only** | **no local replication by design**; member queries/edits against the daemon-hosted instance live; no durable local store | terabyte-scale KBs where full replication is impractical |

**The current defect ≠ either mode:** we *attempt* replication (join pulls nodes into memory) but
**fail to persist durably** (`dir=""`), so we get a broken-`replicated` that loses data on restart —
**not** an intentional `hosted` choice. The repair must (a) make `replicated` genuinely durable, (b)
make `hosted` a real, explicit alternative, and (c) in status/errors **distinguish "replication
disallowed by policy" from "replication failed due to a bug"** — never silently degrade one into the other.

## Concrete gaps (file:line) — replicated mode is not durable/bidirectional

- **G1 — joined instance has no on-disk dir.** `kb_register_joined_instance` pushes a `KbInstance`
  with `org_dir = PathBuf::new()` (`kb_ops.rs:495`), vs `kb_register` which gets a real `org_dir`
  + persistent sentinel (`kb_ops.rs:174-291`, `federation.rs:134-189`). → on restart
  "KB instance dir missing, skipping" → 0 nodes.
- **G2 — no startup loader for shared instances.** The primary store loads at startup, but there is
  **no code** that enumerates the shared-KB CozoDB stores and reconstructs `editor.kb.instances`
  from disk. Joined-node persistence is **best-effort** (`kb_ops.rs:453-477`, write-through warns and
  continues on failure) and never reloaded. → nodes lost on restart (**B-10**).
- **G3 — no state-vector reconciliation for KB (all-or-nothing).** `KbJoined` replaces local state
  with the server's full snapshot (`collab_bridge.rs:1392-1447`); reconnect re-join is
  full-snapshot, not a state-vector diff. Text sync does it right (`text.rs` — encode SV → server
  sends only missing ops → `apply_update` merges). → a member's offline/local edits are **lost** on
  reconnect (overwritten by the snapshot) instead of merging.
- **G4 — emit-enqueue is live-state-fragile (B-8).** Node bodies *are* yrs-CRDT
  (`shared/sync/src/kb.rs` `KbNodeDoc`/`KbCollectionDoc`), and the broadcast gate reads durable
  markers (`kb_ops.rs:811-829`, `kb_collab_id_of` 613-629) which *are* set on join
  (`shared=true`/`collab_id`, 484-485). Yet live, `pending_kb_updates` stayed **0** on a joined-KB
  edit. Suspect the node→owning-instance→`kb_collab_id_of` resolution diverges for a
  dir-less/joined instance (vs the passing `kb_register` repro). Alice's gate-decision trace +
  owner-side view should pin the exact branch; bob can't capture its own trace (0 nodes post-restart).
- **G5 — bespoke KB sync vs unified substrate.** KB share/join ships full node states then
  incremental `KbNodeUpdate`s (`collab_bridge.rs:459-548`), a separate orchestration from the
  text-buffer state-vector model. Converging KB onto the same resync/diff path as text would fix
  G3 and reduce divergence.

## Suggested repair (holistic, spans owner + peer)

1. **Unify register & join into one durable artifact.** A member's KB — whether created/registered
   or joined — should land as the *same* first-class instance: real durable `dir` + CozoDB store +
   sentinel, regardless of origin. Joined `replicated` instances must allocate a durable dir and
   **persist received nodes**, not best-effort.
2. **Add a startup reconstruction loader** that enumerates shared-KB stores on disk and rebuilds
   `editor.kb.instances` (so restart survives), then reconnect performs a **state-vector reconcile**
   with the daemon (merge local + remote), mirroring `text.rs`.
3. **Implement the replication-mode option** (`replicated` | `hosted`): `hosted` skips the local
   store by design and routes reads/edits to the daemon; `replicated` does the durable+bidirectional
   path above. Surface the mode in `:collab-status`/introspect and make policy-denied-replication a
   distinct, explicit state (not a silent empty instance).
4. **Make emit symmetric with receive** so a member's edit reliably enqueues + propagates (fix B-8)
   independent of dir/register-vs-join state.

## Restart-survival result (this run)
❌ Not yet: after relaunch the joined `collabtest` reconstructed its *registration* (uuid/enabled)
but with **0 nodes** (`dir=""`) and the reconnect re-subscribe didn't restore the snapshot — so the
durable-replica + reconciliation contract above is the work item.

---

## 2026-06-17 ~15:45 — bob on Stage-1 build (`aaf33f8`) — pre-test baseline + bob-log findings

bob rebuilt + installed from `aaf33f8` (GUI `make build`, v0.13.12), editor-only (connects to
alice's daemon `192.168.1.137:9480`). Launched with `MAE_LOG=info,kb_sync=debug,collab=debug` →
`/tmp/bob-collab.log` (bob can self-tail it; no manual line-grabbing needed this round). Alice
about to pick up. Baseline captured **before** any live edit this round.

### ✅ B-10 (restart survival) looks FIXED on bob's side — disk-first loader works
`kb_instances`: `collabtest [18b9da6e]: 3 nodes, enabled=true, dir=`. So even with **`dir=""`**
(empty org_dir) the instance reloaded **3 nodes from its CozoDB store** on startup — the Phase-3
disk-first loader did its job. `kb_get collabtest:overview` shows sentinel `ZEPHYRINE` intact **and**
title still `[bob editor edit — ADR-019]` — i.e. bob's edit from the *prior* session **survived the
restart locally**. Contrast the previous run above (0 nodes, snapshot lost). ▶ Net: the dir-less
instance now reloads its nodes; restart-survival of bob's *local* state is good. (Still TBD: does
that surviving bob edit actually reach alice — that's the B-8 emit gate, below.)

### bob startup trace (`/tmp/bob-collab.log`) — reconnect path healthy
```
collab connected            address=192.168.1.137:9480  peers=1
reconnect: re-subscribing shared KBs   count=1     ← ADR-019 re-subscribe fired
joining KB                  kb=collabtest          ← bob auto-rejoined on connect
```
No re-TOFU (alice daemon fingerprint unchanged). Auto-rejoin happened without manual `kb-join`.

### ⚠️ main-thread stall during join (new observation, candidate issue)
Right at `joining KB` + agent-terminal spawn, the watchdog logged
`WATCHDOG: main thread stall detected stall_seconds=6` then `prolonged stall … stall_seconds=10`
(`introspect` later shows `stall_count:0`, so it recovered). Suspect the KB **join / disk-first
load / merge is running synchronously on the main thread**. Non-fatal now, but it'll get worse with
bigger KBs — flagging for owner-side review (move join/load off the UI thread).

### ⭐ B-8 hypothesis — `kb_sync_mode: "on_save"` may gate emit on a save event that never fires
`introspect.collaboration` baseline:
```json
{ "collab_status":"connected", "kb_sync_mode":"on_save",
  "owning_instances":[{ "collab_id":"collabtest","gate_present":true,"shared":true }],
  "pending_collab_intent":false, "pending_kb_updates":0,
  "shared_kbs":[{ "kb_id":"collabtest","node_count":3 }] }
```
Gate IS present (`gate_present:true`) and bob holds collabtest as a shared owning instance — so the
durable markers are set. But `kb_sync_mode:"on_save"` is the *sync-trigger* axis. **Hypothesis:** a
live `kb_update` (MCP) writes the node directly and never triggers a buffer **save**, so an
on_save-gated emit never enqueues → `pending_kb_updates` stays 0 → 0 daemon lines. This would
reconcile alice's divergence: her unit repro (`b8_repro_registered_kb_edit_enqueues`) calls the
enqueue path directly, but the live MCP path under `on_save` never reaches it.
▶ **Test (this round):** drive `kb_update` → re-`introspect` `pending_kb_updates`; if 0, fire manual
`collab-sync` and re-check. If the manual sync makes it propagate, the fix is to make KB-node edits
(MCP + interactive) trigger the emit regardless of `on_save` (or treat a node mutation as a save
event for sync purposes). `introspect.collaboration.pending_kb_updates` is the clean in-band probe.

### Step 1 (alice → bob receive) — ❌ FAIL (B-8 confirmed from owner side)
alice applied a title edit (`[STAGE1-ALICE-RECV-1]`) to `collabtest:overview` and reported
**daemon-side failures**. bob-side confirmation:
- bob's `collabtest:overview` title **unchanged** (`[bob editor edit — ADR-019]`); no
  `[STAGE1-ALICE-RECV-1]`.
- bob's `/tmp/bob-collab.log` **unchanged at 92 lines** — zero inbound, no `kb/node_update`
  received, no merge applied.
▶ So the edit never reached the wire (died on alice's emit/daemon path); **bob's receive path was
not even exercised**. The B-8 emit gap reproduces from the **owner** side too, consistent with the
`on_save`/enqueue hypothesis above. **Holding** for alice's emit-pipeline fix push. Next: re-pull +
rebuild on her push, then re-run step 1 (receive) before step 2 (bob → alice emit).

---

## 2026-06-17 ~16:50 — bob on B-8-fix build (`9a3b973` / fix `95295a2b`) — re-test prep

bob rebuilt + installed from `9a3b973` (GUI). B-8 root cause was **NOT** the `on_save`
hypothesis — it was a **wire-protocol bug**: `kb/node_update` was hand-rolled as a JSON-RPC
*notification* (no `id`), and the daemon drops unrecognized no-`id` messages. Now a proper
request via the shared `shared/sync/src/wire.rs` builder. (My on_save lead → disproven; keeping
the note as a record of the diagnostic path.)

### ⭐ NEW BUG — B-12: pending→approved transition does NOT auto-(re)subscribe the member
Reproduced cleanly this session:
1. alice restarted her daemon → membership reset → bob's auto-rejoin on reconnect landed **pending**
   (invite policy). Because the join was pending (not approved), bob **never subscribed** to the KB
   docs.
2. alice approved bob (editor). The daemon broadcast the collection-doc update, but bob logged:
   `ignoring sync_update for unsubscribed doc  doc=kbc:collabtest` — i.e. **the approval broadcast
   was dropped** because bob isn't subscribed to `kbc:collabtest`.
3. bob had to **manually re-issue `kb_join collabtest`** for the subscription to establish.

▶ **Impact:** after a member's join is approved, they silently receive nothing until they manually
re-join — there's no signal to the member that approval happened, and the approval's own broadcast
is discarded. **Expected:** approval should either (a) push a join/subscribe-trigger to the member,
or (b) the member should auto-retry the pending join on receiving an approval/`kbc:` membership
update (subscribe-then-apply, not drop). Owner-side + member-side coordination. File:line for the
drop: the `"ignoring sync_update for unsubscribed doc"` arm in `collab_bridge.rs`. **Workaround for
testing:** manual `kb_join` after approval.

### ✅ Phase-2 merge-on-join CONFIRMED (offline edit preserved, not overwritten)
The manual re-join completed and **merged** rather than overwrote:
```
joining KB collabtest
KB joined — merging into local store      node_count=3  collection_bytes=867
join: registered first-class instance (merged)  uuid=18b9da6e…  merged=3   (target=kb_sync)
KB join complete (merged)                 node_count=3
```
Post-merge `kb_get collabtest:overview` → title **still** `[bob editor edit — ADR-019]` (bob's
local edit survived the join merge) and sentinel `ZEPHYRINE` intact. This is the ADR-020 Phase-2
contract working: join applies via CRDT `apply_update`, local edits are not clobbered.

### ⚠️ B-11-adjacent — main-thread stall during join STILL present on this build
Same as the prior baseline: at startup `joining KB` the watchdog logs
`stall_seconds=6` → `prolonged stall stall_seconds=10` (recovers, `stall_count:0` after). The
join / disk-first load / merge appears to run **synchronously on the main thread**. Non-fatal at
3 nodes but will scale badly. Tracking as an owner-side perf item (move join off the UI thread).

### State now: bob subscribed (joined+merged), ready for Step 1 receive re-run
`introspect.collaboration`: connected, `kb_sync_mode:on_save`, `gate_present:true`,
`pending_kb_updates:0`, `shared_kbs:[collabtest:3]`. Title baseline `[bob editor edit — ADR-019]`.
Awaiting alice's `[STAGE1-ALICE-RECV-1]` title edit → expect inbound `sync_update`/`node_update`
for `kb:collabtest:overview` on bob + her daemon `kb/node_update: received` + `applied wal_seq=…`.

### Step 1 re-run — ✅ B-8 EMIT FIXED, ❌ NEW B-13: join doesn't subscribe to live node-doc updates
alice fired two title edits (`STAGE1-LIVE-RECV-1`, then `STAGE1-LIVE-RECV-2`). bob result:
- **bob's stored title = still `[bob editor edit — ADR-019]`** — NEITHER slug applied.
- **RECV-1: arrived on the wire, then DROPPED.** `14:53:55 ignoring sync_update for unsubscribed
  doc doc=kb:collabtest:overview`. ⇒ **the emit fix works** — a node update now traverses the wire
  end-to-end (this is the half that was 100% dead pre-`95295a2b`). But bob isn't subscribed to the
  node doc, so it discards it.
- **RECV-2: never arrived at bob** — zero inbound log lines after the `14:53:57` re-join.

**Asymmetry ⇒ both sides of subscription are broken:**
1. *Member side* — a completed `kb/join` merges a one-time snapshot (`KB join complete (merged)`)
   but does **not** establish a live subscription to the node doc(s); a subsequent inbound
   `sync_update` for `kb:<node>` hits the `"ignoring sync_update for unsubscribed doc"` arm
   (`collab_bridge.rs`) and is dropped. (RECV-1.)
2. *Daemon side* — after join the daemon apparently does **not** add bob to the node doc's
   subscriber/broadcast set, so a later edit isn't even sent to bob. (RECV-2 — no inbound at all.)

This is the **receive counterpart to B-8**: ADR-020 Decision 1 says the joining session must
`track_client_connect` + **`subscribe_doc`** for the collection **and node docs**. Emit was fixed;
the **subscribe_doc on join (both the collection `kbc:` AND each node `kb:<id>`) is missing/partial**
— so a member never receives live edits. Same gap surfaced earlier for the collection doc
(`kbc:collabtest`, the approval broadcast, B-12). ⇒ **B-13: join must subscribe the member to the
collection + node docs (member-side local subscription set) AND the daemon must register the joining
session as a subscriber of those docs**, mirroring the text-buffer share/subscribe path. Until then
receive is non-functional even though emit works. Owner+member coordination; primary file
`collab_bridge.rs` (the unsubscribed-doc drop arm + the join handler's subscribe step) + daemon
`collab_handler.rs` (subscriber registration on `kb/join`).

#### B-13 NARROWED → member-side-only (daemon delivery confirmed working)
A 3rd fresh alice edit (after the `14:53:57` completed join) **did reach bob this time**:
`14:56:21 ignoring sync_update for unsubscribed doc doc=kb:collabtest:overview`. So the **daemon
DID broadcast** the node update to bob (RECV-2 earlier not arriving was a pre-completed-join race) —
i.e. **daemon-side subscriber registration on `kb/join` is working**. bob still **dropped it
locally** (title unchanged, neither slug applied). ⇒ **B-13 is a one-sided, member-side fix**: in the
join handler (`collab_bridge.rs`), after `KB join complete (merged)`, bob must `subscribe_doc` each
node `kb:<id>` (+ collection `kbc:<id>`) into its **local** subscribed-docs set so inbound
`sync_update`s apply instead of hitting the `"ignoring sync_update for unsubscribed doc"` arm.
Net receive-path verdict: emit ✅, daemon delivery ✅, **member-side local subscribe ❌ (the one fix
left for Step 1 receive to pass).**

---

## 2026-06-17 ~17:40 — bob on B-13-fix build (`ab19fb1`/`4602ce4b`) — ✅ B-13 confirmed, ❌ NEW B-14 (no-op merge)

bob rebuilt from `ab19fb1`. As alice warned, her editor restart re-shared `collabtest` and
**clobbered bob's membership (B-12)** → bob's auto-rejoin landed **pending** (no `KB join complete`).
alice re-approved by fingerprint; bob `kb_join` → `KB join complete (merged) node_count=3` at
15:09:09.

### ✅ B-13 FIXED — member now receives + runs the apply path (no more "unsubscribed doc" drop)
alice edited `collabtest:overview` then `collabtest:alpha` (she switched to alpha to decouple from
the overview's clobber). bob log:
```
15:09:53 received sync_update notification  doc=kb:collabtest:overview  wal_seq=427  update_b64_len=1496
15:09:53 recv: applied remote kb update     node_id=collabtest:overview owner=alice-fp  changed=false
15:11:02 received sync_update notification  doc=kb:collabtest:alpha      wal_seq=428  update_b64_len=916
15:11:02 recv: applied remote kb update     node_id=collabtest:alpha     owner=alice-fp  changed=false
```
The subscription fix works: inbound `kb:<node>` updates are received and routed to
`kb_apply_remote_update`. Receive-path now: emit ✅, daemon delivery ✅, member subscribe ✅.

### ⭐ NEW BUG — B-14: inbound CRDT merge is a NO-OP (`changed=false`); content never updates
Both applies report **`changed=false`** and the node titles on bob are unchanged
(`collabtest:overview` still `[bob editor edit — ADR-019]`; `collabtest:alpha` still plain
`Collab Test Alpha` — **no slug**). The update is received + applied but the yrs merge produces no
change, so bob's content/title never reflects alice's edit.

**Key discriminator (thanks to alice testing `alpha`):** alpha is a node **bob never edited**, yet it
*also* merges to `changed=false`. So B-14 is **not** a local-edit conflict — it's **structural**.
Strong hypothesis: **divergent yrs document lineage** — bob's and alice's `collabtest:<node>` are
independently-created `KbNodeDoc`s that share a node-id but **no common ancestor** (each side built
its own doc from the org fixture / prior sessions, with distinct yrs client state). alice's broadcast
is a **delta keyed to her doc's state vector**; applied to bob's unrelated doc it references ops bob
doesn't have, so yrs buffers/ignores it → `changed=false`, no text change. (wal_seq advances on the
daemon, update_b64_len is non-trivial, owner=alice-fp — so a real payload arrives; it just doesn't
mutate bob's divergent doc.)

**Why join didn't fix it:** Phase-2 merge-on-join does `apply_update` of the server snapshot INTO
bob's pre-existing local doc (merge, not replace). Merging two independent lineages doesn't give bob
alice's op-history as a shared base, so later deltas still don't apply cleanly. ▶ **Likely fix
direction (owner/arch):** joined nodes must adopt the **authoritative owner doc lineage** — i.e. on
join, *replace* the member's node doc with the owner's encoded yrs state (or seed both from a shared
deterministic base / re-encode the member's doc against the owner's state vector) so that subsequent
deltas share ancestry and merge as real changes. This is the KB analog of the text-buffer rebuild:
the joined `KbNodeDoc` must BE the owner's doc, not a same-id sibling. Primary surfaces: the KbJoin
snapshot-apply path (`collab_bridge.rs` `KB joined — merging`) + `kb_apply_remote_update` (`kb_sync`)
+ `KbNodeDoc` construction in `shared/sync/src/kb.rs`. Needs alice's owner-side wal_seq/state-vector
view to confirm the lineage divergence.

▶ **Step 1 (receive) status: still RED** — but advanced from "dropped" → "received+applied as no-op".
The remaining blocker is B-14 (doc-lineage / no-op merge), not subscription.

---

## 2026-06-22 ~13:16 — ✅✅ STEP 1 (alice → bob RECEIVE) PASSES on B-14+B-15 build (`8d1e040`/`490d9a3`)

bob rebuilt from `8d1e040`. B-12 clobber recurred (auto-rejoin pending → alice re-approved by
fingerprint → bob `kb_join` → `KB join complete (merged)` 13:15:57).

### ✅ Adopt-on-join (B-14) works — bob's titles snapped to alice's authoritative lineage
Immediately post-join, `kb_get` on bob:
- `collabtest:alpha` → `Collab Test Alpha [ALICE-RECV-PROBE-7]` (was plain `Collab Test Alpha`)
- `collabtest:overview` → `Collab Test Fixture Overview [ALICE-ADR019-PROP]` (was bob's local
  `[bob editor edit — ADR-019]` — bob's divergent local edit **replaced** by alice's lineage)

So join now ADOPTS the owner's doc lineage (B-14 fix) instead of merging same-id siblings; bob
converges to alice's current values for all nodes.

### ✅ Live edit propagates with `changed=true` (the no-op B-14/B-15 is GONE)
alice then made a fresh live edit to `collabtest:alpha`. bob log:
```
13:16:31 received sync_update notification  doc=kb:collabtest:alpha  wal_seq=2  update_b64_len=920
13:16:31 recv: applied remote kb update     node_id=collabtest:alpha  owner=alice-fp  changed=true
```
`kb_get collabtest:alpha` → `Collab Test Alpha [B14-CONVERGE-1]`. **`changed=true`** — the merge is
now a real change, not the prior no-op. Note `wal_seq` reset to 2 (alice re-shared on a fresh
collection lineage this round — consistent with B-12 re-share being destructive; tracking).

### Receive path verdict: GREEN end-to-end
emit (B-8) ✅ · daemon delivery ✅ · member subscribe (B-13) ✅ · adopt-on-join lineage (B-14) ✅ ·
live merge changed=true (B-14/B-15) ✅. **Step 1 (alice → bob) = PASS.**

▶ Next: **Step 2 (bob → alice)** — bob edits a node; owner (alice) must receive it (the B-13 fix also
subscribed the owner to its own node docs). Then restart-survival + offline-merge to close Stage 1.
Still-open: B-12 (re-share clobbers membership + resets collection lineage; needs CRDT-merge share,
not delete+replace) and the main-thread stall during join.

### Step 2 (bob → alice) — emit GREEN at bob+daemon, ❌ owner-side merge fails (NEW B-16, provisional)
bob edited `collabtest:beta` → `[BOB-LIVE-1]` via MCP `kb_update`. bob log (outbound):
```
13:18:50 kb edit: broadcast-gate decision   node_id=collabtest:beta  sync_mode=on_save  gate_hit=true
13:18:51 drain: send kb/node_update (durable)  rowid=3  bytes=558
13:18:51 bg: kb/node_update written to wire (awaiting apply-ack)  req_id=21
13:18:51 kb/node_update: daemon confirmed applied  rowid=Some(3)
```
So the **full ADR-020 emit pipeline works from a guest**: gate fires (even under `on_save`),
durable queue→send→**daemon confirmed applied** (ack-on-confirm). **alice reports the change reached
the daemon but did NOT change her local node** (alice debugging owner-side).

**B-16 (provisional) — owner-side receive/merge no-op (mirror of B-14, not covered by the B-14 fix).**
Hypothesis: B-14's adopt re-establishes shared lineage on the **join** path
(`kb_register_joined_instance`, member side). The **owner's local doc** never adopts. This round
alice's re-share reset the collection to a **fresh lineage** (wal_seq=2). bob joined *after* and
adopted the daemon's current lineage → bob↔daemon share lineage (emit applies). But alice's LOCAL
`collabtest:beta` may still be on her pre-re-share lineage, so the daemon's broadcast of bob's edit
no-ops against alice's divergent local doc — the same `changed=false` failure mode as B-14 but on the
owner. ▶ Likely fix: the owner must also converge its local doc to the shared/daemon lineage
(adopt/rebuild on share or on receive), OR fix B-12 so re-share CRDT-merges (preserving one lineage)
instead of resetting it — which would remove the divergence at the source. Bob-side is fully proven;
this is owner-side. Holding for alice's debug.

---

## 2026-06-22 ~14:17 — ✅✅✅ BIDIRECTIONAL Stage-1 KB sync CONFIRMED on B-16 build (`4a33016`/`1652fcf`)

bob rebuilt from `4a33016`. New `client_id` derivation confirmed live at startup:
`KB CRDT client_id derived from collab identity client_id=13578609092317110898` (no longer the
hardcoded `1`). B-12 clobber recurred (auto-rejoin pending → alice re-approved → bob `kb_join` →
`KB join complete (merged)` 14:16:41). Adopt snapped bob's `collabtest:beta` back to alice's fresh
canonical lineage (plain `Collab Test Beta`, bob's old `[BOB-LIVE-1]` replaced).

### ✅ Step 2 (bob → alice) NOW PASSES — owner-side merge works (B-16 fixed)
bob edited `collabtest:beta` → `[BOB-LIVE-2]`. bob outbound (full ADR-020 pipeline):
```
14:17:09 broadcast-gate decision  node_id=collabtest:beta  sync_mode=on_save  gate_hit=true
14:17:09 drain: send kb/node_update (durable)  rowid=4  bytes=565
14:17:09 bg: written to wire (awaiting apply-ack)  req_id=15
14:17:09 kb/node_update: daemon confirmed applied  rowid=Some(4)
```
**alice confirmed: her local `collabtest:beta` updated to `[BOB-LIVE-2]` with `changed=true`.** The
B-16 canonical persisted share-lineage means alice's local doc shares bob's lineage → owner-side merge
is a real change, not a no-op. B-16 closed.

### 🎯 BIDIRECTIONAL Stage-1 = GREEN
- **Step 1 (alice → bob):** ✅ adopt-on-join + live `changed=true` (`[B14-CONVERGE-1]`).
- **Step 2 (bob → alice):** ✅ emit→daemon→owner-apply `changed=true` (`[BOB-LIVE-2]`).

Full pipeline proven both ways: gate → durable queue → wire → daemon apply (ack-on-confirm) →
broadcast → peer subscribe → adopt/shared-lineage → CRDT merge `changed=true`. The B-8→B-16 chain
(emit notification-vs-request, member subscribe, member adopt-lineage, emit-chain stale fields, owner
persisted-lineage, hardcoded client_id) is resolved for the **sequential two-peer** case.

### Remaining for Stage-1 sign-off
1. **B-12** (membership durability) — alice's restart/re-share clobbers membership (bob → pending each
   round) AND historically reset the collection lineage. alice is fixing now (re-share must
   CRDT-merge, not delete+replace). Until then every round needs a manual re-approve + re-join.
2. **Restart-survival** — restart bob → joined nodes reload (disk-first) + edits still flow both ways.
3. **Offline-merge** — edit while disconnected → merges on rejoin, not overwritten.
4. **Main-thread stall during join** (6s→10s watchdog every join) — still present; perf item.
5. **client_id collision under *concurrent* edits** — fix makes ids unique; still untested under true
   simultaneous two-peer edits (latent, per alice's production-fidelity note).

▶ Holding for alice's B-12 fix, then resume with restart-survival + offline-merge + concurrent-edit.
