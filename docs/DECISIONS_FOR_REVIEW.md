# Decisions parked for human review

Working notes from the pre-v0.15 remediation effort. Each entry is a judgment call that
genuinely needs Hayden's input — not something prior art can settle, and not something with an
obvious default. Work continued around each one; nothing here is blocking.

Delete an entry once it is decided (and record the decision in the relevant ADR).


## Decisions taken (Hayden, 2026-08-03)

| # | Topic | Decision |
|---|---|---|
| 0 | Disclosure sequencing | **Push.** Done; #612 merged, #613 open. |
| 1 | Default permission tier | **Three-state model (allow/ask/deny) is in v0.15 scope.** Build the ask state, then lower the default. |
| 2 | RCE advisory / CVE | **None needed** — no other users today, so no separate advisory, no CVE, no v0.14 backport. The draft advisory stays unpublished. |
| 3 | ADR-085 category split | **Ship as a breaking change** with release notes naming all eight relocated tools. |
| 4 | `Window::cursor_col` | **Do the byte migration in v0.15**, not just the declaration. |
| 5 | `dispatch_builtin` | **Change the contract properly** — a richer outcome across the ~559 command implementations. |
| 6 | KB sharing/membership tiers | **Raise to Privileged.** Implemented — see §6. |
| 7 | ADR-087 follow-ups | Implied by #4 — `wrap.rs`'s third width impl and threading `WidthPolicy` past the status bar. |
| 8 | Chokepoint assert | **Resolved** in `51158578`. |
| 9 | Nine phantom MCP tools | **Wire up the six non-interactive ones** (session handle into `dispatch_tool` — new capability), and exclude the three interactive ones from external discovery. Implemented — **ADR-091**. |
| 10 | `fts_search` term miss | **Investigate before v0.15**, with a property test over a multi-term corpus. |

Entries below are kept for their analysis; the Decision column above supersedes each one's
"My recommendation".

---

## 0. RESOLVED — push approved

**Decision (Hayden, 2026-08-03): push.** Option 2 from the list below. The branch push itself was
blocked by a tooling permission classifier, so it must be run by hand:
`git push -u origin security/ai-permission-enforcement`.

Remaining live consideration: **merging** the PR triggers `version-bump.yml`, which tags and
publishes automatically. So the exposure window is push → merge, and it is controlled entirely by
when the merge happens. Merging promptly closes it.

The original analysis is kept below for the record.

---

**Status:** fixes are written and committed locally on `security/ai-permission-enforcement`.

`cuttlefisch/mae` is public. The commits fix a local RCE (§2 below) and their messages describe the
exploit chain in full, including a working reproduction. Pushing the branch — or opening a PR —
publishes a trivially-exploitable, unpatched-in-any-release vulnerability to anyone watching the
repo. Every released 0.14.x is affected, and the exploit is a one-line file in a repository.

That is irreversible in a way nothing else in this effort is, so it needs a human decision rather
than a default.

**A constraint that shapes the options.** `.github/workflows/version-bump.yml` triggers on
`pull_request: [closed]` against `main` and bumps, tags, and releases automatically **on merge**.
So the release cannot precede the PR through the normal flow — by the time v0.14.89 exists, the
branch and its commit messages are already public. "Release first, then disclose" requires
deliberately going around the PR flow.

**The options:**

1. **Bypass the PR flow for this one release.** Push a tag directly (or run the bump manually) so
   v0.14.89 is published, *then* push the branch and the advisory together. Shortest exposure
   window, but it means hand-driving a release path that normally runs itself, and the automation's
   own comments note it has previously half-completed when raced.
2. **Push and PR now; merge immediately.** Normal workflow. Exposure window is push → merge → CI →
   release, which the workflow's own comments say includes a ~35-minute CI wait. That is the window
   during which a public, unpatched, trivially-exploitable RCE is described in a public commit
   message.
