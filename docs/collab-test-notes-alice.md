# Collab Test Notes — alice (D, framework / Linux)

Running log from the **machine-D ("alice" + daemon host)** side of the two-machine
ADR-017 collab validation (`feat/crdt-collab-validation`). **Update + commit as we go**
so E (bob) sees findings. Pairs with [collab-test-notes-bob.md](collab-test-notes-bob.md)
and [collab-testing-plan.md](collab-testing-plan.md).

## Logging convention (shared with bob)

Each entry is tagged with **where in the plan** it happened: **Step** (e.g. `T2.5`),
**Action**, **Expected** vs **Actual**, **Status** (✅ pass · ❌ fail · ⚠️ unexpected ·
🔧 worked-around · ⏳ pending), **Repro**.

## Environment (this machine)

- **D = `framework`, `192.168.1.137`** — runs **both** the `mae-daemon` hub **and** editor **"alice"**.
- Daemon: `0.0.0.0:9480`, `key` + mTLS, dedicated identity dir `~/.local/share/mae/daemon-collab`.
  - **Daemon host fingerprint:** `SHA256:07aWfiNGm690ZcPzxEWvCSTYgkIz+Dw7Db0RPOKK7Ls`
  - Authorized: `alice` (`SHA256:wTMjuc7…UoCQ`), `bob` (`SHA256:9xLh0…2CrI`)
