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

### B-1 ⚠️ UX — pending join reported as "Joined (0 nodes)"  ·  Step T2.6
- Under `invite` policy, bob's `kb-join` is recorded **pending** server-side, but the editor
  status says **"Joined KB 'collabtest' (0 nodes)"** — a real user can't tell *pending
  approval* from *joined-but-empty*. Should say e.g. "Join requested — pending owner approval".
- Daemon-side oracle (alice's `:kb-pending`) is correct; this is editor-side wording only.

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

## Convergence + membership scorecard

| Capability | Step | Result |
|-----------|------|--------|
| alice → bob (receive) | T2.5 | ✅ Run 1 + Run 2 |
| bob → alice (send) | T2.5 | ✅ Run 2 (no crash) |
| simultaneous edit | T2.5 | ✅ Run 2 (replicas identical) |
| KB membership: invite→pending→approve→allowed | T2.6 | ✅ Run 3 (by fingerprint, mTLS) |
| KB replication to approved peer | T2.6 | ✅ Run 3 (ZEPHYRINE sentinel) |
| editor-role write allowed | T2.6 | ✅ Run 3 (kb_update) |
| editor edit propagates to owner | T2.6 | ⏳ alice confirming |
| viewer-role write rejected | T2.6 | ⏳ pending demotion |
| restrictive policy denies 3rd peer | T2.6 | ⏳ not reached |
| security checks | T2.7 | ⏳ not reached |

## Next run (from scratch)

1. D captures rope panic backtrace (I-1) → fix in `crates/core` → push.
2. Both `git pull --rebase` → rebuild both binaries.
3. Restart daemon (key, `0.0.0.0:9480`, authorize bob) + alice (accept-new) + bob.
4. Re-run **T2.4 → T2.7**; re-test **I-2 early** with a stable link.
5. Log every step's outcome here with the convention above.
