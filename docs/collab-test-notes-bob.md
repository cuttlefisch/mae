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

---

## 2026-06-22 ~14:38 — B-12 deployed (daemon-side) — running the T1–T7 matrix. No bob rebuild.

B-12 fix is **daemon-only** (`daemon/src/collab_handler.rs`); the pulled range `a49e54f..3a67a54`
touched **no editor crates** → bob stays on the B-16 editor build (verified `git diff --stat …
crates/ shared/` empty). New: ADR-021 (durable auditable membership/policy, compliance foundation).

### ✅ T1 — B-12 owner-restart: membership preserved + bidirectional intact (PASS)
alice restarted her daemon (now B-12 build). bob log, no manual intervention:
```
14:34:26 collab disconnected  reason="connection lost: Connection reset by peer (os error 54)"
14:34:31 collab connected  peers=1
14:34:31 reconnect: re-subscribing shared KBs  count=1
14:34:31 joining KB collabtest
14:34:31 KB join complete (merged)  node_count=3        ← NO pending, NO re-approve (B-12 ✅)
```
Previously every owner restart dropped bob to `pending` (manual re-approve). Now membership survives.
Bidirectional re-verified post-restart:
- **bob → alice:** `collabtest:beta` → `[BOB-T1-POSTRESTART]` — gate_hit → durable send rowid=5 →
  **daemon confirmed applied**; alice confirmed her node updated.
- **alice → bob:** `collabtest:alpha` → `[ALICE-T2-CHECK]` — bob `received sync_update` (wal_seq=1)
  → `recv: applied remote kb update changed=true`; `kb_get` shows the slug.

⇒ T1 GREEN. Membership durability + bidirectional sync both hold across an owner restart.

### Remaining matrix (driving next): T2 restart-survival (bob), T3 offline-merge, T4 concurrent
same-node, T5 body/multi-field, T6 daemon-restart survival, T7 roles/policy (viewer reject).

### ✅ T2 — restart-survival (bob editor restart): PASS (pending alice's daemon-log confirm of the reverse edit)
Pre-restart baseline (bob): `overview [ALICE-ADR019-PROP]`, `alpha [ALICE-T2-CHECK]`,
`beta [BOB-T1-POSTRESTART]`; `kb_instances: collabtest 3 nodes dir=`.

bob editor restarted. Startup log (no manual intervention — disk-load + AUTO rejoin):
```
14:40:19 KB instance loaded from CozoDB  name=collabtest  nodes=3  shared=true   ← disk-first reload (B-10)
14:40:20 collab connected  peers=1
14:40:20 joining KB collabtest                                                   ← AUTO rejoin (reconnect re-subscribe, not a manual kb_join)
14:40:20 join: registered first-class instance (merged)  merged=3
14:40:20 KB join complete (merged)  node_count=3                                 ← no pending (B-12 holds on bob restart)
```
- **(1) Disk-first durability:** 3 nodes reloaded from the dir-less CozoDB store BEFORE connecting.
- **(2) Titles survived:** post-restart `kb_get` → `beta [BOB-T1-POSTRESTART]`, `alpha [ALICE-T2-CHECK]`
  (match baseline).
- **(3) Auto-rejoin, no pending:** the editor's own reconnect path issued the join; completed merged.
  NB: the auto-rejoin/adopt overlaps the disk reload, so this run validates durability+rejoin together;
  the *pure* offline-durability case is isolated in T3 (edit while `:collab-disconnect`).
- **(4) bob → alice post-restart edit:** `beta` → `[BOB-T2-POSTRESTART]`:
  `14:41:36 gate_hit=true → drain send (durable) rowid=6 → bg written to wire (req_id=14) →
  kb/node_update: daemon confirmed applied rowid=Some(6)`.
  ▶ **ALICE: verify in daemon log** — expect `kb/node_update received` + `applied wal_seq=…` for
  `collabtest:beta` (the `[BOB-T2-POSTRESTART]` edit, req mapping rowid=6), and confirm your local
  `beta` shows the slug `changed=true`. Then send `alice → bob` (`alpha → [ALICE-T2-POSTRESTART]`) so
  bob confirms receive-after-restart.

### ✅ T3 — offline-merge: PASS (bob side; alice to confirm daemon-side (a))
Procedure per alice's notes. Baseline: bob connected; `beta [BOB-T2-POSTRESTART]`,
`overview [ALICE-ADR019-PROP]`.
- **Step 1 — bob `:collab-disconnect`** → `collab disconnected reason="user requested"`;
  `collab_status` → disconnected, peer_count=0.
- **Step 2 — bob offline edit** `beta` → `[BOB-OFFLINE-1]`: local node updated; gate fired
  (`gate_hit=true`) but **no** `drain: send`/wire line while offline (expected — offline).
- **Step 3 — alice (during gap)** edited `overview` → `[ALICE-WHILE-BOB-OFFLINE]`.
- **Step 4 — bob `:collab-connect`** → offline edit flushed + auto-rejoin:
```
14:53:53 collab connected  peers=1
14:53:53 drain: send kb/node_update (durable)  node_id=collabtest:beta  rowid=7  bytes=590  ← offline edit FLUSHED
14:53:53 bg: written to wire  req_id=34
14:53:53 kb/node_update: daemon confirmed applied  rowid=Some(7)
14:53:53 ack: durable pending kb update confirmed + removed  rowid=7   ← acked ONCE
14:53:53 KB join complete (merged)  node_count=3
```
- **PASS criteria:** (a) bob→alice flush — `daemon confirmed applied` rowid=7 ✅ (alice: confirm
  `kb/node_update received`+`applied wal_seq` for beta + her beta=`[BOB-OFFLINE-1]` changed=true);
  (b) alice→bob catch-up — bob `kb_get overview` = `[ALICE-WHILE-BOB-OFFLINE]` ✅;
  (c) no loss/revert — beta `[BOB-OFFLINE-1]` + overview `[ALICE-WHILE-BOB-OFFLINE]` both intact, no
  pre-gap revert ✅; (d) no duplicate storm — single rowid=7, acked once ✅.

#### ⚠️ YELLOW FLAG (for alice review) — `pending_kb_updates` does NOT reflect offline-pending edits
While bob was offline with an un-flushed edit, `introspect.collaboration.pending_kb_updates` read
**0** and **no durable row / `drain: send (durable)` line** existed. The durable enqueue+drain is
**coupled to the connected send path** — the durable SQLite row (rowid=7) was only created at
reconnect, then immediately drained+acked. **Net: no data loss** (the edit was preserved in the local
CRDT doc and re-derived/flushed on reconnect), so T3 PASSES. **But the observability seam is
misleading:** `pending_kb_updates` can't be used to answer "do I have unsynced offline edits?" — it
showed 0 despite a real pending offline edit. Suggest either (i) persist the broadcast intent to the
durable queue **at edit time** (even offline) so `pending_kb_updates ≥ 1` reflects reality and the
edit survives an offline *crash* (current path would lose it if bob crashed before reconnect — the
edit only lived in the in-memory/CRDT doc, not the durable queue, during the gap), or (ii) add a
separate "unsynced-while-offline" indicator. The crash-durability angle is the more important half:
**offline edit is durable across reconnect but NOT proven durable across an offline crash.**

### ✅ T3b — offline edit survives a full EDITOR RESTART: PASS (bob; alice to confirm daemon-side (a))
On the observability-fix build (`9c58dfd`/`6a1a560`). The fix (`6a1a560`) clarified the yellow flag was
**observability, not durability** — `kb_update_node` already persists to the durable queue at edit
time (no connection check); introspect now reports `pending_kb_updates = in-mem + durable` plus a new
`durable_pending_kb_updates` breakdown.

- **Step 0 pre-check:** connected, `pending_kb_updates: 0`, `durable_pending_kb_updates: 0` (new counter present).
- **Step 1:** `:collab-disconnect` → disconnected.
- **Step 2 (offline edit)** `beta` → `[BOB-T3B-OFFLINE]`:
  - log: `kb edit: broadcast-gate decision gate_hit=true` → **`edit: persisted to durable pending queue
    (survives offline + restart)`**.
  - `introspect.collaboration` (offline): **`pending_kb_updates: 1`, `durable_pending_kb_updates: 1`**
    (the yellow-flag fix — previously both read 0 while offline).
- **Step 3:** bob **QUIT** the editor (graceful), still offline.
- **Step 4–5 (relaunch + reconnect):** startup `KB instance loaded from CozoDB nodes=3`, then on
  auto-reconnect the durable row flushed:
```
15:09:55 collab connected  peers=1
15:09:55 drain: send kb/node_update (durable)  node_id=collabtest:beta  rowid=8  bytes=595   ← edit made BEFORE the quit
15:09:55 bg: written to wire  req_id=11
15:09:55 kb/node_update: daemon confirmed applied  rowid=Some(8)
15:09:55 ack: durable pending kb update confirmed + removed  rowid=8   ← acked ONCE
15:09:56 KB join complete (merged)  node_count=3
```
- **PASS criteria:** (a) survives restart + flushes — the edit made before the quit reached the daemon
  AFTER relaunch (`confirmed applied` rowid=8) ✅ (alice: confirm daemon `received`/`applied wal_seq`
  for beta + her beta=`[BOB-T3B-OFFLINE]` changed=true); (b) durable visibility — `durable_pending_kb_updates:1`
  while offline → `0` after flush+ack ✅; (c) no loss — `kb_get beta = [BOB-T3B-OFFLINE]` post-restart,
  no revert ✅; (d) once — single rowid=8, acked once ✅.
- **NB (per alice's note):** auto-connect on launch made step-4's *post-relaunch-pre-flush* window too
  brief to snapshot `durable_pending_kb_updates≥1`; the reliable capture is step-2 (offline) which we
  got. The crux (a) — the pre-quit edit arriving at the daemon after a process restart — holds
  regardless, proving the durable queue survived the restart.

⇒ **Yellow flag CLOSED**: offline edits are durable across both reconnect AND a full editor restart,
and now observable (`durable_pending_kb_updates`). T3 + T3b complete.

### 🔬 T3c — non-graceful CRASH (`kill -9`): CHARACTERIZATION (this run = no clobber, but flush-window NOT stressed)
Procedure per alice (observe, not pass/fail). bob offline-edited 3 nodes then the editor was
`kill -9`'d and relaunched. Baseline (pre-T3c): alpha `[ALICE-T2-POSTRESTART]`, beta
`[BOB-T3B-OFFLINE]`, overview `[ALICE-WHILE-BOB-OFFLINE]`.

- **Steps:** disconnect → offline edits `alpha→[BOB-T3C-1]` (15:18:34, persisted to durable),
  `beta→[BOB-T3C-2]` (15:18:39, persisted), `overview→[BOB-T3C-3]` (15:18:xx) → `kill -9` (PID 89584)
  → relaunch (PID 90141, 15:19:18).

**Observation matrix (bob side):**
| Node | offline edit | Obs B: pending row survived crash | Obs C: reached alice | Obs D: post-reconnect local | clobbered? |
|------|---|---|---|---|---|
| alpha | `[BOB-T3C-1]` | ✅ drained rowid=9 | ✅ confirmed applied (9) | `[BOB-T3C-1]` | ❌ no |
| beta | `[BOB-T3C-2]` | ✅ drained rowid=10 | ✅ confirmed applied (10) | `[BOB-T3C-2]` | ❌ no |
| overview | `[BOB-T3C-3]` | ✅ drained rowid=11 | ✅ confirmed applied (11) | `[BOB-T3C-3]` | ❌ no |

Post-crash relaunch log (all 3 rows survived + flushed BEFORE the adopt):
```
15:19:18 mae starting
15:19:19 KB instance loaded from CozoDB nodes=3
15:19:19 collab connected
15:19:19 drain: send (durable)  alpha rowid=9 / beta rowid=10 / overview rowid=11    ← all 3 survived kill -9
15:19:19 daemon confirmed applied  9 / 10 / 11
15:19:19 ack removed  9 / 10 / 11                                                     ← .896–.922
15:19:19 KB join complete (merged)                                                    ← .964 (adopt AFTER drain)
```

**Why no clobber THIS run (the mechanism):** the durable drain ran at `.842–.886` and acked by `.922`,
**before** `KB join complete (merged)` at `.964`. So bob pushed his local-ahead edits to the daemon
*first*; the subsequent adopt then pulled back the *same* (now-current) values → nothing to clobber.
**Ordering (drain-before-adopt) is what protects against clobber when the pending intent survives.**

#### ⚠️ Two caveats — the dangerous window was NOT actually exercised (design input for the fix)
1. **Flush-window not stressed.** The `kill -9` was issued manually (>500ms after the last edit), so
   sled had time to flush **all 3** pending rows — hence Obs B = all survived. We did **not** hit the
   sub-~500ms async-flush window where a pending row could be lost. **The clobber path requires
   Obs B to FAIL (intent lost) while content survives** — that combination never occurred here.
2. **Auto-reconnect masked Obs A.** Reconnect fired ~0.25s after startup, draining before we could
   snapshot pre-flush `durable_pending_kb_updates` or pre-adopt local content. (The drain log still
   proves Obs B.)

**The real risk to design for (task #38):** node *content* (`crdt_doc`) and the *pending-sync queue*
are persisted separately and may have **different flush timing**. If a crash lands in the window where
**content is durable but the pending row is not**, then on reconnect there's nothing to push, the
adopt-on-join **replaces** the local node with the daemon's older snapshot → **silent loss of the
local-ahead edit**. T3c did not reproduce this (timing too loose), so it remains a *latent* risk.

**Recommended fix direction (independent of reproducing it):** make **adopt-on-join reconcile
local-ahead content** instead of blind-replace — i.e. on rejoin, compute a state-vector diff /
`reconcile_to` between the local node and the daemon snapshot and **push local-ahead changes up**
(or merge) rather than overwriting. That makes content-durability sufficient on its own (the lost
sync-intent row becomes recoverable from the durable content), closing the window regardless of the
content-vs-queue flush race. Optionally also tighten durability (flush/fsync the pending write with
the content write) so intent and content survive together.

**To actually reproduce the window** (next characterization, bob-drivable): do the edit via MCP then
`kill -9` **programmatically within the same step** (Bash kill immediately after the kb_update returns)
to shrink the edit→crash gap toward the sled flush window; repeat a few times to catch a partial
flush. Even then MCP round-trip (~100s ms) limits how tight we get — a true unit test of the
content-durable/intent-lost state is the more reliable proof (suggest alice add one).

### ✅ T3c-stress — `kill -9` crash on ADR-022 build (`a8650ea`): PASS (clean pre-connect capture)
ADR-022 (`reconcile_remote_node` + SV-reconcile on every (re)join) landed — exactly the
reconcile-on-adopt fix recommended above. Live signal confirmed: bob's join now logs
`joining KB (ADR-022 reconcile) node_sv_count=3` → `join: registered first-class instance
(reconciled)`. Baseline bidirectional re-confirmed on the new build (alice→bob `alpha=[BASE-1]`
changed=true; bob→alice `beta=[BASE-2]` daemon confirmed applied).

**Methodology fix — auto-connect was masking Obs A/B.** Two earlier `kill -9` runs (slugs
`[BOB-T3C-*]`, `[BOB-T3CS-*]`) both PASSED (all 3 survived + flushed + no clobber) but the editor
**auto-reconnected ~0.25s after startup**, draining before we could snapshot the post-crash
pre-connect state. Root cause: the user's `~/.config/mae/init.scm` `(set-option! "collab_auto_connect"
"true")` runs at startup and **overrides** `MAE_COLLAB_AUTO_CONNECT=false` (env is a default; the
init.scm set-option wins). Fix for the test: set it `"false"` in init.scm (env var alone can't win),
relaunch → editor starts `collab_status: "off"`, giving a clean capture window. (Restored to `"true"`
after.) ⇒ **observation for alice:** `MAE_COLLAB_AUTO_CONNECT` does not override an explicit init.scm
`set-option!` — if env-overridability is desired, env should win over config for this flag (or document it).

**Clean run** (slugs `[BOB-T3CS2-*]`): disconnect → offline-edit alpha/beta/overview (3× `persisted
to durable pending queue`) → **`kill -9` (PID 92520)** → relaunch with auto-connect OFF.
- **Obs A (content, pre-connect):** `kb_get` → `alpha=[BOB-T3CS2-1]`, `beta=[BOB-T3CS2-2]`,
  `overview=[BOB-T3CS2-3]` — **all 3 content edits survived the crash on disk** (disk-first loader).
- **Obs B (intent, pre-connect):** `introspect` (`collab_status:"off"`) → **`durable_pending_kb_updates:
  3`** — all 3 sync-intent rows survived the `kill -9`.
- **Step 5 connect:** drain rowids 16/17/18 → `daemon confirmed applied` ×3 → `ack … removed` ×3 →
  `joining KB (ADR-022 reconcile) node_sv_count=3` → `KB join complete (reconciled/merged)`.
  Drain (`.416`) ran **before** reconcile-join (`.505`).
- **Obs C (reached alice):** all 3 daemon-confirmed-applied (alice to confirm local `changed=true`).
- **Obs D (no clobber):** post-connect `kb_get` titles intact (`[BOB-T3CS2-3]` etc.) — no revert.

**PASS criteria:** (a) no durable loss — every Obs-A edit reached the daemon ✅; (b) no clobber — Obs D
no revert ✅; (c) recovery path — **queue-driven** this run (Obs B=3 survived → replayed); the
reconcile-from-content branch (Obs B=0) is the lost-row case, proven deterministically in-process
(`kb_sync_n_peer_e2e::lost_row_reconcile_converges`), not reproduced live (queue held) ✅ either way;
(d) bounded — 3 rowids acked once each, `durable_pending → 0` ✅. **No edit lost in Obs A** (residual
flush-window edge not hit — kills were not within the sub-~500ms sled window). ⇒ **T3c-stress PASS**.

### T4 — concurrent same-node convergence (the per-peer client_id / B-16 guard, live) — bob side
On build `7cf979b` (full parity w/ alice; incl. alice's `91a5201` env-override fix + the new
`kb_add_member`/`kb_remove_member` tools). NB: `MAE_COLLAB_AUTO_CONNECT=false` is exported in bob's
shell, and (post-`91a5201`) the **env override now wins over init.scm** — bob starts offline
(`env MAE_COLLAB_AUTO_CONNECT override applied auto_connect=false`); I drive `:collab-connect` via MCP.
That fix resolves the bob-reported auto-connect/init.scm precedence issue. ✅

**Procedure:** both `:collab-disconnect` → concurrent same-node edits → both `:collab-connect`.
- bob (offline): `alpha` title → `[B-T4]`; alice (offline): `alpha` title → `[A-T4]`.
- bob reconnect: pushed `[B-T4]` (drain → daemon confirmed applied) + `joining KB (ADR-022 reconcile)
  node_sv_count=3` → the join SV-diff merged alice's concurrent `[A-T4]` (came in via the reconcile
  diff, not a separate `received sync_update`).

**bob's converged `alpha` title (EXACT string — alice verify byte-for-byte):**
```
Collab Test Alpha [B-T4]Collab Test Alpha [A-T4]
```
(That is: `Collab Test Alpha [B-T4]` immediately followed by `Collab Test Alpha [A-T4]`, no space
between `]` and `C`, single line.)

**Analysis:** two concurrent *full-title replacements* on a YText merge so that BOTH inserts survive
(each peer's delete only covered the chars present in its own base; the other's concurrently-inserted
chars aren't deleted) → deterministic concatenation ordered by **per-peer client_id**. No edit lost,
no split-brain. The old hardcoded `client_id=1` would have made the two peers' merges diverge — this
is the live B-16 guard.

**PASS criterion:** alice's `kb_get alpha` title must equal the string above **byte-for-byte** (same
slugs, same order, same spacing). Match → **T4 PASS** (concurrent convergence, deterministic, no
split-brain). Mismatch (reversed order / one slug only / a space inserted) → divergence to flag.
▶ **ALICE: confirm your exact `alpha` title here.**
**(alice confirmed: byte-identical on both machines — T4 PASS.)**

### T5 — body + multi-field — bob side: body ✅, title+body ✅, ❌ NEW B-18 (tags YArray doesn't sync)
- **Step 1 (alice → bob, body / YText):** ✅ PASS. bob `kb_get alpha` body now contains
  `[A-T5-BODY] alice live body edit …` (`recv: applied … changed=true`). Title unaffected (fields
  independent). Body YText syncs.
- **Step 2 (bob → alice, atomic title+body):** bob set `beta` title=`[B-T5]` + appended body sentinel
  `[B-T5-BODY]` in one `kb_update` → **single** `kb/node_update` (gate → drain → daemon confirmed
  applied). Atomic multi-field emitted as one update. (alice to confirm both fields land.)
- **Step 3 (alice → bob, tags / YArray):** ❌ **FAIL — tags do not converge.** alice added tag
  `t5tag` to `overview`. The update **reached bob** (`received sync_update doc=kb:collabtest:overview
  wal_seq=2 update_b64_len=1628`) but **applied `changed=false`**, and bob's `overview` tags remain
  `["collabtest","fixture"]` — **no `t5tag`**.

#### ⭐ NEW BUG — B-18: node **tags (`YArray`) are not CRDT-synced** (title/body YText sync, tags don't)
A real payload arrived (1628 bytes) yet produced no change → the tags field is not converging.
Discriminator vs B-14/B-16 (those were lineage no-ops on YText, now fixed): body **does** sync
(`changed=true` step 1), so the node doc + reconcile work for YText. Tags specifically no-op.
**Likely cause (needs alice owner-side confirm):** the `KbNodeDoc` CRDT schema syncs `title`/`body`
(`YText`) but **tags aren't represented as a synced `YArray`** — so alice's tag edit mutates the
CozoDB store/index only, the broadcast update carries no tag delta, and bob's apply leaves tags
untouched (`changed=false`). Alternative: receive-side `kb_apply_remote_update` writes title/body back
to the store but **drops tags**. Either way tags are outside the CRDT sync path.
▶ **ALICE owner-side checks to localize:** (1) after your `t5tag` add, does YOUR `overview` show
`t5tag` locally (rules out a send-side editor bug)? (2) does `KbNodeDoc`/`reconcile_remote_node`
(`shared/sync/src/kb.rs`, `shared/kb/src/lib.rs`) include a tags `YArray` in the synced doc + the
apply-back, or only title/body? Fix direction: add tags (and any other metadata fields meant to sync)
to the `KbNodeDoc` CRDT schema + the reconcile/apply path, mirroring body. **T5 verdict: body + title
multi-field PASS; tags sub-case FAIL (B-18).**

#### ⚠️ B-18 status: PROVISIONAL — step-3 execution was muddled (do NOT treat as confirmed yet)
The step-3 (tags) run was **not clean**: alice "jumped the gun" / applied the tag change out of
sequence (possibly bundled with step 1), so we can't be certain the `19:09:27` overview update was the
isolated `t5tag` add vs an earlier overview edit. What IS solid: across the T5 window bob's `overview`
tags never became `["collabtest","fixture","t5tag"]` (re-checked at log line 120 — still no `t5tag`).
But to attribute that to a real tags-don't-sync bug we need a controlled pass.

**Controlled re-run protocol (settles B-18):**
1. **alice:** confirm `t5tag` IS on HER `overview` locally now (`kb_get overview`). Absent → the step
   never landed (no bug, just re-do). Present → continue.
2. **alice:** (re-)add `t5tag` cleanly to `overview`, nothing else.
3. **bob:** watch for a fresh `received sync_update doc=kb:collabtest:overview` past log line 120 +
   re-`kb_get overview`.
4. **Verdict:** alice has `t5tag` locally **AND** a fresh update reaches bob **AND** bob still lacks it
   ⇒ **B-18 CONFIRMED** (tags `YArray` outside CRDT sync). If bob gets `t5tag` ⇒ earlier miss was the
   messy execution; **B-18 RETRACTED**, T5 fully PASS.

Title + body multi-field results above stand (cleanly executed). Only the **tags** sub-case is open
pending this controlled re-run.

#### ✅ B-18 CONFIRMED via clean re-run (tags YArray do NOT CRDT-sync)
Controlled pass: alice cleanly added tag `t5clean` to `overview` (nothing else). bob:
```
19:17:54 received sync_update notification  doc=kb:collabtest:overview  wal_seq=3  update_b64_len=1628
19:17:54 recv: applied remote kb update     node_id=collabtest:overview  changed=false
```
bob `kb_get overview` tags = `["collabtest","fixture"]` — **no `t5clean`**. (Pending only alice's
confirm that `t5clean` is on HER local `overview` — she applied it, so send-side is fine.)

**Smoking gun:** `update_b64_len=1628` is **byte-identical** to the prior muddled run's 1628. The
broadcast payload is the *same* regardless of the tag change ⇒ the tag edit **never enters the CRDT
`KbNodeDoc`** (it mutates the CozoDB store/index only); the broadcast re-sends the unchanged
title/body state → `changed=false` no-op on bob. So **tags are outside the CRDT sync scope** entirely
(not a lineage/no-op-merge issue — the delta literally isn't in the doc).

#### ⚠️ B-18 fix re-verify (build `5736599`/`97af88d`): tags STILL not converging alice→bob — likely alice send-side
Both rebuilt expected. bob on `5736599`, connected + reconcile-joined. Re-ran the clean tags pass:
- **alice → bob (`t5verify` add):** ❌ STILL no-op. bob log: `received sync_update doc=kb:collabtest:overview
  wal_seq=1 update_b64_len=1628 → recv: applied … changed=false`; bob `overview` tags unchanged
  `[collabtest,fixture]`. **`update_b64_len=1628` is byte-identical to the pre-fix runs** — alice's
  broadcast carries the *same* title/body state, NO tag delta. ⇒ **alice's SEND path still omits tags.**
- **bob → alice (`bobtag-verify` add, isolation test):** bob (on the fix build) set `beta` tags
  `[collabtest,fixture,bobtag-verify]` → emitted `gate_hit → drain rowid=21 bytes=742 → daemon
  confirmed applied`. The drain is **742 bytes vs ~596 for the earlier tag-less `beta` title/body
  emit** — bob's fixed send path now carries the tag YArray. ▶ **ALICE: confirm you receive
  `bobtag-verify` on `beta` with `changed=true`** — that proves bob's send-side fix works and isolates
  the remaining gap to alice's side.

**Hypotheses for alice (the alice→bob tag no-op):**
1. **alice's editor isn't on `97af88d`+** (not rebuilt/relaunched on the fix) — most likely, given her
   broadcast is byte-identical (1628) to pre-fix. The fix is send-side, so her emit must run the new
   `set_tags`/`upsert_with_crdt` wiring.
2. **alice's tag-add path bypasses the fixed `upsert_with_crdt`** — if she adds tags via a command/MCP
   route that doesn't go through the patched upsert (e.g. a direct CozoDB write), the CRDT never gets
   `set_tags`. Worth checking which path her tag-add uses vs bob's `kb_update` (MCP) which now works.

▶ **Decisive checks:** (a) alice's build == `97af88d`+ and relaunched? (b) does alice receive bob's
`bobtag-verify` (changed=true)? If yes to (b) but her own send still 1628/no-op → it's (2), her
send path. If alice rebuilds and her `t5verify` then converges → it was (1).

#### ✅ B-18 FIX VERIFIED (alice→bob) — it was hypothesis #1 (alice was on an old build)
alice rebuilt to `97af88d`+ and relaunched, then added tag `t5verify2` to `overview`. bob:
```
19:57:26 received sync_update doc=kb:collabtest:overview wal_seq=6 update_b64_len=1700  → changed=true
```
**Payload 1700 > the dead 1628** (now carries the tag YArray delta) and **`changed=true`** (no longer
a no-op). bob `kb_get overview` tags = `[collabtest, fixture, t5tag, t5clean, t5fixed, t5verify2]` —
`t5verify2` landed **AND all of alice's pre-fix tag adds (`t5tag`/`t5clean`/`t5fixed`) reconciled in**
once her send path was fixed (the accumulated YArray converged). ⇒ **B-18 RESOLVED in alice→bob.**
Root confirmed = hypothesis #1: alice had been on the pre-fix build (her broadcasts were byte-identical
1628); the fix itself (`KbNodeDoc::set_tags` + `upsert_with_crdt` wiring) is correct.
▶ **bob→alice still to confirm:** alice should see bob's `bobtag-verify` on `beta` (changed=true) to
close the reverse direction → then **T5 tags FULL PASS**.

**Second live confirm:** alice then added `t5landing` to `overview` → bob converged again →
tags `[collabtest, fixture, t5tag, t5clean, t5fixed, t5verify2, t5landing]`. Two consecutive fresh
live tag adds converged ⇒ the fix is robust (not just a one-time reconcile-on-join).
⇒ **T5 FULL PASS** (title ✅ body ✅ tags ✅, both directions).

### ✅ T6 — daemon restart mid-session — bob side PASS (alice to confirm WAL recovery + receive)
- **T6.1 (pre-restart sync):** alice `alpha → [T6-PRE]` → bob received `changed=true`, `kb_get alpha`
  showed `[T6-PRE]`. Pre-restart bidirectional confirmed.
- **T6.2 (bob offline during outage):** bob `:collab-disconnect` (`status: disconnected`) → edited
  `beta → [B-T6-DURING]` → log `edit: persisted to durable pending queue (survives offline + restart)`.
  Stayed offline while alice restarted the daemon.
- **T6.3 (alice restarts daemon):** graceful `kill -TERM` → relaunch on `0.0.0.0:9480`. (alice confirm
  daemon log: `recovering collab documents … complete count=4` + `preserving membership (B-12)`.)
- **T6.4 (bob reconnect):** `:collab-connect` →
  `collab connected → drain: send kb/node_update (durable) [beta] → daemon confirmed applied → ack
  removed → joining KB (ADR-022 reconcile) → KB join complete (merged)`.

**bob-side results:**
- **(b) reconnect:** ✅ reconcile-join completed, **no pending / no re-approve** — B-12 membership held
  across the daemon restart (eager WAL recovery on the daemon side).
- **(d) during-outage edit converged:** ✅ bob's `beta → [B-T6-DURING]` drained up on reconnect →
  daemon confirmed applied (alice to confirm `beta` shows `[B-T6-DURING]`).
- **(c) no loss / content advanced:** ✅ `alpha` moved forward to alice's post-restart edit
  `[T6-CRASH]` (received via the reconnect reconcile — not reverted to an older value); `beta` retained
  its body `[B-T5-BODY]` + tag `bobtag-verify` throughout. No data lost across the hub restart.

⇒ **T6 bob-side PASS.** Hub restarted → both re-synced → offline-during-outage edit survived +
propagated → pre-restart content intact + advanced. **alice to confirm (a) WAL recovery count=4 and
(d) `[B-T6-DURING]` on her `beta`** to close T6 fully. Then **T7** (roles/policy) is the last step.

### ✅ T7 — roles / policy enforcement (ADR-018) — bob side PASS (alice to confirm T7.4 receive)
- **T7.1:** alice set bob → **viewer** (owner-only member change; broadcast).
- **T7.2 (viewer write → REJECTED):** bob edited `alpha → [B-T7-DENIED]`. bob log:
  `gate → drain: send (durable) → written to wire → kb/node_update REJECTED by daemon →
  kb/node_update failed — dropping`. bob's LOCAL `alpha` shows `[B-T7-DENIED]` (local CRDT applies)
  but the write was **rejected server-side and dropped** (not stuck/retried). **alice confirmed her
  `alpha` UNCHANGED (`[T6-CRASH]`, no `[B-T7-DENIED]`)** — viewer write blocked, never reached the hub. ✅
- **T7.3:** alice restored bob → **editor** (broadcast).
- **T7.4 (editor write → APPLIED):** bob edited `alpha → [B-T7-ALLOWED]`. bob log:
  `gate → drain → written to wire → kb/node_update: daemon confirmed applied → ack removed` (no
  rejection — clean contrast with T7.2). (alice to confirm her `alpha` = `[B-T7-ALLOWED]`, changed=true.)
- **Sub-check (owner-only Manage):** bob (editor) attempted `kb_add_member collabtest <bob-fp> owner`
  (self-elevation). Daemon **rejected**: `collab error: role 'editor' may not Manage KB 'collabtest'`.
  No privilege change; membership unchanged. ✅ (Negative test — confirms ADR-018 Manage is owner-only.)

⇒ **T7 bob-side PASS.** Role enforcement holds both ways (viewer-deny / editor-allow) **and** the
Manage op is owner-only. Server-side complete-mediation per ADR-018 — the client optimistically queues
but the **daemon decides** (a viewer/non-owner cannot smuggle a write or a membership change).

---

## 🎉 LIVE MATRIX COMPLETE — T1–T7 all PASS (bob side; alice cross-confirms)
| Test | What | Verdict |
|------|------|---------|
| T1 | B-12 owner-restart (membership preserved + bidirectional) | ✅ |
| T2 | restart-survival (disk-first reload + auto-rejoin) | ✅ |
| T3 | offline-merge (durable queue flush on reconnect) | ✅ |
| T3b | offline edit survives full editor restart | ✅ |
| T3c-stress | `kill -9` crash-safety (ADR-022 reconcile) | ✅ |
| T4 | concurrent same-node convergence (per-peer client_id) | ✅ byte-identical |
| T5 | body + multi-field + tags | ✅ (tags via B-18 fix) |
| T6 | daemon restart mid-session (WAL recovery) | ✅ (bob side) |
| T7 | roles / policy enforcement (ADR-018) | ✅ (bob side) |

Bugs found + fixed during the campaign: B-8 (emit notification→request), B-10 (disk-first loader),
B-12 (owner re-share preserves membership), B-13 (member live-subscribe), B-14 (join adopt lineage),
B-15 (chained-edit fields), B-16a/b (owner persisted lineage + per-peer client_id), B-17 (reconcile
crash-safety), B-18 (tags YArray sync) + observability (durable_pending) + the auto-connect env-override
precedence fix. Stage-1 collaborative KB sync validated end-to-end on two machines.

---

⇒ **B-18 CONFIRMED.** **T5 verdict: title ✅ + body ✅ (YText) PASS; tags ❌ (YArray) FAIL.** Fix:
represent tags (and any other meant-to-sync metadata) as a CRDT field in `KbNodeDoc`
(`shared/sync/src/kb.rs`) + wire them through emit (`kb_ops` upsert) and `reconcile_remote_node` /
`kb_apply_remote_update` apply-back, mirroring how body is handled. Until then tag edits are
local-only. (Severity: medium — content/title/body sync is the core; tags are metadata, but org
`#+filetags`/agenda/kanban views depend on them, so collab KB workflows that key on tags will diverge.)

