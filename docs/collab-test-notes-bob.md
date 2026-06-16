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

### I-1 ❌ HIGH — alice rope panic crash on remote update  ·  Step T2.5  ·  task #18
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

## Convergence scorecard

| Direction | Step | Result |
|-----------|------|--------|
| alice → bob (receive) | T2.5 | ✅ confirmed (row 8) |
| bob → alice (send) | T2.5 | ❌ blocked by I-1 (alice crash) + I-2 (edit not visible) |
| simultaneous edit | T2.5 | ⏳ not reached |

## Next run (from scratch)

1. D captures rope panic backtrace (I-1) → fix in `crates/core` → push.
2. Both `git pull --rebase` → rebuild both binaries.
3. Restart daemon (key, `0.0.0.0:9480`, authorize bob) + alice (accept-new) + bob.
4. Re-run **T2.4 → T2.7**; re-test **I-2 early** with a stable link.
5. Log every step's outcome here with the convention above.
