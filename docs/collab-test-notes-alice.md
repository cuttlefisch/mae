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
| 8 | T2.5 | bob edits line w/ `—`, propagates to alice | alice shows bob's edit | **alice GUI panicked & crashed** | ❌ [I-1](#i-1) |

## Issues

### I-1 ❌ HIGH — alice rope panic on remote update (`Rope::char` OOB) · Step T2.5 {#i-1}

Same issue bob filed as **I-1** (his notes) — D owns the backtrace + fix.

- **Symptom (alice GUI stderr):**
  ```
  thread 'main' panicked at ropey-1.6.1/src/rope.rs:803:13:
  Attempt to index past end of Rope: char index 138, Rope char length 34
  ```
- **Panicking call:** `Rope::char(idx)` (ropey `rope.rs:799`) — an **editing/cursor** op
  (word-motion / multicursor / search / buffer char lookup), **not** pure render. So a
  `.char()` caller received a stale offset **138** (≫ alice's 34-char rope) when bob's
  remote edit arrived.
- **Trigger (from bob's repro):** bob joins, **edits a line containing `—` (U+2014)**, the
  update propagates to alice → alice panics. Strong **char vs UTF-16 vs byte** offset-mismatch smell.
- **Scoped / ruled out:**
  - `mae_sync::TextSync::apply_update` is safe — full `rebuild_rope()` from the yrs string,
    no per-index rope ops (`shared/sync/src/text.rs:265`).
  - Local cursor adjustment after a remote update **is** clamped
    (`crates/mae/src/collab_bridge.rs:834`, `adjusted.min(new_len.saturating_sub(1))`).
  - GUI remote-cursor/selection render uses **pixel math** + `line_len(row)` clamps
    (`crates/gui/src/cursor.rs:391,488`) — no direct rope char-index.
  - ⇒ Suspect a remaining **editor-side apply-remote / sync-update-hook / redraw** path that
    feeds an unclamped char offset into `rope.char()` (`crates/core`).
- **Pinpoint plan (D):** reproduce locally **headless** (daemon + two `--test` editors, bob
  joins same `/tmp/...collab-demo.txt` doc-id + edits the `—` line) under
  `RUST_BACKTRACE=full` → exact frame → clamp + regression test in `crates/core`/`mae-sync`.
- **Status:** OPEN — D capturing backtrace now.

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
