# ADR-084: AI permission tiers are enforced at the effect, against a compiler-proven allow-list

**Status:** Accepted. **Revised 2026-08-02** following an external prior-art review; the revision
reverses this ADR's original Decision 2 (see *Revision note*).
**Extends:** ADR-051 (per-session `PermissionPolicy` — this ADR fixes *where* that policy is
consulted, without changing how a session declares one).
**Relates to:** ADR-049 (`mae-agent` as the default AI surface — the embedded session this ADR brings
under the policy), ADR-085 (`ToolCategory` describes subject matter, not blast radius — the sibling
axis), ADR-056 (session-scoped category dispatch, whose guarantees depend on this one holding),
ADR-088 (carried authority — the successor this ADR defers to).
**Tracking:** issue #592 (pre-v0.15 audit epic). Found during that audit and fixed in v0.15
before any release shipped the defect to users; the draft advisory opened while the fix was
in progress was closed unpublished rather than disclosed, as no released version was affected.

## Revision note

The first version of this ADR decided to gate **only** the one Scheme primitive that spawns a process,
keeping `eval_scheme` at `Write` tier, and described this as "a deny-list of one, not a classification
of the whole API." A prior-art review found that reasoning does not survive contact with the evidence.
The enforcement *point* was right; the deny-list *shape* was wrong, and two production runtimes have
already retreated from exactly the architecture it proposed. Decision 3 below replaces it. The
superseded reasoning is preserved under *Alternatives considered* so the record shows what was tried.

## Context

MAE advertises four AI permission tiers — `readonly`, `write`, `shell`, `privileged` — as the control
that bounds what an AI agent may do. `SECURITY.md` described them as "enforced before every tool
execution with no bypass vectors."

A pre-v0.15 audit established that the tier is consulted at exactly **one** place:
`crates/ai/src/executor/tool_dispatch.rs:98,114`, on the MCP tool-dispatch path. Three consequences
follow, each independently verified and each reachable without anything exotic:

1. The embedded `AgentSession` — the `:ai` surface and `delegate()` sub-agents — carries no
   `PermissionPolicy` at all, so it never consults one.
2. The `ai_tier` option updates a status-bar string. The value that is actually enforced comes from a
   different source with no write path between them.
3. `eval_scheme` is `Write` tier, and the Scheme runtime can reach process execution. A session capped at
   `write` therefore reaches shell — including transitively, since some editor commands enqueue Scheme
   that does so.

Two further defects were found in the same mechanism and are fixed here rather than separately:
the resolver's default value (`"trusted"`) and its unknown-value fallback **both** resolve to `Shell`,
the most permissive non-privileged tier (`crates/mae/src/config.rs:697-714`), and
`allowed_categories` defaults to `None`. The shipped posture is therefore *all categories × shell tier*.

The common shape is that **the tier is checked against a tool's name at one gate, while the effect the
tier exists to prevent is reachable through several other doors.** Gating names does not bound effects.

This matters beyond the editor: v0.15 ships MAE as a headless MCP backend for external editors, and that
initiative's own premise (ADR-051, ADR-056) is that the server-side policy is the only real boundary,
because a client's "always allow" is not one. The MCP specification is normatively on that side — for
servers, "Implement proper access controls" is a **MUST**, while client-side confirmation is only a
**SHOULD** ([spec/server/tools, Security Considerations](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)).

## Decision

**Enforcement is applied at the effect, decided in one place, and driven by an allow-list whose
completeness the compiler proves.**

1. **One decision point, many enforcement points (PDP/PEP).** Following NIST SP 800-162's split, the
   tier logic lives in a single Policy Decision Point; enforcement points sit at effects and only ask
   it. This satisfies complete mediation without multiplying the *decision* logic that must be
   reviewed — the two Saltzer & Schroeder principles that otherwise pull against each other here.

2. **Every entry point converges on the PDP.** `AgentSession` gains a `PermissionPolicy`, and the
   embedded/`delegate()` path consults the same check the MCP path does. There must be no
   tool-dispatching entry point that can reach an effect without passing it.

3. **The Scheme surface is an allow-list, and `rustc` proves it complete.** Every primitive declares a
   required tier at registration:
   - `ForeignFn` (`crates/scheme/src/value.rs:321`) gains a required `tier: PermissionTier` field.
   - `SchemeVm::register_fn` (`crates/scheme/src/vm.rs:304`) takes it as a **required argument**, so
     every one of the ~212 registration sites fails to compile until it declares one.
   - The check happens at `crates/scheme/src/vm.rs:1362` — the single site where any `ForeignFn` is
     ever invoked. This is MAE's `fget`: verified to be the only invocation path.

   This is Capsicum's technique verbatim — *"Changing the signature of `fget` allows us to use the
   compiler to detect missed code paths, providing greater assurance that all cases have been
   handled."* An unclassified primitive is a build failure, not a silent gap. A new primitive that
   spawns a process cannot be added without declaring a tier for it.

