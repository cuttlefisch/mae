# Decisions parked for human review

Working notes from the pre-v0.15 remediation effort. Each entry is a judgment call that
genuinely needs Hayden's input — not something prior art can settle, and not something with an
obvious default. Work continued around each one; nothing here is blocking.

Delete an entry once it is decided (and record the decision in the relevant ADR).

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

## 6. KB sharing/membership MCP tools are Write tier — that is an authorization change

**Status:** found while classifying the Scheme surface (ADR-084 D3). Not changed; needs your call.

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