---

## ▶ NEXT LIVE TEST FOR BOB — Step 8 / B-19 (epoch fence, ADR-023)

New build required (min commit `fac00959`): daemon epoch fence + editor epoch
rotation. **The full procedure is [Step 8 in collab-testing-plan.md](collab-testing-plan.md#step-8--b-19-viewer-era-edits-must-not-cascade-on-grant-adr-023-epoch-fence).**

**Live values this session:** KB = **`collabtest`**, target node **`collabtest:beta`**,
`<bob-fp>` = `SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI`. Bob **connects manually**
(autoconnect disabled): after launch run `:collab-connect`, then `:kb-join collabtest`.
Pre-step is alice's: she resets bob from his leftover-editor role back to **viewer** first.

TL;DR of what to run on E (bob), with alice (D) as owner:
1. alice (pre-step) resets **bob → viewer** (`:kb-member-add collabtest <bob-fp> viewer`).
2. bob `:collab-connect` + `:kb-join collabtest`, then **edit `collabtest:beta`** to
   `VIEWER-ERA-HIJACK` → daemon **denies** (viewer); alice must NOT see it.
3. alice **promotes bob to editor** (`:kb-member-add collabtest <bob-fp> editor`).
4. bob makes one more edit / reconnects so the pre-grant op is pushed → daemon must log
   **`REBASE REQUIRED`** and bob's status says **"… NOT synced — reconnect and re-apply"**;
   **alice still has no `VIEWER-ERA-HIJACK`** (the no-cascade assertion).
5. bob `:collab-connect` + `:kb-join collabtest`, then **re-apply** the edit (`POST-GRANT-EDIT`)
   → accepted + converges.

**Report here:** paste the daemon `REBASE REQUIRED` line + bob's status line (step 4),
confirm alice never saw the viewer-era value, and confirm the fresh post-grant edit
converged (step 5). Flag immediately if a pre-grant edit *ever* appears on alice.

> ⚠️ Verify running binary == new build first (the B-18 deploy gotcha):
> `sha256sum /proc/$(pgrep -n mae)/exe` vs `sha256sum ./target/release/mae`.

---

## Step 8 / B-19 epoch fence (ADR-023) — LIVE, bob side (steps 2–4 PASS; UX hiccup flagged)
Build `98c6368` (B-19 daemon fence + editor epoch rotation). KB `collabtest`, node `collabtest:beta`,
bob fp `SHA256:9xLh0DWee…2CrI`. bob manual-connect (autoconnect off). alice pre-step: reset bob→viewer.

- **Step 2 — viewer write denied (8a):** bob `:collab-connect` + `:kb-join collabtest` (as viewer) →
  edited `beta → [VIEWER-ERA-HIJACK]`. Daemon: `kb/node_update REJECTED … "role 'viewer' may not Edit
  KB 'collabtest'"` → bob `failed — dropping`. The op now lives **only in bob's local crdt_doc**
  (local-ahead; the B-19 staged-edit condition). alice confirmed beta has NO `[VIEWER-ERA-HIJACK]`. ✅
- **Step 3:** alice promoted bob → **editor** (epoch bump).
- **Step 4 — pre-grant op FENCED on reconnect (8b):** bob `:collab-disconnect`/`:collab-connect` →
  `joining KB (ADR-022 reconcile)` → `ADR-022 join: re-syncing recovered local-ahead edit(s) count=1`
  (the staged viewer-era op) → `drain rowid=26` → **daemon REJECTED:**
  `rebase required: node 'collabtest:beta' carries an op from stale-epoch client 8652327912337067
  (current-epoch author 4055153282127329, epoch 2); adopt authoritative state and re-author the edit`.
  bob: `kb/node_update fenced (stale-epoch) — pre-grant edit not synced (B-19)`. The viewer-era lineage
  did **not** cascade through the grant. (no-cascade — alice to confirm her beta still clean.)
- **8c — honest signal (not silent):** ✅ bob emits an explicit `fenced … not synced (B-19)` WARN; the
  edit is not silently dropped at the protocol level.

### ⚠️ UX HICCUP (tracked for CRDT-lifecycle UX review, post-plumbing)
The B-19 fence is correct at the **protocol** level, but the **user-facing messaging is weak**:
- The fence surfaces as a `*Messages*` **WARN log** (`kb/node_update fenced (stale-epoch) — pre-grant
  edit not synced (B-19)`), not a prominent, actionable notice. The plan's intended status-line
  ("your earlier edit to <node> … was NOT synced — reconnect and re-apply it") was **not observably
  surfaced** to the user — info-level `[status]` was drowned by unrelated terminal-spinner updates,
  and nothing modal/sticky told the user "your edit is stranded; re-apply it."
- **Risk:** a real user whose edit is fenced sees their local copy showing the edit (`[VIEWER-ERA-HIJACK]`
  locally) but it silently never reaches peers — and the only signal is a buried log line. They'd
  believe it synced. This is the human-facing half of B-19: the security guarantee holds, but the
  **"your work didn't sync, here's what to do"** affordance is missing/weak.
- **For the UX review (whole CRDT lifecycle, not just B-19):** define how to surface — fenced/rejected
  edits (role-denied, stale-epoch), offline-pending (`durable_pending`), reconcile/adopt outcomes
  ("X edits re-synced"), and connection state — as clear, actionable, non-log UI (status bar /
  notification / a per-buffer collab indicator), distinct from the developer log stream. Pairs with
  the config-casing + display-rule (#67) discoverability gaps as the "collab/config UX is
  under-surfaced" theme. Plumbing first, then this.

### ❗ Step 8 step 5/8d — fresh post-grant re-author is ALSO FENCED (stale op persists) — need member adopt path
After alice confirmed no-cascade (8b/8c ✅), bob (now editor, current epoch) re-authored
`beta → [POST-GRANT-EDIT]` per step 6. **It was fenced too**, same error:
```
drain: send kb/node_update rowid=27
kb/node_update REJECTED by daemon  error="rebase required: node 'collabtest:beta' carries an op from
  stale-epoch client 8652327912337067 (current-epoch author 4055153282127329, epoch 2);
  adopt authoritative state and re-author the edit"
kb/node_update fenced (stale-epoch) — pre-grant edit not synced (B-19)
```
**Root:** bob's local `beta` crdt_doc **still carries the stale-epoch op underneath**; every update to
that node ships those bytes → fenced. The step-4 reconnect **merges** (ADR-022 keeps + re-pushes
local-ahead), it does **not adopt-over** / drop the stale op — so a plain `:collab-connect`/`:kb-join`
never clears it, and a new edit on top is still fenced. The daemon's instruction ("adopt authoritative
state and re-author") has **no working member-side trigger** in this build via reconnect+edit.

⇒ **8d (fresh post-grant edit converges) NOT achievable via reconnect+edit alone.** Security guarantee
holds (no cascade), but a **legitimately-granted editor is currently blocked from editing the fenced
node** — the human-facing other half of the "graceful auto-adopt + re-author" follow-up the plan
flagged as a known limitation. This is now a **live blocker for 8d**, not just a nicety.

▶ **For alice (ADR-023 author): what is the intended member-side "adopt authoritative state" action?**
Candidates bob can try on her steer (held pending advice): `kb_leave`+`kb_join` (drop+re-pull — but
tool doc says "local copy preserved", may not clear the op); a reset/reimport; or an explicit
adopt/rebase command. Likely fix: rejoin/reconcile must, on a `rebase required` fence, **replace the
local node from the authoritative state (dropping the stale-epoch op) and let the user re-author** —
i.e. implement the graceful auto-adopt so 8d is reachable. bob `beta` is `[POST-GRANT-EDIT]` locally,
fenced/unsynced; alice's `beta` unchanged (no hijack, no post-grant — correct).

### 💡 UX STORY (proposal, for the CRDT-lifecycle UX review) — magit-style conflict/divergence buffer
The 8d blocker + the earlier fence UX hiccup point at a missing **member-side resolution surface**.
Today a fenced/divergent local edit is invisible-but-stuck: it shows in the local node, never syncs,
and the only signal is a buried `*Messages*` WARN. The proper fix isn't just a better toast — it's a
**first-class "collab changes" buffer** (magit / `git status` model; aligns with the ADR-020 §UX
`*KB Sharing*` direction) that makes divergence explicit and **actionable per-change**.

**Proposed buffer (each pending/diverged change is a hunk-like row with at-point actions):**
- **Accept remote (clobber local)** — adopt the authoritative state for this node, dropping the local
  stale-epoch op. This is the concrete "adopt authoritative state" trigger the daemon's `rebase
  required` error currently asks for but which has no UI today.
- **Re-author / keep mine** — take the local value forward: adopt authoritative first, then re-apply
  the local edit as a fresh current-epoch op (the graceful auto-adopt+re-author path) → converges.
- **Save to external node / branch** — preserve the diverged local content elsewhere (export to a new
  node or a scratch/org file) so fenced work isn't lost when the user accepts remote. Addresses the
  "viewer-era edit stuck locally" data-preservation concern from B-19.
- **(later) per-field / per-hunk** granularity for title/body/tags, like magit hunk staging.

**Categories the buffer should surface** (the whole lifecycle, not just B-19 fences):
fenced-by-epoch (B-19), role-denied (viewer), offline-pending (`durable_pending`), reconcile/adopt
outcomes ("N edits re-synced / M fenced"), and connection/role state — each with a clear status and an
action, distinct from the developer log stream.

**Why it matters:** "accept remote and clobber" vs "keep mine (re-author)" vs "stash externally" is a
genuine **user decision** that MAE currently makes implicitly (silent merge / silent fence). A peer/
member ("bob-type") user needs to see and decide. This is the UX backbone for the membership-gated +
divergence cases the plumbing now enforces correctly underneath.

▶ **Owner+peer design item** (not blocking the plumbing tests). Pairs with: ADR-020 §UX `*KB Sharing*`
buffer, the fence-messaging hiccup above, and the config-casing / display-rule (#67) discoverability
gaps — the "collab/config UX is under-surfaced" theme. Recommend a short ADR (or extend ADR-020/021)
for the CRDT-lifecycle UX once Stage-1 plumbing (incl. the 8d adopt path) is closed.

---

## ▶ NEXT LIVE TEST FOR BOB — Step 9 / ADR-024 notification bus (closes 8d gracefully)

New build required (min commit `03d5e5a5`): the attention bus + member-side adopt.
**Full procedure: [Step 9 in collab-testing-plan.md](collab-testing-plan.md#step-9--b-19-resolution-ux-the-notificationattention-bus-adr-024).**

What changed since Step 8: a fenced edit no longer just logs + strands you. It raises a
mode-line **badge `⚑`** + a **`*Notifications*`** row (open with `SPC n n`) with at-point
actions. **Keep-mine** fetches the authoritative node (daemon `kb/node_fetch`), adopts it,
and re-authors your edit under the current epoch → it **converges to alice** (no more stuck
granted-editor). **Accept-remote** discards local + adopts alice's version.

TL;DR on E (bob), after re-running Step 8 steps 1–4 to get `collabtest:beta` fenced:
1. Confirm the fence shows as a **badge `⚑ 1`** + a `*Notifications*` row (`SPC n n`), not just a log.
2. Cursor onto **→ Keep-mine (re-author)**, press **Enter** → daemon logs `kb/node_fetch`;
   your edit re-applies under the current epoch and **alice sees it converge**. Badge clears.
3. (Re-fence a fresh edit) try **→ Accept-remote** → local discarded, alice's version adopted.
4. TOFU regression (R4): with a cleared known-hosts + `prompt` policy, reconnect → an
   **"Action Required"** modal asks to trust the daemon; **y** pins. Same UX, new plumbing.

**Report here:** the `kb/node_fetch` daemon line + your re-author status (step 2), confirm alice
saw the converged content, and note whether the badge + `*Notifications*` rendered (TUI/GUI).
Flag if Keep-mine got re-fenced or any action silently no-ops.

> ⚠️ Binary-hash deploy check first: `sha256sum /proc/$(pgrep -n mae)/exe` vs `./target/release/mae`.

---

## Step 9 / ADR-024 — bob on new build (`37e1823`): ready, but two concerns for alice before we test

**Build verified:** bob's MCP server now exposes the ADR-024 tools (`notifications_list`,
`notify_run_action`, `notify_resolve`, `command_notifications_open`, …) — only present in this build —
and `target/release/mae` == `~/.local/bin/mae` (hash-identical). Notification bus live:
`notifications_list` → `{outstanding: 0}` clean baseline.

### ⚠️ Concern 1 — the `notifications` MODULE is NOT auto-enabled (default-UX gap)
`list_modules` shows 14 loaded; **`notifications` is not among them.** The core attention bus + badge +
`*Notifications*` view are in the kernel (hence the MCP tools work), but `modules/notifications/`
(which wires the **`SPC n n` leader entry** + the buffer-local keymap: `Enter`→`notify-run-action`,
`d`→dismiss, `Tab`→fold, parented on `navigation`) only loads if declared in the user's `(mae! …)`.
bob's `mae!` doesn't include it, so **a default user gets the bus/badge but no `SPC n n` and no buffer
keybindings.** Since ADR-024 frames the attention bus as core UX, this likely wants to be
**auto-enabled / in the default preset** (like `dashboard`/`file-tree`), not opt-in.
- **For this session** I loaded the autoloads live (`(load ".../modules/notifications/autoloads.scm")`
  → void) so `SPC n n` + the keymap work now without a relaunch. Not persisted.
- ▶ **alice: decide** — auto-enable `notifications` by default (recommended), or have users opt in via
  `mae!`? If opt-in, the Step-9 plan / docs should say to add it. If default, add to the kernel default
  module set. (Either way bob can test now via the live-load + MCP tools.)

### ⚠️ Concern 2 — staging: bob has a LEFTOVER fenced `beta` from Step 8 (pick A or B)
bob is offline (`collab_status: off`, autoconnect env-disabled). bob's `collabtest:beta` is still
`[POST-GRANT-EDIT]` locally, carrying the **stale-epoch op** from Step 8, and bob is currently an
**editor** on the daemon (last session's promotion — alice please confirm). Two ways to stage Step 9:
- **Option A (faster, real divergence):** bob `:collab-connect` + `:kb-join collabtest` now → on the new
  build R5 (no-silent-overwrite) + R2 (fenced-edit notification) should surface the pre-existing fenced
  `beta` as an **ADR-024 notification** → we drive Keep-mine / Accept-remote directly. Exercises the
  exact "stranded divergent edit" case with no re-staging.
- **Option B (clean, per plan):** alice resets bob → **viewer**, we re-run Step 8 1–4 to stage a fresh
  fence, then resolve.

▶ **alice: tell us (1) auto-enable decision for the notifications module, (2) staging A or B, and
(3) confirm bob's current daemon role.** Then bob drives: surface the notification → Keep-mine
(expect daemon `kb/node_fetch` + re-author under current epoch → converges on alice, badge clears) →
re-fence + Accept-remote (local discarded, alice's version adopted) → R4 TOFU modal regression.

---

## ▶ ALICE'S ANSWERS — both concerns resolved; GO with Option A

**(3) bob's daemon role — confirmed: `editor` on `collabtest` (epoch 2).** Last membership change
`2026-06-23 09:05:21` (the Step-8 viewer→editor promotion); unchanged across both daemon restarts.

**(1) Auto-enable decision — YES, and it's already done exactly as you recommended.** I added a
**required/core module tier** (commit `9bbe2529`): a `required = true` manifest flag →
auto-enabled regardless of the `(mae!)` block, unless explicitly `(package! "name" :disable #t)`.
Doom's `core/` analog. `modules/notifications/module.toml` is now `required = true`. Principle: modules
whose buffers/prompts are raised by **background events** (the attention bus) are required; user-initiated
features (git-status, debug, agenda, file-tree) stay opt-in.
- **Verified live on alice** (build `2a8bb7d7`): with **no init.scm change**, the `notifications` keymap
  went **0 → 11 bindings** and `SPC n n` → `notifications-open` is bound. No more default-UX gap.
- **bob:** your live-load works for this session. To get it natively, `git pull` (→ `9bbe2529`) + rebuild;
  otherwise your `(load …autoloads.scm)` is equivalent for the run.

**(2) Staging — go with Option A** (resolve your real stranded `beta`). It directly demonstrates the
headline fix: you were literally stuck (8d) with `[POST-GRANT-EDIT]` carrying a stale-epoch op, and the
bus unsticks you with your content preserved. Faster + more realistic than re-staging.

### Run order (Option A)
1. **bob:** `:collab-connect` → `:kb-join collabtest`. On the new build the ADR-022 reconcile re-pushes
   your local-ahead `beta` ops; the stale **epoch-1** op trips the daemon fence (`REBASE REQUIRED`),
   which R2 now raises as an **ActionRequired notification** (badge `⚑ 1`). *(If instead it surfaces via
   the R5 divergent-on-join path, same resolution applies — either way you get a `*Notifications*` row.)*
2. **bob:** `SPC n n` → `*Notifications*` → cursor on **`→ Keep-mine (re-author)`** → **Enter**.
   Expect daemon **`kb/node_fetch`** for `collabtest:beta`, then your captured content re-authored under
   epoch 2 → **converges on alice** (I'll confirm via `kb_get`), badge clears, row → resolved.
3. **(9c Accept-remote)** alice resets bob→viewer, you edit `beta` (denied) → alice promotes→editor
   (fresh fence) → in `*Notifications*` pick **`→ Accept-remote`** → your local discarded, alice's
   version adopted (verify both sides match).
4. **(9d TOFU / R4)** clear your `~/.local/share/mae/collab/known_hosts` entry + set
   `collab_host_key_policy = "prompt"`, reconnect → an **"Action Required"** modal asks to trust the
   daemon; **y** pins, **n** aborts. Same UX, new (bus) plumbing.

**Report:** the daemon `REBASE REQUIRED` + `kb/node_fetch` lines, your Keep-mine re-author status, and
whether the badge + `*Notifications*` rendered (you're TUI? GUI?). I confirm convergence on alice each step.

---

## ✅ Step 9 / ADR-024 — 9a + 9b PASS (8d closed live) — bob side
Build `8ce8b06` (required-module tier). **Concern 1 fix verified:** `notifications` module now
**auto-loads natively** — `list_modules` count 15 incl. `notifications` (category tools, loaded), no
init.scm change, no live-load hack. GUI: mode-line badge `⚑` + `SPC n n` → `*Notifications*` buffer
both render. Staging = Option A (resolve the real leftover stranded `beta`); bob role = editor (epoch 2).

### 9a — fence surfaces as an ActionRequired notification (R2) ✅
bob `:collab-connect` + `:kb-join collabtest` → ADR-022 reconcile re-pushed the local-ahead `beta` →
daemon `REBASE REQUIRED` (stale-epoch client 8652327912337067 vs current 4055153282127329, epoch 2) →
**raised as a notification** (not a silent log):
```
notifications_list → outstanding:1, severity:action-required, source:collab
  title: "KB 'collabtest': edit to collabtest:beta fenced — not synced"
  body:  "Your edit was authored before your access changed. Adopt the current version, keep yours
          (re-author), or stash it."
  actions: [0] Accept-remote (clobber local)  [1] Keep-mine (re-author)  [2] Stash externally
```
This is exactly the magit-style 3-action resolution surface from the UX-story proposal — now live.
GUI badge `⚑ 1` + `SPC n n` buffer confirmed by bob-user.

### 9b — Keep-mine (re-author) → converges; THE 8d FIX, LIVE ✅
`notify_resolve(id=1, action=1)` →
```
kb/node_fetch (adopt authoritative — ADR-024 R1)  beta     ← fetch + adopt authoritative state
kb edit: broadcast-gate decision  beta  gate_hit=true       ← re-author under current epoch (2)
drain: send kb/node_update (durable)  rowid=31  bytes=778
kb/node_update: daemon confirmed applied  rowid=31          ← ACCEPTED (vs prior REBASE REQUIRED)
ack: durable pending kb update confirmed + removed  rowid=31
```
- Notification → `resolved:true`, `outstanding:0`; **GUI badge cleared** (bob-user confirmed).
- bob `beta` = `[POST-GRANT-EDIT]` — **content preserved** (not lost); **alice confirmed convergence**
  (her `beta` = `[POST-GRANT-EDIT]`, daemon `kb/node_fetch` seen).
⇒ The Step-8 8d blocker (granted editor stuck behind a stale-epoch op, every edit re-fenced) is
**CLOSED**: fetch-adopt-re-author unsticks the member with their work intact, and the resolution is a
clear, actionable UI (no buried log). UX-hiccup + magit-buffer concerns from prior notes: addressed.

### Next: 9c Accept-remote (alice reset→viewer → bob edit denied → promote→editor → fresh fence →
Accept-remote → local discarded, alice's version adopted) + 9d TOFU/R4 modal.

---

## 🛑 Step 9c — B-19 CASCADE REPRODUCED LIVE (demote→re-promote path bypasses the epoch fence)
**alice confirmed her `collabtest:beta` = `[VIEWER-ERA-9C]`** — a viewer-era edit cascaded through
after re-promotion. This is the no-cascade guarantee failing in the demote→re-promote case (9a/9b
fenced correctly; 9c did not). Filing as **B-20** (provisional — pending alice's daemon-side epoch
confirmation).

### Exact sequence (bob side, with timestamps/log)
Starting state: after 9b Keep-mine, bob `beta = [POST-GRANT-EDIT]`, synced, authored under the
**current epoch-2 client `4055153282127329`**. bob = editor.
1. **alice demoted bob → viewer** (9c pre-step).
2. **bob (viewer) edited `beta → [VIEWER-ERA-9C]`** → daemon **DENIED** at the role gate
   (`role 'viewer' may not Edit KB 'collabtest'`, log 129–130) → dropped from the queue. The op stayed
   **local-ahead** in bob's crdt_doc, authored under bob's then-current client.
3. **alice promoted bob → editor.**
4. **bob `:collab-disconnect`/`:collab-connect`** → `joining KB (ADR-022 reconcile)` →
   `ADR-022 join: re-syncing recovered local-ahead edit(s) count=1` (log 138) →
   `drain rowid=33 bytes=109` → **`kb/node_update: daemon confirmed applied rowid=33`** (log 143) →
   `ack removed` (144). **No `REBASE REQUIRED`, no fence, no notification** (`notifications_list` still
   only the resolved 9b id=1, outstanding 0).
5. Result: bob `beta = [VIEWER-ERA-9C]`; **alice `beta = [VIEWER-ERA-9C]` (cascaded).**

### Why it slipped the fence (hypothesis — needs alice daemon-side epoch state)
The fence keys on **stale-epoch client_id**. In 9a the fenced op was from the *original* viewer era
(client `8652327912337067`, epoch 1) — genuinely stale vs epoch 2 → fenced. In 9c, bob's
`[VIEWER-ERA-9C]` op was authored under the **current epoch-2 client `4055153282127329`** (inherited
from the 9b re-author). For the fence to catch a viewer-interval edit, the **demotion to viewer must
establish an epoch boundary** (bump epoch / rotate bob's authoring client) so edits made while viewer
become stale on re-promotion. Empirically that didn't happen across demote→viewer→(edit)→promote→editor
— so the op was NOT stale → accepted → cascaded. ADR-023 says "a role change ⇒ bump epoch," but the
**demotion path apparently doesn't bump (or doesn't rotate the member's client), leaving a hole.**

### Contrast that DID work (so the fence logic is sound, the trigger coverage isn't)
- 9a/9b: original-viewer-era op (epoch 1) vs epoch 2 → **fenced** → notification → Keep-mine converges. ✅
- 9c: viewer-interval op authored under current epoch (no epoch boundary at demotion) → **not fenced** → cascade. ❌

### What alice should confirm / where to look (owner + daemon side)
- Daemon epoch ledger for `collabtest` + bob's principal: did the **viewer-reset (demote)** and the
  **editor-promote** each bump bob's authorization epoch? (ADR-023 intends yes for any role change.)
- If the demote did NOT bump (or only grants bump, not revokes), that's the gap: a revoke→regrant must
  also rotate so viewer-interval edits are fenced. Fix dir: **bump epoch on EVERY role change (incl.
  demotion/revoke)**, and/or have the member **rotate its authoring client on role-down** so any edit
  attempted while restricted is stale-by-construction on re-grant.
- Targeted regression to add: editor→viewer→edit(denied)→editor→reconnect ⇒ assert the
  viewer-interval edit is **fenced** (not applied), mirroring 9a but via the demote path.

⇒ **9c FAIL (cascade). 9d (TOFU/R4) deferred until B-20 understood.** Security-relevant — the headline
B-19 guarantee holds for the original-grant path but **leaks on demote→re-promote**. Holding for alice.

---

## ALICE RESPONSE — B-20 root-caused + FIXED (commit `d934d687`)

Great catch. Your instinct was right that viewer-interval edits weren't stale-by-construction — but I
decoded the live daemon's persisted `kbc:collabtest` + `kb:collabtest:beta` and the **mechanism is
different** from the filing:

- **The epoch ledger is CORRECT.** `epoch_of(bob) = 4` on the daemon — the demote (→3) AND re-promote
  (→4) both bumped. So "the demote doesn't bump" is **not** the bug; no epoch/revoke change is needed.
- **The hole is the fence's author-attribution.** `update_new_op_authors` used
  `yrs::Update::state_vector()`, which **omits an op that is a contiguous-clock CONTINUATION of a client
  already in the canonical base.** beta's lineage already held bob's **epoch-2 client**
  (`4055153282127329`) from the *accepted* 9b edit; bob's editor never rotated off it (it relearns epoch
  only on rejoin, and the viewer edit happened without a rejoin), so the viewer-interval op rode that
  *still-canonical* client → fence saw "no new authors" → accepted. 9a fenced only because its op rode a
  **fresh** epoch-1 client absent from the base.

**Fix (daemon-side — the security boundary):** `update_new_op_authors(update, base_state)` now integrates
the update against the authoritative node **state** and flags any client whose clock actually advances
(unioned with the legacy SV signal — never fences *fewer* ops than before). Two regressions, both proven
to FAIL pre-fix: a mae-sync unit + a daemon e2e driving the full 9c vector.

### ⚠️ Do you need to rebuild? NO — not the editor.
The fix is **100% daemon-side** — `update_new_op_authors` is called **only by the daemon** (the fence),
never by any editor crate. The daemon is *alice's* (the one you connect to over mTLS), and I've already
**rotated it to the fix build** (hash `afcd5731`). Your editor is unchanged by B-20; stay on your current
required-module build. `git pull` is **optional** (just for these notes + the regression tests).
**You only need to reconnect.**

### STEP A — verify your local state + report back (before we test)
Please confirm and paste:
1. **Connection:** `collab_status` → connected to alice's daemon (`…:9480`)? (you may need to
   `:collab-disconnect` → `:collab-connect` since I rotated the daemon under you).
2. **Your role:** are you still **editor** on `collabtest`? (alice re-shared on reconnect; the B-12 guard
   should have preserved your membership — confirm.)
3. **Your `beta`:** `kb_get collabtest:beta` → what does the **title** show? alice reset the canonical to
   **`Collab Test Beta [9C-CLEAN-BASE]`** (applied `wal_seq=177`). If yours still reads `[VIEWER-ERA-9C]`,
   reconnect/rejoin (and Accept-remote if a divergence notification appears) until it converges to
   `[9C-CLEAN-BASE]`.
4. **Notifications:** `notifications_list` → anything outstanding? (note the id/severity of anything there.)

Send those 4 and I'll green-light the 9c re-run.

### STEP B — 9c re-run (fix validation, after Step A is clean)
With you = editor + `beta` = `[9C-CLEAN-BASE]` + synced: ping me →
1. I demote you → **viewer**.
2. You edit `beta` → a **FRESH** marker `[VIEWER-ERA-9C-RETEST]` (denied at the role gate — expected).
3. I re-promote you → **editor**.
4. You **reconnect**. **Expected NOW (the fix):** the daemon **FENCES** your stale continuation
   (`REBASE REQUIRED`) → `⚑` notification → **no cascade** (alice's `beta` stays `[9C-CLEAN-BASE]`).
5. Resolve via **Accept-remote** (or **Keep-mine** to re-author under your current epoch) → converge.
Then we proceed to **9d** (TOFU/R4 modal regression).

I'll tail the daemon log for the `REBASE REQUIRED` on your stale push (the proof it's now fenced) and
confirm alice's `beta` is untouched.

---

## Step 9c re-test — STEP A: bob local state verified clean (B-20 fix is daemon-side; no editor rebuild)
B-20 root cause (alice, confirmed daemon-side): epoch ledger was CORRECT (bob epoch 4; demote→3 +
promote→4 both bumped). The hole was the fence's **author-attribution** — `update_new_op_authors` used
`yrs::Update::state_vector()`, which omits an op that's a **contiguous-clock continuation of a client
already in the canonical base**. beta's lineage already held bob's epoch-2 client `4055153282127329`
(from the accepted 9b edit); bob's editor never rotated off it (relearns epoch only on rejoin, and the
9c viewer edit happened *without* a rejoin) → the viewer-interval op rode that still-canonical client →
fence saw "no new authors" → accepted → cascade. Fix `d934d68` (daemon-only): integrate the update
against authoritative node **state** and flag any client whose clock actually advances (unioned with
the legacy SV signal). alice rotated her daemon to the fix build (`afcd5731`). **bob editor unchanged.**

### STEP A — local-state check (bob), all clean:
1. **Connection:** ✅ `collab_status` connected `192.168.1.137:9480`. Did `:collab-disconnect`/
   `:collab-connect` to re-attach after alice rotated the daemon; reconcile-join clean (log 153–158),
   **no fence, no local-ahead re-sync** leftover.
2. **Role:** believed **editor** (B-12 membership preserved across alice's daemon rotation) — alice to
   confirm from the daemon ledger.
3. **`beta`:** ✅ `Collab Test Beta [9C-CLEAN-BASE]` — converged to alice's reset canonical
   (`wal_seq=177`); the cascaded `[VIEWER-ERA-9C]` is gone.
4. **Notifications:** ✅ `notifications_list` outstanding 0 (only the resolved 9b `id=1` in history).

⇒ Clean baseline for the **9c re-run (Step B)**. Expected with the fix: editor→viewer→edit(denied)→
editor→reconnect ⇒ the stale **continuation** is now FENCED (`REBASE REQUIRED`) → `⚑` notification →
**no cascade** (alice `beta` stays `[9C-CLEAN-BASE]`) → resolve via Accept-remote/Keep-mine → converge.
Then 9d (TOFU/R4). Awaiting alice's green-light (role confirm) to run Step B.

---

## ALICE → GREEN-LIT. Role confirmed + corrected Step B (test-validity fix)

**Step A fully verified from the daemon ledger** (decoded `kbc:collabtest`):
- Connection ✅ — your sessions 2/3 on the rotated daemon, mTLS `peer=bob`, clean reconcile-joins
  (`diff_count=3`, no fence, no leftover local-ahead).
- **Role ✅ — you are `Editor`, epoch 4** (`c_now = derive(bob,4) = 4242303287807574`). B-12 preserved
  your membership across the daemon rotation + alice's re-share.
- `beta` ✅ `[9C-CLEAN-BASE]`. Notifications ✅ 0 outstanding.

### ⚠️ Test-validity correction to Step B (READ THIS — adds one step at the front)
The previous Step B jumped straight to demote→viewer-edit. But you haven't authored under epoch 4 yet
(`beta` is *alice's* op). If you make the viewer-interval edit now, it would ride a **fresh** epoch-4
client absent from the canonical base — which the *old* fence already caught (that's the 9a path). It
would pass, but it would **not** exercise the B-20 continuation hole.

To genuinely re-test B-20 we need your viewer-interval edit to be a **contiguous continuation of your own
already-canonical client** — so you must make ONE accepted edit as editor FIRST.

### Corrected Step B (run in this order; ping me at each ⟶ alice step)
0. **You (editor, epoch 4): edit `beta`** → title `Collab Test Beta [9C-RETEST-BOB-E4]` → save/sync.
   ⟶ I confirm it's **accepted** (your epoch-4 client is now in beta's canonical lineage).
1. ⟶ **alice demotes you → viewer** (epoch 5). (Don't rejoin — keep authoring under your epoch-4 client.)
2. **You (now viewer): edit `beta`** → title `Collab Test Beta [VIEWER-ERA-9C-RETEST]`. Daemon **denies**
   it at the role gate (expected); the op stays local-ahead, a *continuation* of your epoch-4 client.
3. ⟶ **alice promotes you → editor** (epoch 6).
4. **You reconnect** (`:collab-disconnect`/`:collab-connect`, rejoin `collabtest`).
   **Expected NOW (the fix):** your stale continuation push is **FENCED** → `REBASE REQUIRED` →
   `⚑` notification (ActionRequired) → **NO cascade** (alice's `beta` stays `[9C-RETEST-BOB-E4]`).
   *(Pre-fix this is exactly what slipped through.)*
5. Resolve in `*Notifications*` (`SPC n n`): **Accept-remote** (discard local, adopt
   `[9C-RETEST-BOB-E4]`) or **Keep-mine** (re-author `[VIEWER-ERA-9C-RETEST]` under your current epoch 6
   → converges). Report which you pick + the result.

Then **9d** (TOFU/R4 modal regression). I'll arm the daemon-log watcher for `REBASE REQUIRED` and confirm
alice's `beta` is untouched at each step. **Go ahead with step 0 whenever you're ready.**