- Alice editor: GUI dev build (`--features gui`), `init.scm` → key mode, server `127.0.0.1:9480`,
  auto-connect, **`collab_host_key_policy = accept-new`** (#66 workaround).
- **Test data:** `/tmp/mae-collab-run/collab-demo.txt` = `collab demo — line from alice (D)\n`
  (34 chars; contains **em-dash `—` U+2014** — 1 char / 1 UTF-16 unit / **3 UTF-8 bytes**).

## Run 1 — 2026-06-16

| # | Step | Action | Expected | Actual | Status |
|---|------|--------|----------|--------|--------|
| 1 | T0 | local `make test-collab-{mtls,membership}-e2e` (release) | green | mtls 7/7, membership 7/7 | ✅ |
| 2 | T2.2 | daemon `0.0.0.0:9480` key+mTLS | listening, authed | `authorized=2`, fingerprint `07aW…7Ls` | ✅ |
| 3 | T2.3 | authorize alice + bob | 2 keys | both listed, distinct fps | ✅ |
| 4 | T2.4 | bob (mac) connects over LAN | mTLS auth `peer=bob` | authenticated from `192.168.1.132` | ✅ **cross-machine auth works** |
| 5 | T2.4 | alice (local) connects | mTLS auth `peer=alice` | authed session=2; pinned key == `07aW…7Ls` (out-of-band ✅) | ✅ |
| 6 | T2.5 | alice opens `collab-demo.txt` + `collab-share` | daemon accepts | `sync/share accepted`, `synced_docs=1` | ✅ |
| 7 | T2.5 | bob `collab-join` | bob sees alice's line | bob received `collab demo — line from alice (D)` | ✅ (bob row 8) **alice→bob receive** |
| 8 | T2.5 | (during convergence) clicked split panes to focus collab buffer | focus switches | **alice GUI panicked & crashed** (double-click word-select past EOL) | ✅ **fixed** [I-1](#i-1) |
| 9 | T2.5 | headless convergence (daemon + 2 `--test` editors, bob edits) | alice receives | content 36→60, **no crash** — isolates I-1 to the mouse path | ✅ |
| 10 | T2.5 | post-fix live run: bob joins `collab-demo2.txt`, both edit | converge both ways | bob's line + alice's seed + alice's typed line all merged on alice; 52 session-7 + 1 session-8 updates | ✅ **converges** |
| 11 | T2.5 | I-1 fix live check: double-click @ col 138 in split | no crash | alice survived (was the exact crash gesture) | ✅ |

### I-2 (bob) — RESOLVED: not a bug, a wrong-active-buffer MCP artifact · Step T2.5
- alice→bob send appeared broken when driven via MCP `eval_scheme (buffer-insert …)`: **0**
  session-7 updates, and the inserted line never appeared in the collab buffer.
- **Confirmed cause (bob pinned it):** when driving via MCP the active buffer was **`*AI:claude*`**
  (the agent shell), so `buffer-insert` targeted the **wrong buffer** — not the shared doc; a
  same-burst `switch-to-buffer` didn't take before the insert. (My earlier "skips the post-edit
  flush" guess was wrong — the edit simply went elsewhere.)
- **Proof:** typing the same line via **real keystrokes** in the GUI produced **52** session-7
  updates and propagated to bob. ⇒ Real edits sync.
- **Status:** ✅ not a collab bug. *Testing-harness caveat:* when driving collab edits over MCP,
  `switch-to-buffer` to the shared doc as its **own step**, verify with `list_buffers`, then edit
  — or use real input / the `--test` runner.

## Issues

### I-1 ✅ RESOLVED — alice panic: mouse double-click word-select past line end · Step T2.5 {#i-1}

Bob filed the matching **I-1**. The remote-update theory was a **red herring** — the real
trigger was a **mouse click**, not the CRDT sync (headless convergence never crashed).

- **Actual trigger (user-confirmed):** clicking the **left/right window splits** a few times
  to focus the shared-collab pane. Two clicks at the same spot register as a **double-click →
  word-select**, and a click in the right pane of a vertical split has a large **screen
  column (~138)** that far overruns the short collab line.
- **Backtrace (`/tmp/alice-bt.log`, RUST_BACKTRACE=full):**
  ```
  ropey::rope::Rope::char                              (rope.rs:803 — index 138 into 34-char rope)
  mae_core::word::word_start_backward
  mae_core::editor::mouse_ops::handle_mouse_click_inner   (double-click word-select)
  <mae::GuiApp as ApplicationHandler>::window_event
  ```
- **Root cause:** the double-click path computed `char_offset_at(target_row, text_col)` with
  an **unclamped `text_col`** (the single-click path clamps; the double-click path didn't),
  and `word_start_backward` guarded `pos == 0` but **not** `pos > len_chars()` (unlike
  `word_end_forward`, which already guards `pos >= max_pos`). So a click past EOL produced
  `offset = line_start + 138` → `rope.char(137)` → panic. `char_offset_at` clamps `row` but
  not `col`, which let the overrun through.
- **Fix (`6c048bc7`+):**
  1. `crates/core/src/word.rs` — `word_start_backward` clamps `pos.min(len_chars())` (defense
     in depth; symmetric with `word_end_forward`).
  2. `crates/core/src/editor/mouse_ops.rs` — double-click path clamps `text_col` to the
     clicked line's length before `char_offset_at` (matches single-click; also protects the
     link-follow branch).
- **Tests:** `word_motions_clamp_out_of_bounds_pos` + `word_start_backward_out_of_bounds_on_empty_rope`
  (word.rs); `mouse_double_click_past_line_end_does_not_panic` (mouse_tests.rs). All green;
  full mae-core suite 2237/2237.
- **Note:** the unclamped **cross-window column** (fallback `handle_mouse_click` gets raw
  screen coords, not window-relative, when `pixel_to_buffer_position` returns `None`) is a
  separate latent correctness issue — clamping makes it safe (selects the last word) but a
  follow-up should make the fallback window-relative. Logged as **I-3** below.
- **Status:** ✅ FIXED — needs both machines on the rebuilt binary to re-verify T2.5.

### I-3 ⚠️ follow-up — split-window click uses raw (not window-relative) coords · Step T2.5
- When `pixel_to_buffer_position` returns `None`, `main.rs` falls back to
  `handle_mouse_click(row, col)` with **raw screen** row/col; in a split the column isn't
  offset by the pane's x-origin, so clicks in a right pane map to the wrong column (now
  clamped, so no crash, but cursor lands at the line end rather than the clicked glyph).
- **Fix idea:** subtract the focused window's `area_col`/`area_row` before dispatch, or always
  resolve via the focused window's fresh layout. Low severity (cosmetic) post-I-1 fix.

### Cross-refs to bob's issues
- **I-2** (bob) ⚠️ bob's local edit to joined buffer not visible on read-back — re-test early
  next run with a stable link; may be coupled to I-1's rope path.
- **I-7** (bob) ⚠️ connection flapping (`peer closed without TLS close_notify`) — correlated
  with **alice crashing/restarting** (each crash drops bob's link). Likely a **symptom of I-1**,
  not independent; re-evaluate once alice is stable.
- **#66** (filed) — interactive `prompt` TOFU deadlocks; both editors on `accept-new`.

## Convergence scorecard (D view)

| Direction | Step | Result |
|-----------|------|--------|
| alice → bob (receive) | T2.5 | ✅ (Run 2) |
| bob → alice (send) | T2.5 | ✅ (Run 2, post-I-1 fix) |
| alice → bob (send, real keys) | T2.5 | ✅ (Run 2) |
| simultaneous | T2.5 | ✅ (bob confirmed Run 2) |

## T2.6 — shared-KB membership (in progress)

- **New committed fixture: `tests/fixtures/kb/collabtest/`** — a 3-node throwaway KB
  (`overview`/`alpha`/`beta`, sentinels `ZEPHYRINE`/`QUOKKA`/`NARWHAL`) so we never
  replicate personal `RoamNotes` to a peer. Follows the `assets/manual` org format
  (`:ID: collabtest:*`). Validated via MCP: `kb-register collabtest <dir>` → 3 nodes,
  `kb_search "ZEPHYRINE"` → `collabtest:overview`.
- **Wired into `scripts/collab-membership-e2e.sh`:** alice now ingests the fixture
  before sharing, so membership runs against real content. **e2e green** (alice 8/8,
  bob 7/7, `PASS`: deny → add → allow).
- **Caveat:** the `mae --test` runtime doesn't register the KB query layer, so the
  fixture can't be asserted via a scheme test (the whole `tests/kb-lifecycle` suite is
  orphaned for the same reason). Validation is the membership e2e + MCP `kb_search`.
- **Live two-machine T2.6:** ready — share `collabtest` by name (see I-4) and run
  deny → add → allow → remove across D/E.

### I-4 ✅ FIXED — `kb-share` could not target a specific KB (shares first instance) · Step T2.6
- **Gap:** `kb-share` shared `registry.instances.first()` (`kb_state.rs:99`) with no way to
  pick the KB. On a machine with personal notes + a project KB (alice: RoamNotes is first),
  bare `:kb-share` would replicate **RoamNotes** to peers — a real data-leak risk and the
  blocker for a clean live T2.6 against the fixture.
- **Fix:** `:kb-share <name>` now queues `ShareKb { kb_name: <name> }` for that instance
  (`command.rs`, mirroring `:collab-join <name>`); the intent processor already resolves the
  name (`collab_bridge.rs:418`, errors if unknown). Bare `:kb-share`/`SPC C S` unchanged
  (active/first instance). Docs updated; 2 regression tests in `command_tests.rs`.
- **Status:** ✅ fixed (shipped `b111b9e6`). Implementing it surfaced two deeper bugs (I-5, I-6).

### I-5 ✅ FIXED — named-instance KB share resolved `instances` by name (keyed by UUID) · Step T2.6
- **Found via:** live `:kb-share collabtest` returned "KB 'collabtest' not found" even though
  `collabtest` was registered + queryable via `kb_search`.
- **Root cause:** `editor.kb.instances` is keyed by **UUID** (`kb_ops.rs:236`), but the ShareKb
  resolver did `instances.get(&kb_name)` with the **name** (`collab_bridge.rs:421`) → never
  matched. (The membership e2e only worked because it shared `"default"` → the *primary* path.)
- **Fix:** resolve name→uuid via `registry.find()` before the `instances` lookup
  (`collab_bridge.rs`, with a uuid-passthrough fallback).

### I-6 ✅ FIXED — `:kb-join`/`:kb-leave <id>` ignored the arg (joined the active KB) · Step T2.6
- **Found via:** the e2e — bob's `:kb-join collabtest` hit `kb_id=default` (denied), not
  `collabtest`. Same bug family as I-4: the dispatch used `active_instance_name()` and the
  ex-command never parsed the arg (the handler's own comment claimed command.rs did — it didn't).
- **Fix:** `command.rs` now parses `:kb-join <id>` / `:kb-leave <id>` (mirroring `:collab-join`
  and the I-4 kb-share arm). 2 regression tests.
- **Also fixed a FALSE PASS in the membership e2e:** the verdict counted any non-denied
  `kb/join` line, but the daemon logs the *request* (`"kb/join"`) before the membership check,
  so a denied join still matched. Re-keyed the verdict on `"kb/join: complete"` for `collabtest`
  (the daemon's acceptance line, `collab_handler.rs:1357`). e2e now genuinely exercises
  register → share-by-name → deny → add → allow, **green** (alice 8/8, bob 7/7) with bob's
  join correctly targeting `collabtest`.

## Next

1. Rebuild/relaunch both on the I-4/I-5/I-6 fixes; live T2.6 via `:kb-share collabtest`
   / `:kb-join collabtest` (deny → add → allow → remove), then T2.7 (security checks).
2. Log each step here with the shared convention.
