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
| alice → bob (receive) | T2.5 | ✅ (bob confirmed) |
| bob → alice (send) | T2.5 | ❌ blocked by I-1 (alice crash) |
| simultaneous | T2.5 | ⏳ not reached |

## Next run (from scratch, after I-1 fix)

1. **D: capture I-1 backtrace → fix in `crates/core` → push.** ← in progress
2. Both `git pull --rebase` → rebuild both binaries (GUI).
3. Restart daemon (key, `0.0.0.0:9480`, authorize bob) + alice (`accept-new`) + bob.
4. Re-run T2.4 → T2.7; re-test bob's I-2 early with a stable link.
5. Log every step here with the shared convention.