4. **Absent and unrecognised tier values both fail closed.** An unknown tier string is **rejected at
   config load** — naming the bad value, listing the valid ones, exiting non-zero — rather than
   resolved to anything. `make check-config` already exists, which makes a strict parser
   operationally safe (the sshd/nginx pattern: strict loader plus offline validator). Where a runtime
   path cannot refuse, it resolves to the *most restrictive* tier and warns loudly. The absent-value
   default changes from `"trusted"` to the most restrictive tier that keeps the editor usable.

   The current behaviour is the literal text of [CWE-636](https://cwe.mitre.org/data/definitions/636.html)
   ("using the most permissive access control restrictions"). It is also internally inconsistent: an
   unknown *category* already fails closed (`knowledge_only_denies_uncategorized_tools_fail_closed`)
   while an unknown *tier* fails open. The realistic source of an unrecognised tier is a typo in a
   local config authored by the same human running the binary — not version skew — so the
   forward-compatibility argument for leniency does not apply.

5. **A tier denial aborts the evaluation; it does not return an ignorable error.** Garfinkel's
   NDSS 2003 survey documents that denying a call mid-execution leaves callers running in a
   half-applied state, and that callers frequently ignore the failure — his recommendation for
   privilege-relevant denials is to abort rather than return. A Scheme program denied at an effect has
   typically already mutated buffers and files; continuing is worse than stopping. This is a
   user-visible behaviour change and belongs in release notes.

6. **`eval_scheme`'s tier is a calling requirement, not a containment claim.** Evaluated Scheme runs
   against whatever tier is ambient; the tool's own tier says only who may *invoke* the evaluator. This
   is stated explicitly in the tool description and in `SECURITY.md`, following Deno's precedent of
   documenting the limit bluntly rather than letting the label imply containment.

7. **`ai_tier` either reaches the enforced policy or ceases to exist.** An option that is registered,
   `:set-save`-persistable and rendered in the status bar, but which changes nothing, is worse than no
   option — it actively misinforms. Principle #7 requires user-visible behaviour to be genuinely
   configurable; a decorative control is a violation of it, not a partial implementation.

## What this does *not* fix — accepted, tracked debt

Stated explicitly, because leaving them unstated is how the original version of this ADR overclaimed.

- **The tier is ambient authority, not carried authority.** The check answers "is this session allowed
  to spawn?" It cannot answer "did the argument originate from the user or from a cloned repository's
  README?" — and only the second is the confused-deputy question this ADR's own threat model names.
  Hardy's 1988 paper documents the ambient "switch hats" fix as the one that did not generalise.
  **ADR-088** designs the provenance-carrying successor (CaMeL is the worked example); this ADR is the
  precondition for it, not a substitute.
- **Composition is not bounded.** JEP 411's postmortem states the general form: permissions "allow …
  a series of safe operations whose overall effect is sufficiently unsafe that it would require a more
  powerful permission if granted directly." A write-tier agent can write a file into a watched
  directory, edit a `Makefile`, or create `.git/hooks/*`, then trigger it. Per-effect tiers do not see
  chains.
- **One mutable Scheme image is an escalation channel, and principle #6 makes it worse.** Runtime
  redefinability is sacred in MAE, which means write-tier Scheme can redefine a function that
  privileged Scheme later calls. PostgreSQL solves the equivalent problem by running trusted and
  untrusted PL in *separate interpreter instances* and *still* ships a warning that the mechanism may
  not hold. MAE does not separate them. Recorded as `@ai-caution: [architecture-debt]` at the VM, not
  silently accepted.
- **Tiers bound; they do not prevent.** Symlink escapes, path-canonicalisation bypasses, and
  exfiltration via an allowlisted binary all stay *within* a granted tier. The consensus position is
  tiers plus sandboxing; this ADR delivers the first only.

## Consequences

**Positive**

- The tier becomes a property of the system rather than of one code path, so a new entry point cannot
  silently escape it — new surfaces converge on the same check by construction.
- Completeness stops being a maintenance burden and becomes a compile error, which is the specific
  failure that JEP 411 and .NET CAS both cited when retreating.
- Fail-closed parsing removes a whole family of "typo widens access" bugs, not just the two found.
- `SECURITY.md`'s claim becomes defensible — and, where it is still not true, this ADR says so.

**Negative / Risks**

- ~212 registration sites must each declare a tier. Mechanical, one-time, and compiler-guided, but it
  is a genuinely larger change than the original deny-list, and it touches every `runtime/*.rs` module.
- Gating at the effect means refusal surfaces later than a name-based gate would. The error must say
  plainly which tier was required and which was in force, or it will read as a malfunction.
- Rejecting unknown tiers at load turns a previously-silent typo into a failed startup. This is the
  intended trade, and `make check-config` exists to catch it before launch.

## Enforcement

- A test that iterates every registered tool and asserts its declared `PermissionTier` is honoured on
  **every** entry path — embedded session, MCP dispatch, `execute_command`, and `eval_scheme` — not only
  on the MCP one, following the precedent of `crates/ai/src/executor/mod_tests.rs:1418` and
  `every_registered_option_is_reachable_via_get_option`: registry-driven, so it cannot fall behind
  what ships.
- A test asserting a process-spawning primitive is refused below the required tier, including via the
  transitive command-enqueues-Scheme route.
- A test asserting an unknown tier string is rejected rather than resolved, and that the absent-value
  default is not the most permissive tier.
- The allow-list needs no completeness test: `register_fn`'s signature is the test.

## Alternatives considered

**Gate only the process-spawning primitive, keeping `eval_scheme` at `Write` (this ADR's original
Decision 2).** Rejected on review. Deno documents that this is not achievable in principle — *"All code
executing on the same thread shares the same privilege level"* and *"It is not possible for different
modules to have different privilege levels within the same thread"* — so a tier label on an eval tool
cannot bound what evaluated code reaches. Saltzer & Schroeder's fail-safe-defaults principle names the
shape directly: a mechanism that "attempts to identify conditions under which access should be refused"
fails by *allowing* access, "a failure which may go unnoticed in normal use." And two vendors abandoned
production systems built on scattered per-effect checks against ambient authority — JEP 411 ("There is
no way to have partial security"; "an ongoing maintenance burden") and .NET CAS, which Microsoft
explicitly de-supported as a security boundary. The original argument that full classification was
"disproportionate" assumed a human would do the classifying; making it a required argument means the
compiler does.

**Raise `eval_scheme` to `Shell`.** Still rejected, and for the original reason: it makes a `write`-tier
session unable to evaluate any Scheme at all, including pure expressions, and it does not address the
transitive route. Under Decision 3 it is also unnecessary — per-primitive tiers give a finer answer than
reclassifying the evaluator.

**Leave the embedded session ungated on the grounds that the human invoked it.** Rejected, and the
evidence is stronger than the original reasoning claimed. No surveyed production coding agent keys
gating to session origin; two key it the *opposite* way — OpenHands gates every action in the
interactive CLI while `--headless` is the one hard exemption, and Claude Code disables trust
verification only under non-interactive `-p`. Human presence buys more checks, not fewer. Note the
justification should rest on *deterministic machine-enforced mediation*, not on human review: Anthropic
reports users approve 93% of permission prompts, so an argument from human vigilance would undercut
itself.

## Evidence

Prior-art review, 2026-08-02. Full report:
`docs/research/084-enforcement-placement-prior-art.md`.

- Saltzer & Schroeder (1975), *The Protection of Information in Computer Systems* — complete mediation,
  fail-safe defaults, economy of mechanism, least privilege.
- Watson et al., *Capsicum: practical capabilities for UNIX*, USENIX Security 2010 — enforcement at
  `namei`/`fget`; compiler-proven completeness via signature change; allow-list policy shape.
- Wright et al., *Linux Security Modules*, USENIX Security 2002 §3, §4.5 — the named interface "may not
  adequately express the full context needed"; mediate objects, not names.
- Felt et al., *Android Permissions Demystified*, ACM CCS 2011 §2.2.1 — client-side checks "cannot be
  relied upon"; the auditability cost of diffuse mediation.
- Hardy (1988), *The Confused Deputy*; Miller, Yee & Shapiro (2003), *Capability Myths Demolished* —
  ambient authority defined; why the ambient fix did not generalise.
- JEP 411 (OpenJDK) and Microsoft's de-support of .NET Code Access Security — two retreats from this
  ADR's originally-proposed architecture.
- Deno security documentation — same-thread privilege uniformity; eval cannot be contained by label.
- PostgreSQL *Trusted and Untrusted PL/Perl* — capability-category restriction plus interpreter
  separation, and an explicit hedge that it may not hold.
- Garfinkel, *Traps and Pitfalls*, NDSS 2003 §4.5 — side effects of denying calls mid-execution.
- NIST SP 800-162 — PDP/PEP separation.
- CWE-636; Google, *Building Secure and Reliable Systems* ch. 8 — "Security-critical operations should
  not fail open."
- MCP specification (2026-07-28), server/tools Security Considerations — server access control is
  MUST, client confirmation is SHOULD.
