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

### ADR-018 ✅ IMPLEMENTED — identity-anchored KB access control (the structural fix)

I-7 (creator-mismatch reject) was the symptom of a deeper gap: KB ownership/membership
keyed on a **mutable, non-unique label** + self-claimed `collab-user-name`. Rebuilt the
whole model — see [ADR-018](adr/018-identity-anchored-kb-access-control.md) +
[COLLABORATION.md](COLLABORATION.md). Shipped across `feat/crdt-collab-validation`
(commits `863d854`→`585f799`): identity plumbing → v2 CRDT schema → `kb_access` engine →
CLI → editor commands → migration → docs/e2e + smuggling gate. Grounded in NIST RBAC +
Zanzibar/ReBAC + OWASP. **All layers tested green** (mae-mcp 124, mae-sync 155, daemon
144, editor dispatch) and the **membership e2e passes the full flow over real mTLS**.

- **Identity = key fingerprint** (`SHA256:…`); label/`collab-user-name` are display-only.
- **Roles** `owner ⊇ editor ⊇ viewer`; **join policy** `restrictive|invite|permissive`
  (default `invite`). Members managed **by fingerprint**.
- **No more `collab-user-name=alice` workaround** — `:kb-share` binds owner from the cert.

## Run 3 — 2026-06-16 — live T2.6 under ADR-018 (two machines, key mode, real mTLS)

Both machines rebuilt daemon + editor. Daemon D pid 3337008, `0.0.0.0:9480`, fp
`SHA256:07aW…7Ls`, authorized=2. alice (owner, loopback) session 4; bob (mac
`192.168.1.132`) session 5, fp `SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI`.

| # | Step | Expected | Result |
|---|---|---|---|
| 1 | alice `:kb-share collabtest` | owner = alice's fp (no `collab-user-name` workaround) | ✅ `kb/share: complete … node_count=3`; owner derived from cert (session 4 = alice's principal) |
| 2 | bob `:kb-join collabtest` | PENDING (invite policy), 0 nodes | ✅ `kb/join: pending … principal=Some("SHA256:9xLh0DWee…")`, bob got 0 nodes |
| 3 | alice `:kb-pending` + `:kb-approve … editor` | recorded by fingerprint | ✅ `kb/list_pending` (session 4) then `kb/approve_member: complete … role="editor"` |
| 4 | bob `:kb-join collabtest` again | ALLOWED, 3 nodes | ✅ `kb/join: complete … node_count=3` — bob received replication |
| 5 | alice `:kb-member-add … viewer` | role demoted | ✅ `kb membership change … add=true role="viewer"` |
| 6 | bob edits a node → should be **rejected** | `kb/node_update denied` | ⚠️ **BLOCKED by I-8/I-9** — no `kb/node_update` ever reaches the daemon (write propagation broken), so the gate can't be exercised live |
| 7 | alice `:kb-policy collabtest restrictive` + `:kb-member-remove bob` | bob non-member under restrictive | ✅ `kb/set_policy: complete policy="restrictive"` + `kb membership change … add=false` |
| 8 | bob `:kb-join collabtest` (non-member, restrictive) | **DENIED** (deny-by-default, no pending) | ✅ `kb/join denied … reason=not a member of KB 'collabtest'` (WARN). bob's UI showed "0 nodes" — his **B-1** UX bug, daemon correctly denied |

**T2.6 access-control PASSES live over mTLS:** invite → pending → approve-by-fingerprint
→ join (steps 1–4), role demote (5), **and restrictive deny-by-default (7–8)**. Identity
anchored to the key fingerprint throughout. The two things NOT demonstrable live are both
*content-editing* gaps, not access-control failures: step 6 (viewer-edit-denial) is blocked
because **KB edits don't propagate at all** (I-9), of which I-8 is one face. The daemon's
viewer-Edit gate is real + unit-tested (daemon 144 green); it simply has no live traffic to
act on yet.

### I-9 🚨 OPEN (critical) — shared-KB content edits do not propagate between peers · Step T2.6 {#i-9}

The headline gap. **No `kb/node_update` has *ever* reached the daemon** in the entire live
run (grep on the daemon log is empty), on either machine, despite a successful local
`kb_update`. So ADR-018 access control works, but the collaborative KB *editing* it gates is
non-functional end-to-end.