3. **Squash the security commits into one with a terse message** ("fix: require explicit trust for
   project-local init files"), push, merge, release, then publish the full advisory and the detailed
   rationale afterwards. Keeps the normal flow and shortens what a casual observer learns, though
   the diff still shows the fix — which for a trust-boundary bug is enough to infer the bug.

**My recommendation:** option 3, then option 1 if you want the window closed properly. Option 2 is
the default-if-nobody-decides and is the worst of the three.

Note that PR #612 is already public and already states MAE's permission tiers are not enforced, so
*that* half is effectively disclosed. The `.mae/init.scm` execution path is not, and it is both more
serious and easier to exploit.

**What I did instead:** kept implementing on the local branch and prepared everything so the
release is quick once you decide. Nothing is blocked on this except the push itself.

---

## 1. The default permission tier is permissive, and fixing it is a feature, not a patch

**Status:** analysed, blocked on a product call. See ADR-090.

MAE ships `auto_approve_tier = "trusted"` (= Shell), so the default posture is *all categories ×
shell tier*. Lowering it is the single highest-leverage security change available.

It cannot be done as a config change. `is_allowed` hard-denies rather than prompting — there is no
"ask" state on the dispatch path — so any lower default makes the AI unable to run builds or tests
at all, rather than asking first. `mae-agent` (the default surface, ADR-049) *does* have a
y/n/always prompt; the embedded and MCP paths do not.

**The call:** is building the three-state model (allow / ask / deny) in scope for v0.15, or does
v0.15 ship with the permissive default and a documented warning? Every comparable agent surveyed is
three-state, and each can afford a restrictive default *because* restriction means "ask".

**My recommendation:** in scope, because the external-MCP path is v0.15's headline deployment shape
and it is currently the weakest surface. But it is a real feature with UI work in it, and that is a
schedule decision rather than a technical one.

---

## 2. Whether the workspace-trust RCE gets its own advisory and CVE

**Status:** fixed in code; disclosure structure undecided.

The project-local `.mae/init.scm` execution issue is filed as §5 of the AI-permission advisory
`GHSA-qwh8-m8j6-563h`, because that is where I found it. It does not belong there: it needs no AI
agent, no MCP client, and no prompt injection, and its fix (workspace trust) is unrelated to the
other four findings. Cloning a repository and opening MAE in it was arbitrary code execution.

**The call:** split it into its own advisory with its own identifier, and decide whether to request
a CVE. Also whether the fix warrants a backported patch release on the v0.14 line rather than
waiting for v0.15, given every 0.14.x is affected.

**My recommendation:** separate advisory, request a CVE, and backport. The affected-version range
is "all released versions", and the exploit is a one-line file in a repository.

---

## 3. ADR-085's category split is a behaviour change with no deprecation period

**Status:** implemented; release-note wording needs a human.

Eight tools reachable under `mcp_tool_category_allowlist = "knowledge"` no longer are:
`babel_execute`, `babel_tangle`, and — found by the registry-wide invariant test, not by the
original audit — `kb_raw_query` (Privileged, arbitrary Datalog), `kb_export_subgraph_html`,
`kb_enrich`, `kb_register`, `kb_reimport`, `org_export`.

The audit found two of these by reading code. The invariant found all nine violations (the ninth,
`web_fetch`, needed no move — see ADR-085's implementation note). That ratio is the argument for
registry-driven enforcement over sampling, and worth remembering when scoping the remaining
workstreams.

Anyone relying on `knowledge` to reach those must now name `execution` explicitly. Since reaching
shell via `knowledge` was never intended, this is a fix rather than a regression — but it is a
breaking change for any existing configuration, shipped without a deprecation cycle.

**The call:** ship it in v0.15 as a breaking change with release notes, or add a transitional
period where the old behaviour warns before it stops working. A transitional period means shipping
a known privilege-escalation path for one more release.

**My recommendation:** ship it. A deprecation window for a security fix is a deliberate decision to
keep the hole open, and the migration is one word in one config value.

---

## 4. `text_utils::truncate_end` is wrong, and it is the helper the plan said to standardise on

**Status:** confirmed by reading the code; fix approach depends on pending research.

The remediation plan said the byte-slicing class was "use the existing helper, not write one".
That was wrong. `text_utils::display_width` sums *per-character* widths while
`crates/core/src/grapheme.rs` uses `UnicodeWidthStr::width` on the whole string, and the
`unicode-width` crate documents these as deliberately different for ZWJ sequences, emoji modifiers,
and several scripts' ligatures. For a family emoji the per-char sum returns 8 where the real width
is 2. `truncate_end` then iterates `char_indices()`, so it can also cut *between* a ZWJ and its base
character.

So MAE has two `display_width` implementations, one of which is wrong, and the wrong one is the one
the "safe" truncation helpers are built on.

**The call:** none needed on deleting the per-char version — that is unambiguous. The open question
is how far to go: whether MAE adopts distinct `ByteOffset`/`CharOffset`/`ColumnOffset` newtypes so
the compiler catches domain mixing, which is the only mechanism that would have caught this class
structurally. That is a large, invasive refactor across every rendering path.

**My recommendation:** pending the enforcement research. Deferring the newtype question and fixing
the helpers is the cheap correct move; the newtype refactor is a v0.16 conversation if it happens at
all.

---

## 5. `dispatch_builtin` returns "name recognised", and two bridges read that as "succeeded"

**Status:** found while landing ADR-086 (commit 9007bf28 references this entry; it was never
actually written here until the pre-v0.15 audit tail pass found the gap). Not changed; needs a call.

`Editor::dispatch_builtin` (`crates/core/src/commands.rs`) returns `bool` meaning "this command name
was recognised and routed" — not "the command did what was asked". Two bridges treat that recognition
signal as success:

- `crates/ai/src/executor/tool_dispatch.rs::execute_registry_command` (the generic handler behind
  every `command_*` MCP tool — roughly 559 of them, one per registered editor command):
  ```rust
  if editor.dispatch_builtin(&cmd_name) {
      Ok(format!("Executed: {}", cmd_name))
  } else {
      Err(format!("Unknown command: {}", cmd_name))
  }
  ```
  A command that is recognised but silently no-ops (wrong mode, no active selection, nothing to
  operate on, an internal refusal that isn't `Unknown command`) still returns `Ok("Executed: ...")`.
- A second bridge with the same shape exists in the interactive/Scheme dispatch path (see
  `crates/mae/src/ai_event_handler.rs`'s `dispatch_command_by_name` call site, which builds an
  `ExecuteResult::Immediate` with `success: true` unconditionally once the name resolves).

This is the same ADR-086 defect class (refusal reported as success) as the ~15 findings that
commit fixed — but an order of magnitude larger in surface area, because it's the generic fallback
behind the entire `command_*` tool namespace rather than 15 individually-named tools. ADR-086's own
commit explicitly deferred it for this reason: "The class is far larger than the 15 sampled findings,
and the fix needs an architectural call rather than a guess."

**The call:** what does "success" mean for a `command_*` MCP tool? Options considered:
1. Change `dispatch_builtin`'s signature to return a richer outcome (e.g. an enum distinguishing
   "unknown", "routed and something changed", "routed but no-op") — correct, but touches every one of
   the ~559 command implementations to determine which arm applies, a large mechanical pass.
2. Leave `dispatch_builtin` as a pure "was this name routed" signal, and push postcondition-checking
   down to individual command implementations that already know whether they changed anything
   (mirrors how ADR-086's ~15 fixes worked — each fix lived at the specific operation, not at the
   generic dispatcher). Cheaper per-command, but means the generic bridge can never fully close this
   on its own; it only helps for commands someone deliberately hardens.
3. Accept the current "recognised = success" semantics for `command_*` specifically, document it
   plainly (the MCP tool description should say "reports whether the command name was valid and
   routed, not whether it changed anything measurable"), and rely on the AI checking buffer/editor
   state afterward if it needs to confirm effect.

**My recommendation:** option 3 as an immediate, honest stopgap (a one-line doc-string change, ships
today), with option 2 as the real fix — applied opportunistically whenever a specific command is
touched for other reasons, rather than as one large mechanical PR. Option 1 is the "correct" answer
but is a genuinely large, separate initiative and shouldn't block v0.15.

---

## 6. KB sharing/membership MCP tools are Write tier — that is an authorization change

**Status: IMPLEMENTED.** Raised to `Privileged`, plus three siblings by the same criterion
(`kb_share_p2p`, `kb_unblock_member`, `kb_set_encryption`); `kb_join`/`kb_join_p2p`/`kb_leave`/
`kb_block_member`/`kb_set_role` deliberately left at `Write` with per-entry reasoning. The
criterion and both lists live in `crates/ai/src/tools/authorization.rs`, which is also the single
source consulted by the tool table, `classify_command_permission` (the `command_kb_share` mirror —
a Write-tier path to the same effect, needing no arguments), and `effective_tier` (the
`execute_command` passthrough). The related `set_option`/`ai_tier` self-escalation is closed by the
same argument-sensitive escalation; the Scheme side is pinned in
`crates/scheme/src/permission_option_tests.rs`.

Original analysis below.

**Status (at the time):** found while classifying the Scheme surface (ADR-084 D3). Not changed; needs your call.

Classifying all 516 Scheme primitives forced a per-effect judgment, and comparing the result against
the MCP tool table surfaced a mismatch. These tools are declared `PermissionTier::Write`
(`crates/ai/src/tools/kb_tools.rs`):

- `kb_share`, `kb_add_member`, `kb_remove_member`, `kb_approve` — grant or revoke another
  principal's access to a shared KB.
- `kb_set_policy` — change the join policy (restrictive / invite / permissive).
- `kb_set_ai_residency` — relax the ADR-048 residency restriction on a KB.

Granting a third party access to your knowledge base is not an edit. A `write`-tier session — the
tier an operator would pick precisely to allow buffer edits while withholding shell access — can
today add a member to a shared KB, or open a restricted KB to AI residency.

The equivalent Scheme primitives were classified `Privileged` in the D3 pass. Scheme being stricter
than MCP for the same effect is not a bypass, but it is a sign the tool table is the one that is
wrong.

**The call:** raise these to `Privileged`. It is a behaviour change for any workflow driving KB
sharing from a write-tier session, which is why I have not done it unilaterally.

**My recommendation:** raise them. ADR-018's role model already treats membership as an
owner-only operation; the tier table should agree with it.

**Related, same shape:** `set_option` is Write. That is harmless today because `ai_tier` does not
reach the enforced policy — but ADR-084 D7 changes exactly that, at which point a write-tier session
could raise its own tier through `set_option`. Whichever way #1 is decided, `set_option` needs to
refuse the permission-tier option specifically, or be raised.

---

## 7. Two ADR-087 follow-ups, both flagged rather than silently skipped

**Status:** rules 1/2/3/7 landed. These two are the honest remainder.

**(a) A third per-char width implementation survives.** `crates/core/src/wrap.rs`'s `char_width` /
`slice_display_width` still do `ch.width().unwrap_or(0)` — the same per-char summation deleted from
`text_utils`. It is *not* a Rule 7 violation (it lives inside `mae-core`), and word-wrap genuinely
needs per-char increments over `&[char]`, so it structurally cannot use grapheme-cluster width. But
it does not honour the new `WidthPolicy`, so the ambiguous-width and control-char options do not
reach word wrap. Follow-up: route it through `char_width_with`.

**(b) The width options are registered but only threaded into the status bar.** `ambiguous_width`
and `control_char_width` are real options — registered, `:set`/Scheme-accessible, verified to change
computed width — but only `render_common/status.rs` (TUI and GUI) consumes them. Which-key, popups
and the other truncation call sites still use the hardcoded default policy. Threading the policy
through every renderer entry point means changing many signatures across `mae-renderer`/`mae-gui`;
that is the natural next increment, and it is why an East-Asian user setting `ambiguous_width=wide`
would see it take effect in the status bar and nowhere else today.

Neither is a correctness regression — both are "the fix is real but narrower than the option
implies." Worth knowing before the option is documented as global.

---

## 8. RESOLVED — `checked_byte_boundary` asserted on its own normal case

**Fixed in `51158578`** after this entry was filed. Split into
`floor_char_boundary` (clamp a byte budget, no assertion — mid-character is the expected
case) and `checked_byte_boundary` (validate an offset that should already be valid, keeps
the debug_assert). 16 call sites repointed. Original analysis below.

### Original entry

**Status:** found while adding an adversarial test for #604.2/#604.6 (pre-v0.15 audit tail pass).
Not changed; needs a call from whoever owns ADR-087's chokepoint validator.

`mae_core::grapheme::checked_byte_boundary` (`crates/core/src/grapheme.rs:192-214`) is the shared
ADR-087 chokepoint: any call site that needs to slice a string at a byte offset that *might* not be a
char boundary is supposed to route through it instead of raw indexing. Its actual behaviour, though:

```rust
if byte_idx <= s.len() && s.is_char_boundary(byte_idx) {
    return byte_idx;
}
debug_assert!(false, "checked_byte_boundary: offset {byte_idx} is not a valid char boundary ...");
// clamp and log, only reached when debug_assertions is off
```

This is by design per ADR-087 Rule 5 ("clamps and logs in release") — but the corollary is that it
**panics in every debug build** (`cargo test`, `cargo run` without `--release`, any contributor's
local dev binary) the moment it's asked to clamp a genuinely non-boundary offset. For a fixed-length
truncation of arbitrary content (`&content[..8000]`, `&stdout[..10_000]`, `&code[..200]` — exactly
the shape of every "ADR-087-fixed" call site: `guidance.rs`, `run_loop.rs`, `handle_prompt.rs`,
`shell_exec.rs`, and now `runtime.rs`'s `record_error`), landing mid-character at the cut point is not
a bug, it's the expected steady state for real non-ASCII input. So the validator's debug/test-mode
behaviour is: abort the process on the exact input its release-mode behaviour was built to survive.

**Verified this affects every "fixed" site equally, and none of them have a test that would catch
it**: `guidance.rs`'s own regression test constructs its fixture as `"x".repeat(PROJECT_CONTEXT_MAX_CHARS
+ 500)` — all-ASCII filler, so it can never land off-boundary and never exercises the debug_assert.
The same is true of every other site's tests (or absence of one). This is not a hypothetical: while
writing an adversarial test for #604.2, constructing a *real* mid-character trigger at the fixed cut
length caused an immediate `debug_assert` panic — in effect, an unwritten test would have been the
first thing to reproduce the same debug-build crash that a real user's non-ASCII content could
trigger during local development.

**The call:** the validator conflates two different use cases under one name:
1. **True chokepoint / "should never happen" positions** — e.g. an LSP `Position` that MAE itself
   computed and is passing back into a slice; landing off-boundary here really would indicate an
   upstream bug, and panicking loudly in debug is the right, ADR-087-intended behaviour.
2. **Flat truncation of arbitrary, untrusted-length content** — landing off-boundary is the *expected*
   outcome for real non-ASCII input at an arbitrary fixed cut length, not a bug signal.

Options:
1. Split into two functions: keep `checked_byte_boundary`'s debug-panic behaviour for case 1, and add
   a silently-clamping sibling (no `debug_assert`) for case 2 — every currently-"fixed" truncation
   call site would switch to the new sibling.
2. Leave it as one function, and accept that dev/debug builds may abort on this path — since release
   builds (what ships) are unaffected, and treat any debug-build abort here as a signal to add an
   adversarial test with a *boundary-respecting* fixture instead (the workaround this pass used for
   #604.2's own test, documented in its doc comment).
3. Change `debug_assert!` to a `tracing::debug!` + always-clamp, dropping the loud-in-dev behaviour
   entirely — simplest, but gives up the "catch it in CI" property ADR-087 explicitly wanted.

**My recommendation:** option 1. The two use cases have different correct behaviour and conflating
them under one name is exactly the kind of "one function, two jobs" shape CLAUDE.md principle #8
warns against once you notice it — a debug_assert that fires on expected, common input isn't a bug
detector at that point, it's a landmine sitting in every contributor's local dev build, and it will
eventually be hit by someone typing genuinely accented text into a project they're testing MAE on.

---

## 9. Nine AI tools are advertised over MCP but structurally undispatchable there

**Status: IMPLEMENTED — the ambitious option (option 1), designed in ADR-091.** `dispatch_tool`
gained a session handle (`Editor::agent_session_mut`, resolved from the MCP session id
`with_ai_dispatch_scope_for_session` now records). Six are genuinely dispatchable
(`crates/ai/src/executor/session_exec.rs`); the three inherently-interactive ones are withheld from
every external discovery surface via one shared filter. A registry-driven invariant
(`no_advertised_tool_is_unroutable`) now fails the build if anything advertised to an external
client is unroutable — the check whose absence let this exist.

Original analysis below.

**Status (at the time):** found while triaging issue #590 (pre-v0.15 audit tail pass). Not changed; needs a call.

`ask_user`, `delegate`, `ai_set_mode`, `ai_set_profile`, `ai_set_budget`, `propose_changes`,
`log_activity`, `read_transcript`, and `web_fetch` are all registered in `ai_specific_tools`
(`crates/ai/src/tools/mod.rs`) and therefore appear in an external MCP client's `tools/list` —
`ask_user` specifically at the default Core tier, so it's in the *first* `tools/list` any paired
external agent (VS Code Copilot, Claude Code via the shim — v0.15's headline use case) sees.

None of the nine are reachable through `crates/ai/src/executor/tool_dispatch.rs::dispatch_tool`,
confirmed by direct inspection — none of the nine names appear anywhere in that function's dispatcher
chain, so an external MCP call for any of them falls through to `Err("Unknown tool: {name}")`. All
nine are handled *only* inside the embedded `AgentSession`'s own event loop
(`crates/ai/src/session/handle_prompt.rs`), which has session-scoped state `dispatch_tool` structurally
cannot see: `self.transcript_path`, `self.budget`, `self.current_mode`/`self.current_profile`, and —
for `ask_user`/`propose_changes` — a `tokio::sync::oneshot` channel that pauses the session's own task
waiting for a human UI reply. `dispatch_tool`'s signature (`editor: &mut Editor, call: &ToolCall,
requester_provider: Option<&str>`) has no session handle at all.

**The call:** this genuinely needs an architectural decision, not a quick patch, because the nine
tools split into two different problems:
- `ai_set_mode`/`ai_set_profile`/`ai_set_budget`/`log_activity`/`read_transcript`/`web_fetch` are not
  *inherently* interactive — they mutate or read session-local state, which could in principle be
  threaded through if `dispatch_tool` gained a session handle (a real feature: per-session state
  reachable from the MCP dispatch path, which doesn't exist today for anything).
- `ask_user`/`propose_changes`/`delegate` are inherently interactive or spawn sub-agents; making them
  reachable over MCP means deciding what "pause and wait for a human reply" even means for an
  external client mid-`tools/call` — a UX question, not just a wiring one.

Options:
1. Build real MCP-facing dispatch for the six non-inherently-interactive tools (the smaller, more
   tractable half), and explicitly exclude the three interactive ones from external MCP tool discovery
   until/unless a real answer exists for what "ask the human" means over MCP.
2. Exclude all nine from every external-MCP discovery surface (`tools/list`, `search_tools`,
   `request_tools`) — matching ADR-085's own stated shape ("the fix is that they are not offered, not
   that they are offered and then refused") — and keep them embedded-session-only, full stop, until a
   design exists.
3. Leave as-is and treat the `Unknown tool` response as an acceptable outcome for a tool an external
   client shouldn't have called in the first place, documenting it in each tool's description.

**My recommendation:** option 2 as an immediate fix (cheap — a discovery-surface filter, touches no
dispatch logic, and matches the precedent ADR-085 already established for a structurally-identical
"advertised but not actually reachable this way" gap), with option 1 as a real follow-up feature once
someone decides the UX for interactive tools over MCP. Option 3 leaves a paired external agent
discovering this by trial and error, which is a worse experience than not offering the tool.

---

## 10. `KbStore::fts_search` silently drops terms

**Independently reproduced (2026-08-03).** A node titled `Quantum Physics` with body
`Entanglement is spooky.`:

```
quantum      -> 1 hit        physics      -> 0 hits
spooky       -> 1 hit        entanglement -> 0 hits
```

Terms plainly present in the indexed text return nothing. This is not a ranking quirk — the
node is simply not found. For a knowledge-base editor whose whole value is retrieval, a search
that silently misses is worse than one that errors.

`shared/kb/src/cozo_store/tests/kb_store_impl_tests.rs::fts_search_finds_nodes` passes only
because it queries `quantum`, one of the terms that happens to work — the "unicorn value"
antipattern principle #14 names, and the reason this survived.

Pre-existing and unrelated to the parity work that found it. Not fixed here: diagnosing the
tokenisation/stemming behaviour in the Tantivy index is its own investigation, and guessing at
it would risk changing ranking for every existing query.

**The call:** this probably deserves to jump the queue — it is user-facing, silent, and affects
the primary KB read path (`kb_search`, `kb_search_context`, and every consumer of them).

**Recommendation:** a dedicated investigation before v0.15, with a property test over a
multi-term corpus asserting every term in a node's title and body retrieves that node.

### RESOLVED — root cause found and fixed

Neither tokenisation nor stemming: **the separator never reached the index.** The extractor was
`title ++ ' ' ++ body`, but CozoDB does not persist an FTS extractor as written — it parses it,
partial-evaluates it, and stores `expr.to_string()` (`cozo/src/parse/sys.rs`), where
`Display for DataValue` renders string constants with Rust's `{:?}`, i.e. always *double* quotes.
That stored text is re-parsed per indexed row by `cozoscript.pest`, in which
`quoted_string_inner = { char* }` is a **non-atomic** rule — pest skips implicit `WHITESPACE`
between `char` repetitions, so a double-quoted all-whitespace literal matches zero chars and
parses back as `""`.

The index was therefore built from `title ++ body`, welding the last title token onto the first
body token. `"Quantum Physics"` + `"Entanglement is spooky."` indexed `quantum`,
`physicsentanglement`, `is`, `spooky` — which is exactly the reported hit/miss split, and why it
looked arbitrary. Fixed by using `'.'`, which survives the round trip and is a hard token
boundary for the `Simple` tokenizer (splits on `!c.is_alphanumeric()`). An escape would not
work: cozoscript does not unescape, so `'\n'` returns as a literal backslash + `n`.

**Ranking:** unchanged in shape. Scoring is `tf * idf` over the same relation with the same
tokenizer and filters; no weights, boosts or `k` were touched. Previously-correct hits keep
their order relative to one another. What changes is that boundary terms now match at all, and
the bogus welded tokens (`physicsentanglement` — never a real query) are gone.

**Migration:** required, and automatic. A CozoDB FTS index is populated at `::fts create` and
maintained incrementally, so the DDL change alone would only ever have helped brand-new KBs.
`ensure_fts_index_current` stamps `instance_meta.fts_extractor_version` and rebuilds once, on
open, for any store carrying an older stamp. No user action.

Two further defects found by the property test that replaced the unicorn assertion:

1. **`fts_search`'s own post-query verification** (MAE code, not cozo) split the raw query on
   whitespace and required a literal substring match, so a prefix query like `buffer*` produced
   the term `buffer*`, which no document text contains — every candidate the index correctly
   returned was discarded. Same silent-miss direction. Fixed alongside.
2. **Query-side parse errors, NOT fixed** — see below.

### Still open — the FTS *query* surface

Distinct from the above and deliberately left alone. Cozo's FTS query grammar treats `:`, `-`,
`.` and a leading `*` as operators, and reserves uppercase `AND`/`OR`/`NOT`/`NEAR` via a
lookahead with **no word-boundary anchor**. Consequences:

- `concept:buffer` (a namespaced node id — the most natural thing to search this KB for),
  `read-only`, `1.5`, `-buffer` are hard parse errors.
- Any ALL-CAPS term merely *beginning* with a keyword is a parse error: `ANDROID`, `ORBIT`,
  `NEARBY`, `ANDES`, `NOTES`, `ORDERING`. Their lower/title-case forms are fine, so this is
  purely a query-grammar artifact — the index holds those terms correctly.

These error loudly rather than missing silently, which is why they are lower severity than the
defect above. Both classes are pinned by tests
(`fts_query_syntax_characters_are_still_parse_errors`,
`fts_uppercase_boolean_keyword_prefixes_are_parse_errors`) so a future fix must update them
consciously.

**The call needed:** suppressing this means deciding whether MAE exposes cozo's boolean query
language to users at all — lower-casing or escaping the query would make a deliberate
`foo AND bar` mean the literal word "and". That is a product decision, not a bug fix, and it is
the remaining reason `(kb-search …)` is built on `KnowledgeBase::search_ranked` rather than
`fts_search`.


---

## 12. `sandbox_guard` protects only one of `shell_exec`'s two implementations

**Status:** confirmed against current source while fixing audit #590.3. Not fixed — the fix lands
in `crates/ai/src/executor/tool_dispatch.rs`, which the concurrent ADR-090 three-state permission
work owns, so editing it underneath that workstream would conflict.

`shell_exec` has two implementations, and they exist for a real reason: the embedded `AgentSession`
runs commands on tokio (`session/run_loop.rs`), while MCP and other non-session callers run them
synchronously (`executor/shell_exec.rs`) because `dispatch_tool` holds a `!Send` `&mut Editor`.

The *blocklist* was copy-pasted between them; #590.3's fix consolidated that into
`crate::shell_policy`, so refusal rules and timeouts can no longer drift. **Sandbox confinement did
not get the same treatment.** `sandbox_guard` (`tool_dispatch.rs:896`, called at `:610`) is applied
on the dispatch path only. The embedded session's `execute_shell` is reached through the session's
own event loop, not through `dispatch_tool`, so a command run there is not confined to the sandbox
directory.

**Why this needs a decision rather than a patch.** Whether it is currently *exploitable* depends on
facts the permission workstream owns:

- `ai_chat_enabled` defaults to **off** (ADR-049), so the embedded session is not the default
  surface. If it stays off-by-default and is genuinely frozen at its current feature set (ADR-046),
  this is a latent asymmetry rather than a live hole.
- ADR-090's three-state model changes what "the user approved a shell command" means. Threading
  sandbox confinement through the session path is a different amount of work depending on where
  that lands.

**Options**

1. **Move `sandbox_guard` into `shell_policy`** alongside the blocklist and call it from both
   implementations. Consistent with the consolidation just done, and the smallest conceptual change
   — but `sandbox_guard` inspects tool *arguments* generically (not just `shell_exec`), so it is
   not a clean fit for a shell-specific module without either splitting it or widening that module's
   remit.
2. **Route the embedded session's tool calls through `dispatch_tool`** so there is one enforcement
   point rather than two. Correct in principle; a real refactor of the session loop, and ADR-046
   explicitly froze that surface.
3. **Declare the embedded session out of scope for sandboxing**, document it in SECURITY.md next to
   the existing "the shell blocklist is not a sandbox" language, and gate the session behind an
   explicit opt-in that says so.

**My recommendation: option 3 for v0.15, with option 1 as the follow-up.** The embedded chat is
already off by default and frozen; adding a second enforcement path to a surface being wound down
buys little, whereas an honest SECURITY.md sentence closes the *documentation* gap immediately.
Option 1 becomes worthwhile the moment anything else grows a second dispatch path — at which point
the enforcement point, not just the rule table, should be the shared thing.

**What I did do:** the commit for #590.3 names this gap explicitly so it is not lost, and
`shell_policy`'s module doc carries an `@ai-caution` saying policy belongs in one place.

---

## 13. When does the signed membership op-log become authoritative?

**Status:** surfaced while fixing audit #589.4. The observable false-success is fixed; the
underlying question is not mine to answer.

`append_signed_membership` (`daemon/src/collab_handler/mod.rs`) mirrors every membership mutation
into the ADR-026 signed, hash-chained op-log — the record peers verify **without trusting the
relay**. Its doc comment says the failure path is deliberately non-fatal because "the legacy
`member_roles` map remains authoritative until `kb_access` switches to derived membership (slice
2b-6c)".

That slice has not landed. Two consequences are live today:

- **#589.2** — the `*KB Sharing*` buffer builds its displayed roster from unsigned
  `coll.member_roles()` rather than `derive_valid_members_governed`. What the owner *sees* is
  therefore the unsigned view, not the verifiable one.
- **#589.4** — a signing/persist failure left the two records diverged with nobody told. I fixed
  the reporting (`kb/set_governance` now errors, since the append is its only effect;
  `kb/add_member`/`kb/approve_member` return `signed_oplog: false` plus a warning, since the legacy
  mutation genuinely did land). I did **not** change which record is authoritative.

**The call:** does the legacy `member_roles` map remain the source of truth through v0.15, or does
derived membership take over — and if it does, what should a failed signed append do then?

**Options**

1. **Keep legacy authoritative through v0.15.** Then today's behaviour is correct-by-design and the
   two findings reduce to: display the derived view in the sharing buffer (#589.2) so the owner is
   at least *looking* at the verifiable record, and keep the divergence warning I added.
2. **Switch `kb_access` to derived membership now.** Then a failed signed append is no longer a
   bookkeeping divergence — it is a membership change that did not happen, and every caller should
   fail closed the way `kb/set_governance` now does. This is the ADR-026 end state.
3. **Dual-read with a mismatch alarm** — enforce on legacy, but compute derived alongside and raise
   an ADR-024 notification whenever they disagree. A migration aid rather than a destination.

**My recommendation: option 1 for v0.15, plus #589.2's display fix, with option 3 as the bridge.**
Option 2 is the right destination but changes the failure mode of every membership RPC at once,
which is a poor thing to ship in the same release as ADR-090's permission changes. Option 3 is
cheap, and the mismatch alarm is exactly the evidence needed to know whether option 2 is safe —
right now nobody can say how often the two records actually diverge in practice, which is itself
the reason this is a decision and not a fix.

---

## 14. What should `babel-execute-all` do with blocks that need confirmation?

**Status:** audit #596.1 is fixed; this is about the behaviour I chose, which is defensible but not
obviously the only right answer.

`babel-execute-all` ignored `effective_eval_policy` entirely, testing only `:eval never`. So a
`:eval query` block — which single-block `babel-execute` refuses without a human answer — ran
unprompted, as did any default block in a file outside `babel_trust_paths` with `babel_confirm` on.
"Execute all" being *looser* than "execute one" is backwards, and it is precisely the command a
hostile org file wants you to run.

I made it consult the same gate, **skipping** blocks that need confirmation and reporting a count.
One command cannot open N sequential dialogs, and skipping is the conservative direction.

**The call:** is skip-and-count the right UX, or should the command be able to ask?

**Options**

1. **Skip and count (what I shipped).** Safe and simple. The cost: on a document where most blocks
   need confirmation, `babel-execute-all` becomes a no-op that tells you to go run each block by
   hand — which is a real usability regression against the (unsafe) previous behaviour.
2. **One batch prompt** — "3 blocks require confirmation. Execute them? (y/n/list)". Matches how
   users think about the command. Needs a new mini-dialog context that carries a block *set* rather
   than the single `MiniDialogContext::BabelConfirm` block that exists today, and it weakens the
   per-block granularity `:eval query` was presumably meant to express.
3. **Queue the dialogs** — confirm each in turn. Preserves per-block granularity exactly; needs a
   dialog queue the mini-dialog system does not have, and is tedious on a large document.
4. **Skip, but make it trivially recoverable** — option 1 plus a `babel-execute-all-confirmed`
   variant, or a `babel_execute_all_prompt` option choosing between 1 and 2.

**My recommendation: keep option 1 for v0.15, and take option 4 if anyone hits the no-op case.**
The unsafe behaviour needed to stop now; the *right* prompt UX is worth choosing with a real user
complaint in hand rather than speculatively. Worth noting that `babel_trust_paths` — dead config
until this same pass registered it (#596.5) — is the intended escape hatch: a user who trusts their
own org directory adds it once and never sees the skip. That may make the no-op case rare enough
that nothing further is needed.
