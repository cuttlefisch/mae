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

---

## 2026-06-17 ~17:40 — ⭐ B-14 + B-15 FIXED (`490d9a3c`) — KB edits finally MERGE across peers

B-13 made bob **receive + run the apply path**, but applies came back `changed=false` and bob's
content never updated. bob diagnosed the next layer (B-14); the realistic test surfaced a *second*
bug (B-15) in the same pipeline. **This is the class** you flagged — multiple defects on the same
path, hidden because every prior merge test used a SHARED lineage (one doc → encode → apply to a doc
from those same bytes), never two independently-built peers.

- **`changed=false` is NOT hardcoded** — it's `hash_before != hash_after` around the real yrs apply.
  An honest signal; the rot was upstream.
- **B-14 (divergent lineage):** yrs merges on lineage (client_id + op history), not the node-id
  string. alice and bob each built `collabtest:<node>` independently (both imported the org fixture)
  → incompatible lineages → their title/body YText are different yrs objects at the same map key →
  `apply_remote_update` no-ops (map last-writer-wins discards the owner's text). **Fix:**
  `KnowledgeBase::adopt_remote_node` rebuilds the node from the owner's encoded state so both share
  ONE lineage; `kb_register_joined_instance` now ADOPTS on join (mirrors the text-buffer
  `from_state_with_client_id` model) instead of merging same-id siblings.
- **B-15 (edits after the first never entered the CRDT):** `upsert_with_crdt`, when the node already
  had a `crdt_doc`, rebuilt from the OLD bytes and **ignored the new title/body fields**. So alice's
  RECV-2/3/4 re-broadcast stale content (byte-identical `update_len=1121` each — visible in the
  daemon log!). **Fix:** apply the edited fields onto the existing lineage via `set_title`/`set_body`.
- **Test (the methodology fix):** `divergent_lineage_merge_noops_but_adopt_converges` (shared/kb) —
  alice edits chained on her lineage, bob built the same id independently; a plain merge no-ops
  (B-14 marker), adoption converges, and the owner's NEXT chained edit (B-15) merges as a real change.
  mae-kb 223 green.

### → BOB: rebuild for B-14+B-15 (editor-only; daemon unchanged)
```sh
git fetch && git pull          # → 490d9a3c (or later)
make build                     # GUI editor
cp target/release/mae ~/.local/bin/mae.new && mv -f ~/.local/bin/mae.new ~/.local/bin/mae
# restart bob's editor (MAE_LOG=info,kb_sync=debug,collab=debug → /tmp/bob-collab.log)
```
alice is already on the B-14+B-15 build (`490d9a3c`). **Both editors need it** (B-15 = emit chains,
B-14 = receive adopts). daemon unchanged.

**⚠️ Important — fresh divergence:** existing `collabtest:<node>` docs on alice and bob still carry
their OLD divergent lineages from before this fix. The adopt path only re-establishes shared lineage
**on join**. So after both rebuild: alice re-approves bob (B-12), **bob re-joins → bob ADOPTS alice's
current node lineage** (you'll see bob's titles snap to alice's current values), and from then on
alice's chained edits should propagate live. If a node is still stuck, the cleanest reset is bob
leave+rejoin so the adopt runs again.

### Expected with B-14+B-15
After bob's (re)join adopts alice's lineage: alice edits `collabtest:alpha` → bob's alpha title
updates **on screen** (`changed=true` in bob's log). Then bob→alice reverse. That closes bidirectional
Stage-1 (modulo B-12 membership-durability, still open).

---

## 2026-06-22 ~15:48 — ✅ STEP 1 confirmed (bob) + ⭐ B-16 FIXED (`1652fcf4`) — bob→alice owner-side

**Step 1 (alice→bob) is GREEN** (bob confirmed: adopt-on-join snapped his titles to alice's lineage;
live edit `changed=true`). **Step 2 (bob→alice) failed** — bob's emit + the daemon were green
(`kb/node_update applied wal_seq=69`), alice RECEIVED it but `changed=false` (owner-side no-op).

**Root cause (B-16) — owner lineage divergence + the audit finding:**
- `KnowledgeBase::to_collection` (the share payload builder) calls `node.to_crdt_doc()`, which for a
  **never-edited** node (`crdt_doc=None`, e.g. `beta`) mints an **ephemeral, random lineage each call**
  and — being `&self` — never persists it. So the daemon + bob (on join) adopted lineage A while
  alice's LOCAL `beta` kept no durable lineage; bob's edit (on A) no-opped against alice's freshly
  minted lineage B. (Same `changed=false` failure mode as B-14, but on the **owner**.)
- Audit (per your "find other hardcoded params") confirmed `client_id = 1` in `kb_update_node` is the
  ONLY hardcoded collaborative-write param (kb:/kbc:, OffsetKind::Utf16, channel caps, NodeKind::Note
  fallback are genuine constants). It's a **latent** concurrent-edit collision (two peers
  indistinguishable to yrs), not the live sequential blocker — proven by a production-fidelity test.

**Fix (`1652fcf4`):**
- `Editor::kb_prepare_share_lineage` — establishes + **persists** a canonical `crdt_doc` for every
  shared node (incl. unedited) with write-through, BEFORE encoding the payload → owner's local doc IS
  the lineage peers adopt. Called from the ShareKb path.
- Stable, **unique** per-peer `client_id` derived from the durable collab identity fingerprint
  (`derive_kb_client_id`), set once at startup — replaces the hardcoded `1`.

**Test methodology (your meta-point):** the bugs hid because tests hand-picked DISTINCT client_ids
(alice=1/bob=2) while production hardcodes 1 for both — *a test using different standins than the code
can't catch a hardcoded-value bug*. New production-fidelity tests + the full write-up are in
**`docs/collab-kb-sync-testing-lessons.md`**.

### → BOB: rebuild for B-16 (editor-only; daemon unchanged)
```sh
git fetch && git pull          # → decf6ba2 (or later)
make build                     # GUI editor
cp target/release/mae ~/.local/bin/mae.new && mv -f ~/.local/bin/mae.new ~/.local/bin/mae
# restart bob's editor (MAE_LOG=info,kb_sync=debug,collab=debug → /tmp/bob-collab.log)
```
alice is on the B-16 build. **Both editors need it** (the owner establishes the persisted lineage on
share; the client_id is per-peer). Note: existing `collabtest` nodes carry pre-fix lineages —
alice's restart re-shares with **canonical persisted** lineages this time, so bob's re-join adopts
those and the reverse direction should finally converge.

### Expected with B-16
alice restart → re-share (canonical persisted lineage) → re-approve bob (B-12) → bob re-join (adopt) →
**bob edits `collabtest:beta` → alice's beta updates on screen (`changed=true` in alice's log)**. That
closes **bidirectional** Stage-1 (modulo B-12 membership-durability + the next-step two-independent-
peers e2e).

---

## 2026-06-22 ~16:32 — ✅ BIDIRECTIONAL GREEN + B-12 fixed — current state & remaining manual CRDT test matrix

### What is DONE (live-validated on two machines)
- **The entire KB-sync bug chain is fixed**: B-8 (emit) → B-13 (subscribe) → B-14 (join-adopt lineage)
  → B-15 (chained-edit) → B-16a (owner persisted lineage) → B-16b (per-peer client_id) → **B-12**
  (owner re-share preserves membership). All have production-fidelity unit tests.
- **Bidirectional live propagation CONFIRMED** with content convergence:
  - alice→bob: `collabtest:alpha` → `[B14-CONVERGE-1]` landed on bob (`changed=true`).
  - bob→alice: `collabtest:beta` → `[BOB-LIVE-2]` landed on **alice** (`changed=true`).
- Write-up: `docs/collab-kb-sync-testing-lessons.md` (bug chain + why tests missed them + e2e contract).
  Compliance/RBAC direction: `docs/adr/021-membership-policy-compliance.md`.

### Current deployed builds (branch `ca08e52a`)
- **daemon**: B-12 build (pid up; eager-recovers all docs + membership from WAL on restart). `0.0.0.0:9480`.
- **editors**: B-16 build on both. alice client_id derived from identity fingerprint; bob distinct.
- Identity fingerprints unchanged → **no re-TOFU**. bob = `SHA256:9xLh0DWee…2CrI` (approved editor).

### → BOB: pull + confirm you're current
```sh
git fetch && git pull          # → ca08e52a (or later)
make build                     # GUI editor (you already have B-16; pull in case of newer)
cp target/release/mae ~/.local/bin/mae.new && mv -f ~/.local/bin/mae.new ~/.local/bin/mae
# restart editor with tracing: MAE_LOG=info,kb_sync=debug,collab=debug → /tmp/bob-collab.log
```
**B-12 is now deployed daemon-side** — so after alice restarts you should **stay an approved member
(no `pending`, no re-approve)**. If you DO land pending, that's a B-12 regression to flag.

### REMAINING manual test matrix (to finish validating the full CRDT feature)
Each: drive via MCP `kb_update` / editor; watch daemon log + the peer's `kb_get`/screen + `changed` in logs.

| # | Test | Steps | Expected |
|---|------|-------|----------|
| **T1** | **B-12 owner-restart (now)** | alice restart → re-share fires | bob **stays approved** (daemon log: `collection exists — preserving membership`); **no re-approve**; bidirectional still works |
| **T2** | **Restart survival (B-10)** | restart bob's editor → `kb_instances` | joined `collabtest` reloads **3 nodes** from disk (dir=""); after re-join, edits still flow both ways |
| **T3** | **Offline-merge** | bob disconnects (`:collab-disconnect`) → edits a node offline → reconnects | bob's offline edit **converges** on alice (and vice-versa) — no loss, `changed=true` on apply |
| **T4** | **Concurrent same-node** | alice & bob edit the SAME node (e.g. `collabtest:alpha` title) within a second of each other | both converge to **one identical value** on both screens (per-peer client_id, B-16b) — no divergence |
| **T5** | **Body + multi-field** | edit the **body** (not just title) of a node; verify propagation both ways | body change converges; title unaffected |
| **T6** | **Daemon-restart survival** | restart the **daemon** (alice side) → editors reconnect | docs + **membership recover from WAL**; sync resumes; bob stays approved |
| **T7** | **Roles/policy (ADR-018)** | alice `:kb-member-add collabtest <bob-fp> viewer` → bob edits a node | bob's edit **rejected** (read-only); restore editor role → edit allowed again |

T1 is happening now (alice restarting). Then work down T2–T7. Log each result here with the shared
convention so we have a complete record for the write-up.

### Results log
- **T1 — B-12 owner-restart: ✅ PASS** (alice + bob confirmed). alice restart → daemon logged
  `kb/share: collection exists — preserving daemon-side membership (B-12)`; bob auto-rejoin →
  `kb/join: complete` (NO pending, NO re-approve). Bidirectional re-verified post-restart.
- **T2 — restart-survival (bob editor restart): ✅ PASS** (cross-validated alice daemon log ⇄ bob startup log).
  - bob startup: `KB instance loaded from CozoDB nodes=3 shared=true` (disk-first reload, B-10, despite
    `dir=""`) → auto `joining KB` → `KB join complete (merged) node_count=3` (no pending, B-12 holds on
    bob restart). Titles survived the restart (kb_get matched the pre-restart baseline).
  - **bob→alice post-restart:** `beta → [BOB-T2-POSTRESTART]` (rowid=6) → daemon `received → applied
    wal_seq=85`; alice `recv: applied … changed=true`, `kb_get` shows the slug.
  - **alice→bob post-restart:** `alpha → [ALICE-T2-POSTRESTART]` → daemon `received → applied
    wal_seq=86` → broadcast to bob (bob confirmed). Receive-after-restart works both ways.
  - NB (bob): the restart's disk-reload overlaps the auto-rejoin/adopt, so T2 validates durability +
    rejoin together; the **pure offline-durability** case is isolated in **T3** (edit while disconnected).

---

## T3 — offline-merge: READY-TO-RUN procedure (alice ⇄ bob)

**Goal / what it proves:** edits made **while a peer is disconnected** converge on reconnect, in
**both directions, with nothing lost** — the local-first contract. Exercises the **durable pending
queue** (ADR-020 Phase 1: bob's offline edit persists to the SQLite queue and flushes on reconnect)
and the **reconnect reconciliation** (alice's edits during the gap reach bob via his rejoin snapshot).

**Daemon baseline for this run:** line **251** (alice tails `/tmp/mae-daemon-live.log`).
**Probe slugs:** bob offline → `collabtest:beta` = `[BOB-OFFLINE-1]`; alice during-gap → a DIFFERENT
node `collabtest:overview` = `[ALICE-WHILE-BOB-OFFLINE]` (different nodes so neither masks the other).

### Steps (ordered — do not interleave)
0. **Pre-check (both):** connected + synced; record current titles. (alice will `kb_get` overview+beta;
   bob `kb_get` the same. `collab_status` = connected on both.)
1. **bob:** `:collab-disconnect`. → bob log `collab disconnected`; alice daemon log: bob's session ends
   (no more requests from his session id). bob `collab_status` → disconnected. **Tell alice "offline".**
2. **bob (while offline):** edit `collabtest:beta` title → `[BOB-OFFLINE-1]`.
   - Expected: bob's LOCAL node updates immediately; **no `kb/node_update` for beta appears in the
     daemon log** (he's offline). The edit is held in bob's durable queue
     (`introspect.collaboration` → `pending_kb_updates ≥ 1` or the SQLite pending row). **Tell alice "edited offline".**
3. **alice (while bob offline):** edit `collabtest:overview` title → `[ALICE-WHILE-BOB-OFFLINE]`.
   - Expected: alice is still connected → daemon `kb/node_update received → applied wal_seq=N` for
     `overview`; it is **held for bob** (he's not subscribed while offline — no delivery yet).
4. **bob:** `:collab-connect` (or auto-reconnect) → auto re-join (adopt) + **durable queue flushes**.
5. **Convergence — PASS criteria (verify all four):**
   - **(a) bob→alice flush:** daemon logs `kb/node_update received … node_id=collabtest:beta` →
     `applied wal_seq=…`; **alice** `recv: applied … changed=true`, `kb_get beta` shows `[BOB-OFFLINE-1]`.
   - **(b) alice→bob catch-up:** **bob** `kb_get overview` shows `[ALICE-WHILE-BOB-OFFLINE]` (delivered
     via the rejoin snapshot or broadcast).
   - **(c) no loss / no revert:** beta still `[BOB-OFFLINE-1]` on both; overview `[ALICE-WHILE-BOB-OFFLINE]`
     on both; no node reverts to a pre-gap value.
   - **(d) no duplicate-send storm:** beta flushes **once** (one `received` line; durable row acked once).

### Roles
- **alice (this session):** step 3 (overview edit during the gap) + verify (a) daemon received bob's beta
  + alice applied changed=true, and (c)/(d). Watches the daemon log from line 251.
- **bob:** steps 1, 2, 4 + verify (b) overview slug appears + his beta reached alice; report
  `pending_kb_updates` while offline (proves the durable queue held it).

### ✅ T3 RESULT: PASS (alice + bob).
- bob→alice flush (a): daemon `kb/node_update received → applied wal_seq=88`; alice `changed=true`,
  `beta = [BOB-OFFLINE-1]`. alice→bob catch-up (b): bob `overview = [ALICE-WHILE-BOB-OFFLINE]`.
  No revert (c): `alpha = [ALICE-T2-POSTRESTART]` control held. Single flush, acked once (d).
- **bob yellow flag → FIXED (`6a1a5604`):** while offline, `introspect.pending_kb_updates` read **0**
  even though the durable SQLite row exists (B-16 single-source leaves the in-mem Vec empty). Root
  cause was **observability, not durability** — `kb_update_node` persists to the durable queue at
  *edit time* with no connection check (crash-durable, modulo sled's ~500ms flush). Fix: introspect
  now reports `pending_kb_updates = in-mem + durable` + a `durable_pending_kb_updates` breakdown
  (new `KbStore::count_pending_updates`), and the offline enqueue is logged
  (`edit: persisted to durable pending queue`). **Both editors must rebuild to `6a1a5604`+ to SEE the
  new counts** (the durability itself already worked).

---

## T3b — offline-durability across an EDITOR RESTART: READY-TO-RUN procedure

**Goal / what it proves:** an edit made **while offline survives a full PROCESS restart** of the
editor (quit + relaunch), then flushes on reconnect — the strongest form of "never silently lose."
Also live-validates the new observability (`durable_pending_kb_updates` ≥ 1 while offline / before flush).

**Prereq:** both editors on **`6a1a5604`+** (`git pull && make build` → install → restart). alice tails
`/tmp/mae-daemon-live.log`. **Probe slug:** bob `collabtest:beta` → `[BOB-T3B-OFFLINE]`.

### Steps (ordered)
0. **Pre-check (both):** connected + synced; `kb_get beta` matches on both. bob `introspect.collaboration`
   → `pending_kb_updates: 0`, `durable_pending_kb_updates: 0`.
1. **bob:** `:collab-disconnect` → offline (`collab_status` disconnected).
2. **bob (offline):** edit `collabtest:beta` → `[BOB-T3B-OFFLINE]`.
   - **Observability check (the fix):** bob `introspect.collaboration` → **`pending_kb_updates ≥ 1`** and
     **`durable_pending_kb_updates ≥ 1`**. bob log: `edit: persisted to durable pending queue (survives
     offline + restart)`. Daemon log: **no** `kb/node_update` for beta (still offline). **Tell alice "offline+edited".**
3. **bob:** **QUIT the editor** (graceful exit) — while still offline. Do NOT reconnect first.
4. **bob:** **relaunch** the editor. Startup loads collabtest from disk (B-10). **Before the post-restart
   reconnect flushes** (may be fast — best-effort), `introspect` → `durable_pending_kb_updates ≥ 1`
   (the queue **survived the process restart** — this is the crux). bob startup log:
   `KB instance loaded from CozoDB nodes=3`.
5. **bob:** reconnect (auto on launch, or `:collab-connect`) → durable queue flushes + auto-rejoin.
6. **PASS criteria:**
   - **(a) survives restart + flushes:** after relaunch, daemon logs `kb/node_update received …
     node_id=collabtest:beta → applied wal_seq=…`; **alice** `recv: applied … changed=true`,
     `kb_get beta = [BOB-T3B-OFFLINE]`. (The edit made before the quit reached alice after the restart.)
   - **(b) durable visibility:** `durable_pending_kb_updates ≥ 1` observed while offline (step 2) and/or
     post-relaunch-pre-flush (step 4); returns to 0 after the flush+ack.
   - **(c) no loss:** beta = `[BOB-T3B-OFFLINE]` on both; no revert of other nodes.
   - **(d) once:** beta flushes exactly once (single `received`; durable row acked once).

### Roles
- **bob:** steps 1–5 + the two observability captures (step 2 offline, step 4 post-relaunch).
- **alice (this session):** mark a fresh daemon baseline at step 5; verify (a) the daemon received bob's
  `[BOB-T3B-OFFLINE]` **after his relaunch** + alice applied `changed=true`, and (c)/(d).

> Note on auto-connect: if bob's editor auto-connects on launch, step 4's pre-flush window is brief —
> the reliable durable-count capture is step 2 (offline). The crux (a) holds regardless: the edit made
> before the quit must arrive at alice after the relaunch, proving the queue survived the restart.

### ✅ T3b RESULT: PASS (alice + bob, on `6a1a5604`).
The crux holds: bob edited `beta → [BOB-T3B-OFFLINE]` **while offline**, **quit + relaunched the
editor**, reconnected → the durable pending row **survived the process restart** and flushed:
daemon `session=8 kb/node_update received → applied wal_seq=92`; alice `recv: applied … changed=true`,
`kb_get beta = [BOB-T3B-OFFLINE]`. Single flush (d); bob confirms the observability fix
(`durable_pending_kb_updates ≥ 1` while offline). ⇒ offline edits are durable across a graceful
editor restart.

## T3c — non-graceful CRASH (`kill -9`): CHARACTERIZATION procedure (observe, don't pass/fail)

**Goal:** gather DATA (not a pass/fail) on what a hard crash does to unsynced edits, so we can design
the fix from real observations on both sides. Three questions: (1) **content durability** — do bob's
offline edits survive in his local KB after `kill -9`? (2) **sync-intent durability** — do the durable
`pending_updates` rows survive? (3) **reconnect-adopt clobber** — for an edit whose content survived
but whose sync-intent was lost, does the rejoin **overwrite** it with the daemon's older snapshot?

**Why it matters (task #38):** node *content* (crdt_doc) is persisted separately from the pending-sync
queue; the B-14 adopt-on-join *replaces* the local node with the daemon snapshot. So a durable-but-
unsynced local edit could be silently clobbered on rejoin = lost work.

**Prereq:** both editors on `6a1a5604`+ (durable-pending observability). alice tails the daemon log +
marks a fresh baseline at step 5. **Probe slugs:** `[BOB-T3C-1]` (alpha), `[BOB-T3C-2]` (beta),
`[BOB-T3C-3]` (overview) — three nodes so we see per-node behavior; do the last one+kill back-to-back
to probe the sled flush window.

### Steps (ordered)
0. **Pre-check (both):** connected + synced; `kb_get` alpha/beta/overview match on both; bob
   `introspect.collaboration` → `pending_kb_updates: 0`, `durable_pending_kb_updates: 0`.
1. **bob:** `:collab-disconnect` → offline.
2. **bob (offline):** edit, in quick succession — `alpha → [BOB-T3C-1]`, `beta → [BOB-T3C-2]`,
   `overview → [BOB-T3C-3]`. Then **record**: bob `introspect` → `durable_pending_kb_updates` (expect
   ≈ 3); bob `kb_get` each → confirm the three slugs locally. Daemon log: NO kb/node_update (offline).
3. **bob:** **`kill -9`** the editor *immediately* after the last edit — `kill -9 $(pgrep -f 'bin/mae$')`
   (NOT a graceful quit; no flush-on-drop).
4. **bob:** relaunch the editor. **Before reconnect flushes** (best-effort), record:
   - **Obs A — content durability:** `kb_get` alpha/beta/overview → which of `[BOB-T3C-1/2/3]` survived
     locally (vs reverted to the pre-edit title)?
   - **Obs B — sync-intent durability:** `introspect` → `durable_pending_kb_updates` = ? (how many
     pending rows survived the crash). startup log: `KB instance loaded from CozoDB nodes=3`.
5. **bob:** reconnect → re-join + drain. **alice marks a fresh daemon baseline here.**
   - **Obs C — re-sync (alice):** daemon log — which `[BOB-T3C-*]` edits `received → applied`? alice
     `kb_get` each → which converged on alice.
   - **Obs D — clobber (bob):** AFTER reconnect, bob `kb_get` each again → did any node that showed a
     `[BOB-T3C-*]` slug in **Obs A** now **revert** to an older value (clobbered by the rejoin adopt)?
6. **Record the matrix from BOTH sides** (no pass/fail — this is the design input):

   | Node | offline edit | Obs A: post-crash local (bob) | Obs B: pending survived? | Obs C: reached alice? | Obs D: post-reconnect local (bob) | clobbered? |
   |------|-------------|-------------------------------|--------------------------|-----------------------|-----------------------------------|------------|
   | alpha | `[BOB-T3C-1]` | ? | (part of B count) | ? | ? | ? |
   | beta | `[BOB-T3C-2]` | ? | | ? | ? | ? |
   | overview | `[BOB-T3C-3]` | ? | | ? | ? | ? |

### Roles
- **bob:** steps 1–5 + Obs A, B, D (local kb_get + introspect, pre- and post-reconnect).
- **alice (this session):** fresh daemon baseline at step 5; Obs C (which edits reach the daemon +
  alice's final converged state). Cross-check bob's Obs A/D against what the daemon received.

### What the data tells us (fix design, AFTER observations)
- If **Obs A = all survived** and **Obs D = none clobbered** and **Obs C = all reached alice** → the
  current path is already crash-robust (sled flushed in time + drain recovered); document the window.
- If **Obs A survived but Obs C missing + Obs D reverted** → confirms the **adopt-clobber**: fix =
  adopt-on-join must reconcile local-ahead content up (state-vector diff / `reconcile_to`) before/instead
  of replacing, and/or the drain must re-derive the sync delta from local-vs-daemon on reconnect.
- If **Obs A itself lost edits** → the sled flush window dropped content; fix = flush-on-write / fsync
  for the node-content + pending writes (durability tuning), independent of the adopt path.

### Known-open (not blocking the matrix)
- B-12 idle-eviction edge: a collection evicted while everyone's offline, then re-shared, could still
  recreate (narrow) — closed properly by the ADR-021 durable audit record (tracked).
- The two-independent-peers **automated** e2e (so CI catches this class, not just two machines) — next code task.
- B-11 (`*Collab Status*` steals the dashboard on launch) — Stage-2 UX.

---

## T3c-stress — ADR-022 crash-safety: READY-TO-RUN (PASS/FAIL)

The prior T3c was a *characterization* on `6a1a5604` (old adopt path). **ADR-022 has
landed** (`reconcile_remote_node` + SV-reconcile on (re)join; B-17 fixed): (re)join now
**merges** the daemon's SV-diff and re-derives any local-ahead edit from the durable
`crdt_doc` — independent of the pending-queue row surviving. So T3c is now a **PASS/FAIL**:
a non-graceful `kill -9` must lose **no** durable edit and **clobber nothing**.

> **Why no 3rd machine is needed, and no surgical "delete the row":** convergence for N
> peers is already proven *deterministically* in-process (`kb_sync_n_peer_e2e.rs`, N∈{2,3,5},
> incl. `lost_row_reconcile_converges`) + the real-daemon e2e
> (`kb_join_with_svs_returns_reconcile_diff_else_full_state`). The live run confirms the
> *integration* on real machines + LAN. The reconcile fires on **every** rejoin, so a plain
> `kill -9` exercises it; we then **observe** `durable_pending_kb_updates` after relaunch to
> see *which* path recovered the edit (queue-replay vs reconcile). The pending rows live in
> the **same sled DB** as content and `kb_raw_query` is read-only, so there is no clean live
> "delete just the intent" — and we don't need one: the deterministic lost-row case is the
> in-process suite's job; the live job is "real crash → converge, no clobber."

### §0 — PREREQ: rebuild BOTH machines from the branch (this code is unreleased)
1. **both:** `git fetch origin && git checkout feat/crdt-collab-validation && git pull`
   (HEAD should be `cb3a65eb` *docs: ADR-022 as-built* or later).
2. **both:** build from source (no longer installed-binary-only):
   - editor: `make build` (GUI; Skia ok on both) → `./target/release/mae`
   - daemon (alice only): `(cd daemon && cargo build --release)` → `daemon/target/release/mae-daemon`
   - shim (alice only, for MCP drive): `cargo build --release -p mae-mcp` → `mae-mcp-shim`
3. **alice:** restart the daemon **on the new binary**, same params as prior runs
   (`--bind 0.0.0.0:9480`, log → `/tmp/mae-daemon-live.log`). It eager-recovers `collabtest`
   + membership from WAL (B-12), so bob/alice stay authorized. Confirm log line:
   `daemon listening … 0.0.0.0:9480`.
4. **both:** launch the new editor; **alice** reconnects the MCP shim to the new PID socket.
5. **sanity (the T1/T2 baseline, fast):** bob `:collab-connect` → daemon log
   `authenticated … peer=bob`; bob `:kb-join collabtest` → bob log shows the 3 nodes
   reloaded/joined; alice edits `collabtest:alpha → [BASE-1]`, bob sees it (`changed=true`);
   bob edits `collabtest:beta → [BASE-2]`, alice sees it. **Baseline bidirectional OK.**
   - **New daemon signal to confirm ADR-022 is live:** on bob's join, the daemon logs
     `kb/join: complete … reconcile_mode=true diff_count=N` (reconcile path, not full-state).

### §1 — T3c-stress: offline edit → instant `kill -9` → relaunch → reconnect
**Probe slugs:** `[BOB-T3CS-1]` (alpha), `[BOB-T3CS-2]` (beta), `[BOB-T3CS-3]` (overview).
alice marks a fresh daemon-log line number here.
1. **bob:** `:collab-disconnect` (so the edits are genuinely unsynced when the crash lands).
   → daemon log: bob's session ends.
2. **bob (offline):** edit in quick succession `alpha→[BOB-T3CS-1]`, `beta→[BOB-T3CS-2]`,
   `overview→[BOB-T3CS-3]`. Record `introspect → durable_pending_kb_updates` (expect **3**).
3. **bob:** **`kill -9 $(pgrep -f 'bin/mae$')`** *immediately* after the last edit (aim inside
   the ~500 ms sled-flush window — back-to-back with step 2's last edit).
4. **bob:** **relaunch** `./target/release/mae`. **Before** it auto-reconnects/flushes, record:
   - **Obs A — content durability (bob):** `kb_get` alpha/beta/overview → which `[BOB-T3CS-*]`
     survived the crash locally? (Disk-first loader reloads from the durable `crdt_doc`.)
   - **Obs B — intent survival (bob):** `introspect → durable_pending_kb_updates` → 3, <3, or 0?
     *(0 here + convergence below = live proof the **reconcile** recovered it, not the queue.)*
5. **bob:** `:collab-connect` (if not auto). The reconnect fires `kb-resubscribe → JoinKb` with
   bob's per-node **state vectors** → daemon SV-diff → **reconcile + local-ahead re-sync**.
6. **alice (Obs C — re-sync reached the hub):** daemon log from the step-1 mark — for each
   surviving `[BOB-T3CS-*]`: `kb/node_update … received → applied wal_seq=…`; alice editor
   `recv: applied … changed=true`. Then alice `kb_get` alpha/beta/overview.
7. **bob (Obs D — no clobber):** AFTER reconnect, `kb_get` each again → did any node that showed
   a `[BOB-T3CS-*]` slug in Obs A **revert** to an older value? (The old adopt-clobber risk.)

### PASS criteria (all four)
- **(a) No durable loss:** every edit present in **Obs A** (content that reached disk) is, after
  reconnect, present on **alice** (Obs C) — converged, `changed=true` on apply.
- **(b) No clobber:** **Obs D** shows **no** node reverting; `[BASE-1]/[BASE-2]` and any
  alice-side edits are intact on both peers.
- **(c) Recovery is reconcile-driven (or queue, but it converges either way):** record **Obs B**.
  If `durable_pending_kb_updates == 0` after relaunch *and* (a) still holds → **the SV-reconcile
  re-derived the edit from the durable content** (the ADR-022 guarantee, live). If >0, the queue
  also replayed — still PASS, note which.
- **(d) Bounded:** after the post-reconnect drain+ack, bob `durable_pending_kb_updates → 0`
  (no stuck/duplicated intents); a single apply per edit on alice (no double-send).
- **Edge:** if **Obs A itself lost an edit** (content never flushed before `kill -9`) → that edit
  is *expected* unrecoverable (nothing durable to reconcile); it's the residual flush-window, NOT
  a clobber. Note it; it's the separate `crdt_doc` flush-on-write task, not an ADR-022 failure.

### §2 — (OPTIONAL) 3-way fan-out: carol = 2nd editor on alice (read-only observer)
A 3rd peer needs only a distinct **identity + data dir**, not a 3rd machine. If quick on the day;
else **leave strict N-peer to CI** (the in-process harness covers N∈{2,3,5}).
1. **alice:** start carol = a 2nd editor with an isolated env + its own identity:
   `HOME=/tmp/mae-carol XDG_CONFIG_HOME=/tmp/mae-carol/.config XDG_DATA_HOME=/tmp/mae-carol/.local/share ./target/release/mae` (own MCP socket).
   Generate/point its `--collab-identity` so it has a distinct fingerprint.
2. **alice (owner):** authorize carol's key on the daemon + `:kb-member-add collabtest <carol-fp> viewer`.
3. **carol:** `:collab-connect` + `:kb-join collabtest` → receives the current converged state.
4. **After §1 step 6 converges:** carol `kb_get` alpha/beta/overview → must show the **same**
   `[BOB-T3CS-*]` values as alice. PASS = bob's recovered edit fanned out to a 3rd independent
   subscriber (not just the owner) — hub-and-spoke N-peer convergence, live.

### Roles
- **bob:** §0 build + §1 steps 1–7 (offline edits, the `kill -9`, relaunch, Obs A/B/D).
- **alice (this session, MCP-driven):** §0 daemon rebuild+restart + baseline; the step-1 daemon-log
  mark; Obs C (received + converged); optional carol observer; records the verdict here.

### ✅ T3c-stress RESULT: PASS (alice + bob), 2 runs, on `a8650ea8` (ADR-022 build)

**§0 baseline (new build, both machines):** bob's reconnect logged `kb/join: complete
reconcile_mode=true diff_count=3` — the ADR-022 SV-reconcile join is **live cross-machine**
(bob sent per-node SVs, daemon replied with diffs, not full snapshots). Bidirectional
confirmed: alice `alpha→[BASE-1]` (wal 105) reached bob; bob `beta→[BASE-2]` (wal 106)
reached alice.

**The crash test — bob edits 3 nodes offline → `kill -9` → relaunch → reconnect:**

| Run | relaunch | slugs | daemon applied | converged on alice | clobber (bob Obs D) |
|-----|----------|-------|----------------|--------------------|----|
| 1 | auto-connected | `[BOB-T3CS-1/2/3]` | wal 107/108/109 | ✅ all 3 | ✅ none (green) |
| 2 | offline (auto-connect off) | `[BOB-T3CS2-1/2/3]` | wal 110/111/112 | ✅ all 3 | ✅ none (green) |

- **(a) No durable loss — PASS:** every offline edit survived the `kill -9` and converged on
  alice in both runs (alice `kb_get` = the latest `[BOB-T3CS*-n]` on all three nodes).
- **(b) No clobber — PASS:** bob's post-reconnect `kb_get` showed no reverts ("all green");
  `[BASE-*]` and the chained edits all intact. The reconcile join **merged** on top of the
  drained edits rather than replacing — the exact contrast with the old adopt-clobber.
- **(c) Mechanism — both runs hit the *queue-survived* branch.** The daemon-log **ordering**
  is the tell: in both runs the 3 `kb/node_update`s applied **before** the `kb/join` line →
  the reconnect drained the surviving pending rows first, *then* the `reconcile_mode=true`
  join ran and merged. So the pending rows flushed to sled before the manual `kill -9` both
  times (sub-~500 ms edit→kill is hard to hit by hand). **The lost-row branch (row gone →
  reconcile re-derives from durable `crdt_doc`) was NOT triggered live** — it stays
  deterministically covered in CI (`kb_sync_n_peer_e2e::lost_row_reconcile_converges`,
  `reconcile_remote_node_lost_row*`, and the daemon `kb_join_with_svs_returns_reconcile_diff`
  e2e). The live run confirms the integrated *crash → converge, no clobber* behavior + that
  the new reconcile join doesn't disturb the drain-first happy path.
- **(d) Bounded — PASS:** each edit applied exactly once (wal monotonic, no dup/stuck rows).

**New diagnostic (reusable):** the daemon-log order distinguishes the two recovery paths —
node_updates **before** `kb/join: complete` = pending-queue drain (row survived);
**after** it = SV-reconcile local-ahead re-derive (row lost). No editor-side timing capture
needed to tell which fired.

**Follow-ups surfaced:**
- To exercise the **reconcile-only (lost-row)** path *live* on demand, we'd need to clear
  bob's durable `pending_updates` between crash and reconnect — but it shares the sled DB
  with content and `kb_raw_query` is read-only. A small **dev/test affordance** (a gated
  "clear pending queue" command, or a `MAE_KB_DISABLE_PENDING_QUEUE` env for a run) would let
  a live run force the reconcile path. Low priority — CI already proves it deterministically.
- `crdt_doc` **flush-on-write** (task #39 split): the only residual loss window is content
  that never flushed before a hard crash; neither reconcile nor queue can recover that.

---

## T4–T7 — live matrix: READY-TO-RUN (PASS/FAIL)

Continues the matrix after T3c-stress PASS. **No rebuild needed** — both machines are on the
ADR-022 build (`a8650ea8`+) from the T3c run; daemon up on `0.0.0.0:9480`, log
`/tmp/mae-daemon-live.log`. KB = `collabtest` (nodes `alpha`/`beta`/`overview`), bob =
`192.168.1.129`, fp `SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI`; alice owner, fp
`SHA256:+jBinAwoF1KCDqCw3TWb3T3L4xytAVKHofvTnLSn69k`.

**§0 baseline (fast):** both connected (`:collab-connect`), bob joined (`:kb-join collabtest`
→ daemon `reconcile_mode=true`). alice edits `alpha→[BASE]`, bob sees it; bob edits `beta→[BASE]`,
alice sees it. alice marks a fresh daemon-log line before each T-step.

### T4 — concurrent same-node convergence (the B-16 guard, live)
**Goal:** two peers edit the SAME node concurrently → both converge to **one identical value**
(distinct per-peer `client_id`; the old hardcoded `client_id=1` would diverge).
1. **both:** `:collab-disconnect` (forces the edits to be genuinely concurrent — neither sees the
   other before reconnect).
2. **alice:** `alpha` title → `[A-T4]`. **bob:** `alpha` title → `[B-T4]`. (Same node, different
   values, while both offline.)
3. **both:** `:collab-connect` (order irrelevant). Each reconnect SV-reconciles + pushes local-ahead.
4. **PASS:** after convergence, `kb_get alpha` title is **identical on alice and bob** (a
   deterministic CRDT merge of the two — e.g. interleaved/one-wins-by-id; the *value* isn't
   predicted, but it **must match** on both). No split-brain. Daemon log shows both `kb/node_update`
   for `alpha` applied; each peer ends on the same materialized title.
   - **FAIL signal:** alice shows `[A-T4]`, bob shows `[B-T4]` (or any divergence) → lineage/client_id
     regression.

### T5 — body + multi-field edits
**Goal:** edits beyond the title (body uses a separate `YText`; tags a `YArray`) sync both ways,
including a single edit touching multiple fields.
1. **alice:** edit `alpha` **body** — append a sentinel line `[A-T5-BODY]`. → bob `kb_get alpha`
   body contains `[A-T5-BODY]` (daemon `changed=true`).
2. **bob:** edit `beta` **body** + **title** in one save (`title=[B-T5]`, body append `[B-T5-BODY]`).
   → alice `kb_get beta` shows both the new title AND the body sentinel.
3. **alice:** edit `overview` **tags** (add `t5tag`). → bob `kb_get overview` tags include `t5tag`.
4. **PASS:** all three converge on the receiving peer — body, multi-field (title+body atomically),
   and tags. No field "sticks" (the B-15 class — an edit that doesn't enter the CRDT).

### T6 — daemon restart mid-session
**Goal:** restart the hub while peers are connected → eager-recovery from WAL + auto-reconnect →
sync resumes, pre-restart state intact, an edit made during the outage flushes on reconnect.
1. **both:** confirm connected + synced (a quick `alpha→[T6-PRE]` from alice lands on bob).
2. **bob (during what will be the outage):** `:collab-disconnect`, edit `beta→[B-T6-DURING]`
   (durable, queued), stay offline.
3. **alice:** restart the daemon (graceful `kill -TERM` → relaunch the ADR-022 `mae-daemon`,
   same `0.0.0.0:9480`). Daemon log: `recovering collab documents … complete count=4` +
   `preserving membership (B-12)`.
4. **alice editor:** auto-reconnects (backoff) → daemon `peer=alice` + `kb/share`. **bob:**
   `:collab-connect` → `reconcile_mode=true`; his `[B-T6-DURING]` drains up.
5. **PASS:** (a) daemon recovered all 4 docs from WAL; (b) both peers reconnect; (c) pre-restart
   content intact on both (`[T6-PRE]` not reverted); (d) bob's during-outage `beta→[B-T6-DURING]`
   converges on alice after reconnect — no loss across the hub restart.

### T7 — roles / policy enforcement (ADR-018)
**Goal:** a viewer cannot write; restoring editor re-enables writes. (Membership is enforced at the
daemon: `kb/node_update` from a non-writer is rejected.)
1. **alice (owner):** `:kb-member-add collabtest SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI viewer`
   → daemon logs the membership update; broadcast.
2. **bob:** edit `alpha title → [B-T7-DENIED]`. → daemon log: `kb/node_update … rejected`/access
   denied for bob (viewer); **alice does NOT receive it** (`kb_get alpha` unchanged). bob may see a
   permission error / the edit stays local-only.
3. **alice:** restore — `:kb-member-add collabtest SHA256:9xLh0DWeeAi3hl2W7yudaE05aTHtYQpNUUyMWO+2CrI editor`.
4. **bob:** edit `alpha title → [B-T7-ALLOWED]`. → now daemon **applies** + broadcasts; **alice
   receives** `[B-T7-ALLOWED]`.
5. **PASS:** the viewer write is rejected at the daemon (not reaching alice); after role restore the
   write propagates. Membership change is owner-only (a bob-issued `:kb-member-add` must be rejected —
   optional sub-check).

### Roles & driving
- **bob:** the per-step edits, disconnect/connect, and his Obs (`kb_get`, `introspect`).
- **alice (MCP-driven):** owner-side edits + membership commands, daemon-log marks + Obs C
  (received/applied/rejected), the daemon restart (T6), records verdicts here.
- **Note (alice MCP):** T7 membership is now **MCP-drivable** via the `kb_add_member` /
  `kb_remove_member` tools (added 2026-06-22 — `crates/ai/src/tools/kb_tools.rs` +
  `executor/collab_exec.rs`), so alice drives it over MCP, no manual command-line entry. (Requires
  alice's editor **rebuilt** to expose the new tools — do this before T7; T4–T6 don't need it.)
  T4's forced concurrency uses the disconnect-edit-reconnect pattern (same as T3's offline edits).

### ✅ T4 RESULT: PASS (alice + bob)
Both disconnected, edited `alpha` title concurrently (alice `[A-T4]`, bob `[B-T4]`, shared base
`[BOB-T3CS2-1]`), both reconnected. Daemon log (mark 2866): alice push `wal_seq=116`, bob push
`wal_seq=120` + `kb/join reconcile_mode=true`. **Converged byte-identical on both machines:**
`Collab Test Alpha [B-T4]Collab Test Alpha [A-T4]` (bob's then alice's full title, no space —
concurrent full-replace interleave, deterministically ordered by client_id). **Both edits survived,
single value, no split-brain** → the per-peer-client_id (B-16) guard holds live. Cross-checked
alice `kb_get` == bob's recorded string (byte-for-byte).

### T5 RESULT: title ✅ + body ✅ PASS · tags ❌ → **B-18 found + FIXED** (`97af88df`)
- **Body (YText), alice→bob:** ✅ alice appended `[A-T5-BODY]` to `alpha` body → bob applied
  `changed=true`, title unaffected (independent fields).
- **Multi-field (title+body atomic), bob→alice:** ✅ bob set `beta` title `[B-T5]` + body `[B-T5-BODY]`
  in one save → **single** `kb/node_update` → alice `kb_get beta` shows BOTH (the B-15 guard, live).
- **Tags (YArray):** ❌ FAIL → **B-18.** alice's `t5tag`/`t5clean` adds reached bob as real payloads
  (`wal_seq=124`, 1628 bytes) but applied `changed=false`; bob's tags stayed `[collabtest,fixture]`.
  My first run was muddled (batched the tag edit with the body edit); bob's **controlled re-run**
  (clean isolated `t5clean` add) confirmed it.
  - **Root cause (code-confirmed):** `KbNodeDoc` had `tags()` but no `set_tags`, and
    `upsert_with_crdt` only wrote `set_title`/`set_body` — a tags-only edit never entered the CRDT
    (B-15 class, never extended to tags). Receive side (`apply_crdt_doc → self.tags = doc.tags()`) was
    fine; the **send** side was the gap.
  - **FIX (`97af88df`):** added `KbNodeDoc::set_tags` (clear+reinsert YArray) + wired into
    `upsert_with_crdt`. Tests: mae-sync `set_tags` round-trip + mae-kb production-fidelity
    tag-only-edit-converges regression. **Links/meta** share the latent send-side gap (not in the
    active `kb_update` path — links derive from body text) → follow-up.
  - **LIVE RE-VERIFY pending:** needs BOTH editors rebuilt (shared/sync+shared/kb changed) — re-run
    the clean tags add after the rebuild; expect bob to converge. (Daemon doesn't need it — it relays
    the bytes; the editor's send path is where the fix lives.)