- **Mirror-image resolution asymmetry (register vs. join):** the KB lives in a different store
  on each machine, and the read/write/search/instances paths each consult a different store:
  | path | alice (registered instance) | bob (joined via collab) |
  |---|---|---|
  | `kb_get <id>` (node_json) | ✅ resolves (iterates `instances`) | ❌ "No KB node" (bob's **B-3**) |
  | `kb_update <id>` (kb_update_node) | ❌ "No KB node" (primary-only, **I-8**) | ✅ resolves + writes locally |
  | `kb_search` | ✅ | ✅ |
  | `kb_instances` lists it | ✅ | ❌ not tracked (bob's **B-3**) |
- **Even when the write succeeds (bob), nothing broadcasts.** `kb_update_node`'s CRDT-broadcast
  block (`kb_ops.rs:492-518`) only fires when `collab.kb_sync_mode == "on_save"` **and** the id
  is in `collab.shared_kbs`. A joined KB isn't registered in `shared_kbs`, so the branch is
  skipped → no `kb/node_update` RPC → owner never sees the edit (bob's row 7 = ❌).
- **Consolidated fix scope (the I-8 follow-up the user signed off on, now bigger):**
  1. Unify KB node resolution across `kb_get`/`kb_update`/`kb_delete`/`kb_search`/`kb_instances`
     so register-as-instance and join-via-collab present the **same** node namespace + store.
  2. On `kb-join`, track the joined KB in `collab.shared_kbs` (and surface it in `kb_instances`)
     so edits to its nodes flow through the CRDT-broadcast path.
  3. Make `kb_update_node`/`kb_delete_node` federation-aware (resolve across primary ∪ instances ∪
     joined; apply CRDT upsert to the owning store; emit `kb/node_update`).
  4. Regression + e2e: edit a node in a joined KB → `kb/node_update` reaches the daemon →
     converges to the owner; then the viewer-denial e2e becomes drivable (closes T2.6 step 6).
- **Status:** OPEN, **critical** — this is the actual collaborative-KB-editing feature; access
  control is the scaffolding around it. Supersedes the narrow I-8 framing (I-8 kept as a
  sub-symptom). Cross-ref bob's **B-3** (same family, owner/join side).
  **✅ FIXED (`697b9015`)** — see "Fixes landed" below.

### I-8 ⚠️ OPEN — KB write path (`kb_update`/`kb_delete`) is primary-only, not federation-aware · Step T2.6 {#i-8}

- **Symptom (bob, reproduced on alice):** `kb_get collabtest:overview` resolves, but
  `kb_update collabtest:overview` → **`No KB node: collabtest:overview`**. Read sees the
  node, write doesn't.
- **Root cause:** `Editor::kb_update_node` (`crates/core/src/editor/kb_ops.rs:469-473`) and
  `kb_delete_node` (`:445`) resolve the id **only against `self.kb.primary`**. Joined/registered
  KBs (collabtest) live in `self.kb.instances`, which the write path never consults.
  `kb_get`/`node_json` (`crates/ai/src/tool_impls/kb.rs:46-66`) *does* iterate instances —
  hence the read/write asymmetry. The CRDT-broadcast block (`kb_ops.rs:492-518`) is downstream
  of the failed resolution, so it never runs → **no `kb/node_update` is ever emitted** for a
  shared-KB node.
- **Impact:** No one — not even the owner — can edit a shared/joined KB node via `kb_update`.
  This blocks the **viewer-edit-denial e2e** (the edit dies client-side before the daemon's
  Edit gate runs). The daemon-side gate is real + unit-tested (daemon 144 green incl. the
  viewer-edit-denied case); it just can't be demonstrated through `kb_update` until I-8 is fixed.
- **Fix direction:** make `kb_update_node`/`kb_delete_node` federation-aware — resolve the id
  across `primary` ∪ `instances`, apply the CRDT upsert to the owning store, and ensure the
  shared-KB broadcast path keys off the resolved instance (not just `self.collab.shared_kbs`
  membership against primary). Add a regression test: edit a node in a *registered instance*
  → succeeds + emits a node_update. Verify both owner-edit (allowed) and the e2e viewer-denial
  once writes flow.
- **Status:** ✅ FIXED (`697b9015`, folded into I-9). Captured during live T2.6.

### I-10 🚨 OPEN (security) — daemon auth is a once-at-startup snapshot; authorize/revoke need a restart · Step T2.7 {#i-10}

- **Symptom (demonstrated live):** ran `mae-daemon revoke <bob-fp>` → bob removed from on-disk
  `authorized_keys` (only alice left), **yet bob's mTLS session stayed established and unblocked**
  — no auth-layer rejection in the daemon log. Revoking a key (even a *compromised* one) does
  nothing until the daemon restarts.
- **Root cause:** `daemon/src/main.rs:390` calls `AuthorizedKeys::load(&ak_path)` **once at
  startup**, wraps it in an `Arc`, and bakes it into the rustls `ServerConfig` client-cert
  verifier (`mae_mcp::tls::server_config`). There is **no reload/watch** — the running server
  never re-reads the file. The `mae-daemon revoke`/`authorize` CLIs mutate the file from a
  separate process the live daemon never consults.
- **Impact:** can't add or remove collaborators on a running daemon; **revocation is not
  enforceable live** — a serious gap for a multi-user service (OWASP: revocation must be timely).
  Also blocks T2.7 (revoked-key-denied-on-reconnect) without a restart workaround.
- **Fix direction:** make the authorized set live. The cert verifier should consult a shared,
  swappable source (`Arc<ArcSwap<AuthorizedKeys>>` or `Arc<RwLock<…>>`) rather than a baked-in
  copy; reload on file change (reuse the existing `notify` infra) or re-read per handshake
  (connections are infrequent). `authorize`/`revoke` then take effect immediately. Add an
  integration test: connect → revoke → **reconnect denied** with no restart.
- **Status:** ✅ FIXED (`27929083`) — live reload per handshake; see "Fixes landed" below.
  User flagged the once-at-startup model as unacceptable; fixed before resuming the plan.

> **Test-state note:** bob is now de-authorized **on disk** (re-add before resuming:
> `mae-daemon authorize mae-ed25519 aBjMkdzHH9YVUxfP5NxHJo7fcu5qGC75pUl1SWdAvnM= bob`).
> The live daemon still trusts him until it restarts — itself the I-10 repro.

## Pivot — bug-fix pass before resuming KB tests (decided 2026-06-16)

Live T2.6 validated ADR-018 access control but surfaced three defects that must be fixed
before re-running the KB collab plan from the start:

1. **I-10 (security)** — live auth reload (no restart for authorize/revoke). *Cleanest; first.*
2. **I-9 (critical)** — shared-KB edit propagation + unified node resolution (folds in I-8).
   The core collaborative-editing feature; biggest change.
3. **B-1 (UX, bob)** — editor can't distinguish joined / pending / denied (all show "0 nodes").

Each lands with positive + negative tests (TDD), `make ci-all` green, both-OS aware. After
the fixes both machines rebuild and we restart the KB test plan clean (re-authorize bob,
re-share collabtest, re-run T2.6 incl. the now-unblocked viewer-edit-denial + T2.7 revoke).

### ✅ Fixes landed 2026-06-16 (all tested, on `feat/crdt-collab-validation`)

- **I-10 (`27929083`)** — `ClientAuthSource` consulted per handshake;
  `ReloadingAuthorizedKeys` re-reads `authorized_keys` from disk each connection
  (fail-secure on missing); daemon uses `server_config_reloading`.
  `authorize`/`revoke` now take effect with **no restart**. Test:
  `mtls_reloading_verifier_honors_live_revoke` (one config, authorized→connect,
  revoke-on-disk→reconnect-rejected).
- **I-9 (`697b9015`, folds I-8)** — federation-aware writes:
  `kb_update_node`/`kb_delete_node` resolve across `primary ∪ instances`
  (`kb_owner_of`) and mutate the owning KB/store; `node_json` (kb_get) falls
  through on a query-layer miss (joined nodes live in `primary`); the `KbShared`
  handler resolves name→uuid so `shared_kbs` is populated (was empty → no
  broadcast). Tests: instance-node update resolves + queues a CRDT update;
  instance delete resolves; named-instance share tracks by uuid.
- **B-1 (`43f6c5a5`)** — kb/join surfaces **joined / pending / denied** as three
  distinct outcomes (was "Joined (0 nodes)" for all). Tests: pending→StatusReport,
  denied→Error, success→KbJoined.

Regression: editor workspace (mae-core 2247, mae-ai 450, mae 269, mae-mcp 125) +
daemon (85/36/14/9) all green, 0 failures. Clippy clean. **Both machines must
rebuild daemon + editor** to pick these up before resuming.

## ✅ ADR-019 landed 2026-06-17 — durable, reconstruction-capable shared-KB sync

The "edits don't propagate" investigation root-caused a structural flaw: the
broadcast gate (`shared_kbs`) was a transient, event-only set — never durable,
never reconstructed. Even an in-session share left it empty for the owner. Fixed
as a full architectural pass (7 phases, ADR-019), all on `feat/crdt-collab-validation`:

- **P0 `23b73f15`** — traceability: `MAE_LOG=kb_sync=debug` greps an edit end-to-end;
  `introspect` now shows shared_kbs / kb_sync_mode / pending counts / owning-instance
  markers + a `gate_present` divergence flag (diagnose live via MCP, no rebuild).
- **P1 `23b73f15`** — **durable emit gate**: share stamps `KbInstance.shared/collab_id`
  (+ new registry `primary_shared/collab_id`), persisted; `kb_update_node` gates on the
  DURABLE marker (`kb_collab_id_of`), not the cache → edits emit across restart.
- **P2 `35aafc20`** — joined KBs are **first-class instances** (addressable, in
  `kb_instances`, durable markers) instead of dumped into `primary` (fixes bob's B-3);
  guest edits now emit.
- **P4 `35aafc20`** — receive routes to the **owning instance** (`kb_apply_remote_update`),
  not always primary.
- **P3 `e6a4c458`** — reconstruct gate from durable markers at startup + on `Connected`;
  re-subscribe (re-join) every durable KB via a `reconnect_intents` queue → survives
  reconnect/restart.
- **P5 `cf673b7c`** — B-5 tolerant KB row-load (no main-thread stall); **B-6 XDG-first KB
  path** (also correctness: marker save+load paths must match); I-10 live label
  resolution; MCP `kb_share` honors `kb_id`; **Collab Status buffer auto-refresh** on
  KbShared/KbJoined/KbLeft (bob's stale-after-join report).
- **P6 `fb5c4559`** — [ADR-019](adr/019-durable-reconstruction-capable-kb-sync.md) +
  restart-survival e2e (durable marker survives registry save→load; restarted editor with
  empty cache still emits).

**Regression:** editor (mae-core 2251, mae-ai 451, mae 270, mae-mcp 125) + daemon all
green, 0 failures; clippy clean. **Both machines rebuild editor + daemon** (daemon changed
for I-10 label). Daemon fingerprint `07aW…7Ls` unchanged (no re-TOFU). After rebuild, the
T2.6 flow below should propagate edits **both ways** + survive restart.

> **Note (this machine):** during the bug hunt I moved alice's editor collab dir aside, so
> alice regenerated her key (`SHA256:+jBinAwoF…`, re-authorized live). bob's key unchanged.

## (Superseded) Next — live T2.6 under ADR-018 (BOTH machines rebuild daemon + editor)

> ⚠️ Both `mae` and `mae-daemon` changed. On each machine: `git pull` →
> `make build && make install` (GUI) + `make build-daemon && make install-daemon`,
> then restart the daemon and relaunch the editor. D's daemon fingerprint
> `SHA256:07aW…7Ls` is unchanged (no re-TOFU). Default join policy is **invite**.

1. **alice (owner):** `:kb-share collabtest` → owner bound to alice's key (no workaround).
2. **bob:** `:kb-join collabtest` → **PENDING** (invite policy), not denied.
3. **alice:** `:kb-pending collabtest` (lists bob's label + fingerprint) →
   `:kb-approve collabtest <bob-fingerprint> editor`.
4. **bob:** `:kb-join collabtest` → **ALLOWED** (member); edits a node → propagates.
5. **Role check:** alice `:kb-member-add collabtest <bob-fp> viewer` → bob's next node
   edit is **rejected** (read-only). `:kb-policy collabtest restrictive` → a 3rd peer's
   join is denied.
6. T2.7 security: unauthorized peer rejected; `mae-daemon revoke <fp>` → denied on
   reconnect; `tcpdump` still shows TLS.
7. Log each step here with the shared convention.

## B-8 (bob) — shared-KB edit does not enqueue/emit · Run 5/6 (ADR-019)

Confirmed live on BOTH sides (alice owner-instance + bob guest-instance): a
`kb_update` on a `collabtest` node changes the node **locally** but
`pending_kb_updates` stays **0** and **zero `kb/node_update`** ever reaches the
daemon (grep on the whole daemon log = 0). So nothing propagates either direction.

**Localization so far (alice side):**
- Phase-0 introspect: `owning_instances[collabtest]` = `shared:true, gate_present:true`,
  `kb_sync_mode:on_save` — all gate INPUTS present on the live editor.
- The full live chain is correct by inspection: MCP tool runs on the **live** editor
  (`ai_event_handler::handle_mcp_request` → `execute_tool` → `kb_update_node`, no
  snapshot/clone), and `drain_collab_intents` runs every `about_to_wait` (~70 fps).
- **Local repro PASSES** (`b8_repro_registered_kb_edit_enqueues`): a real `kb_register`
  of the fixture + durable marker + `kb_update_node` → `pending_kb_updates == 1`,
  node in the instance (not primary), uuid matches registry. So the **gate logic is
  correct**; B-8 is live-state-specific, not a logic bug.
- **Next:** relaunch alice with `MAE_LOG=kb_sync=debug,collab=debug` to capture the
  `kb edit: broadcast-gate decision` trace (owner + gate_hit) — the binary already has
  the trace (Phase 0/1); no rebuild needed.

---

## 2026-06-17 ~15:30 — ⭐ STAGE 1 (ADR-020 Phases 0–3) LANDED + PUSHED — bob pickup here

**Branch `feat/crdt-collab-validation` is at `1f4a6993`.** Stage 1 of ADR-020 (the
holistic shared-KB durability + emit-pipeline redesign) is committed and pushed:

| commit | phase |
|---|---|
| `b93498d1` | Phase 0 — ADR-020 doc (`docs/adr/020-replicated-kb-crdt-artifact.md`) + observability seam |
| `0865b4d8` | Phase 1 — emit-pipeline hardening: never silently lose a `kb/node_update` (durable requeue) + daemon liveness (`track_client_connect` so live docs aren't idle-evicted) |
| `4d72ed41` | Phase 2 — merge-on-join (CRDT `apply_update`) instead of insert/overwrite (preserves offline edits) |
| `1f4a6993` | Phase 3 — durable joined instance + **disk-first** startup loader (loads from `db_path` even when `org_dir=""`) + registry rescan to recover shared KBs missing from a clobbered registry. Fixes **B-10**. |

Full design + the four decisions + the deferred backlog (D1–D6) are in
**`docs/adr/020-replicated-kb-crdt-artifact.md`**; the staged plan is
`.claude/plans/crystalline-forging-pond.md`.

### → BOB: how to pick up (do this first)
```sh
git fetch && git checkout feat/crdt-collab-validation && git pull   # → 1f4a6993
make build      # ⚠️ GUI build (FEATURES defaults to gui). Do NOT use `cargo build -p mae`
                #    — that's TUI-only and will drop your GUI (alice hit exactly this).
# install over ~/.local/bin via temp+mv to avoid "Text file busy", e.g.:
cp target/release/mae ~/.local/bin/mae.new && mv -f ~/.local/bin/mae.new ~/.local/bin/mae
# bob is editor-only (no daemon on bob — it connects to alice's daemon at 192.168.1.137:9480).
# Relaunch bob's editor with tracing so we can localize emit:
MAE_LOG=info,kb_sync=debug,collab=debug ~/.local/bin/mae   # (or your usual GUI launch + this env)
```
alice's **daemon** is already on the Stage-1 build (running, `0.0.0.0:9480`,
fingerprint `SHA256:07aW…7Ls` unchanged → no re-TOFU; 2 authorized keys incl. the
**new** alice key `SHA256:+jBinAwoF…`). alice's **editor** is on the Stage-1 GUI build.

### Live finding so far (alice side, this session — bob was offline so only the editor→daemon half ran)
Daemon traced to `/tmp/mae-daemon-live.log` (`MAE_LOG=info,kb_sync=debug,collab=debug`):
- ✅ **`kb/share` reaches the daemon** — explicit `kb_share collabtest` →
  `kb/share: complete session=5 kb_id=collabtest node_count=3 owner=alice (+jBinAwoF…)`.
- ❌ **`kb/node_update` STILL does not reach the daemon (B-8 not yet closed at emit).**
  Two `kb_update collabtest:overview` (title → `[STAGE1-ALICE-A1]`, then `[A2]`) changed
  the node **locally** but produced **0** new daemon log lines — the local daemon
  (definitely live) received nothing. So this is NOT a bob-offline artifact; the editor
  is not emitting the update at all. NB: collabtest now loads with a **real dir** under the
  disk-first loader, yet emit is still 0 — so the dir-less instance (B-10) was a *separate*
  restart-survival bug, not the emit cause. Phase 1 makes emit durable *once enqueued*; the
  remaining gap is that the update is apparently **never enqueued** from this path.
- `collab_status` shows `synced_docs:0` even after a successful share — likely counts
  text buffers, not KB docs, so treat it as uninformative for KB sync (confirm).

**Open question for the next session (the real Stage-1 blocker):** does the **MCP
`kb_update` tool path** (`kb_update_node`) fire the broadcast gate, or does it write the node
to the store **bypassing** the path that enqueues the collab intent? alice's passing local
repro (`b8_repro_registered_kb_edit_enqueues`) gets `pending_kb_updates == 1` via
`kb_update_node` — but live it stays 0. Reconcile that divergence: capture the editor-side
`kb edit: broadcast-gate decision` trace (owner + gate_hit) + `pending_kb_updates` right after
a live `kb_update`. If the gate never fires live, that's the fix target.

### Remaining Stage-1 LIVE GATE (once bob is live + rebuilt)
1. **Bidirectional propagation** — alice `kb_update` a node → lands on bob (daemon logs
   `kb/node_update: received`, doc stays `connected_clients≥1`, no idle-evict); bob edits →
   lands on alice.
2. **Restart survival (B-10)** — restart alice's editor → joined nodes reload from disk
   (disk-first loader) + edits still flow.
3. **Offline-merge (Phase 2)** — edit while disconnected → merges, not overwritten, on rejoin.

Only when all three are green do we proceed to **Stage 2** (Phases 4–7: `replicated|hosted`
mode + status taxonomy, the `*Collab Status*` launch fix B-11, the magit-style `*KB Sharing*`
management buffer, flagship e2e) and the **deferred D1–D6** backlog — all still in flight,
tracked in ADR-020 §Future Work.

---

## 2026-06-17 ~16:30 — ⭐ B-8 ROOT CAUSE FOUND + FIXED (`95295a2b`) — bob: rebuild + re-run Step 1

**The edit emit was a wire-protocol bug, not durability.** alice-side tracing
(`/tmp/alice-kbsync.log`) proved the editor did everything right —
`gate_hit:true → drain: send → bg: kb/node_update written to wire (×2)` — yet the
daemon logged **0** `kb/node_update: received`. Root cause:

> `kb/node_update` was hand-rolled in the editor bg-task **as a JSON-RPC notification
> (no `id`)**. The daemon's read loop routes no-`id` messages to the *notification*
> handler, which only relays `sync/awareness` and **drops everything else** — so it
> never reached the apply+broadcast request handler. Text `sync/update` carries an
> `id` and works. (Also: the durable row was acked on channel-send, before the wire;
> and `kb_update_node` enqueued to BOTH SQLite and an in-mem Vec → double-send — hence
> "written to wire ×2".)

**Why no test caught it (the meta-bug):** the one KB e2e was `#[ignore]`d AND used a
hand-rolled client that sent the *correct* id-bearing shape — it tested a parallel
implementation, not the shipping path.

**Fix (`d1e04cee`, pushed on `95295a2b`):**
- `shared/sync/src/wire.rs` — ONE shared builder for the collab JSON-RPC messages, used
  by the editor emit path **and** the daemon e2e. `kb/node_update`/`kb/share`/`kb/join`
  are requests (carry `id`). Unit test asserts every request builder has an `id`.
- `collab_bridge.rs` — `kb/node_update` is now a request; durable row acked only on the
  daemon's `{applied:true}` (queue→send→confirm→ack); in-flight rowid set (no re-send
  storms; cleared on disconnect); error responses surface loudly.
- `kb_ops.rs` — single-source enqueue (kills the double-send).
- `daemon/collab_handler.rs` — a request-only doc method arriving as a notification is
  now a **loud `warn!`**, never a silent drop again.
- `daemon/tests/collab_e2e.rs::kb_node_update_applies_and_broadcasts_to_peer` — real
  wire round-trip: share→join→edit→`{applied:true}`→peer receives broadcast. **Proven
  to FAIL (hang) when the builder omits the `id`, pass with it.** All suites green;
  clippy clean both workspaces.

### → BOB: to validate the fix
```sh
git fetch && git pull         # → 95295a2b (or later)
make build                    # GUI editor (NOT cargo build -p mae)
cp target/release/mae ~/.local/bin/mae.new && mv -f ~/.local/bin/mae.new ~/.local/bin/mae
# restart bob's editor with tracing (MAE_LOG=info,kb_sync=debug,collab=debug)
```
alice will rebuild + restart her **daemon** (carries the loud-warn + is the apply/
broadcast hub) and her **editor** (carries the request-emit). Then re-run **Step 1**:
alice edits `collabtest:overview` title → expect bob sees it, the daemon logs
`kb/node_update: received` + `kb/node_update: applied wal_seq=…`, and bob's log shows an
inbound `sync_update` for `kb:collabtest:overview`. Then the reverse (bob→alice) and
restart-survival.

---

## 2026-06-17 ~17:05 — ⭐ B-13 FIXED (`4602ce4b`) — receive path: members now live-subscribe to node docs

Step-1 re-run proved **B-8 emit is fixed** (alice's edit reached the daemon →
`received`/`applied wal_seq=48,51,52` across three edits) but bob still didn't see it.
bob localised it: the daemon broadcasts and bob **receives**, but bob **drops it locally**
(`ignoring sync_update for unsubscribed doc doc=kb:collabtest:overview`). Confirmed
member-side only (bob: "daemon delivery confirmed").

**Root cause (B-13):** the editor gates inbound `sync_update` by `shared_docs.contains(buffer_name)`
(`collab_bridge.rs:2832`). Text buffers add their doc to `shared_docs` on share/join — but the
**KbShare/KbJoin paths never did**, so every inbound `kb:<node>` update was discarded. Emit worked,
receive was dead.

**Fix (`f7e9e6d1`, pushed on `4602ce4b`):** mirror the text-buffer subscription —
- **ShareKb** (owner): subscribe to `kbc:<kb>` + each `kb:<node>` → owner receives peer edits (bob→alice).
- **KbJoin response** (member): subscribe to `kbc:<kb>` + each joined `kb:<node>` → member receives
  live edits after the join snapshot (alice→bob).
- Inbound `kb:<node>` already routes to `KbNodeUpdate → kb_apply_remote_update` (by node-id prefix) →
  `mark_full_redraw`.
- Test `handle_response_kb_join_subscribes_to_collection_and_node_docs` guards it. 271 bin tests green.

### → BOB: rebuild for the B-13 fix (editor-only; daemon unchanged)
```sh
git fetch && git pull          # → 4602ce4b (or later)
make build                     # GUI editor (NOT cargo build -p mae)
cp target/release/mae ~/.local/bin/mae.new && mv -f ~/.local/bin/mae.new ~/.local/bin/mae
# restart bob's editor (MAE_LOG=info,kb_sync=debug,collab=debug → /tmp/bob-collab.log)
```
alice's editor is already on the B-13 build (`4602ce4b`); daemon unchanged (B-13 is editor-only).

**⚠️ B-12 still open:** alice's editor restart re-shares `collabtest` and **clobbers bob's
membership** (owner re-share is destructive on the daemon, not a merge). So on reconnect bob lands
**pending** again — alice re-approves (`:kb-approve collabtest <bob-fp> editor`), bob re-joins, then
we test. Tracked as B-12 (fix: daemon `kb/share` must CRDT-merge onto an existing collection/node,
not delete+replace).

### Expected with B-13 fixed
alice edits `collabtest:overview` → bob's editor **applies** the inbound `kb:collabtest:overview`
update (no more "unsubscribed doc" drop) and the title updates **on bob's screen**. Then bob edits →
alice receives (owner now subscribed to its own node docs). That closes bidirectional Stage-1.
