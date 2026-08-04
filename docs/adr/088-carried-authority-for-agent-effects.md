# ADR-088: Agent effects are authorised by carried provenance, not ambient session tier

**Status:** Proposed. Deferred past v0.15 — recorded now so ADR-084's known limit has a named owner
rather than being an unstated gap.
**Extends:** ADR-084 (permission enforcement at the effect — this ADR replaces *what* the effect
consults; ADR-084 is the precondition, since there must be one enforcement path before it can be
taught provenance).
**Relates to:** ADR-048 (AI residency policy), ADR-051 (per-session policy), ADR-085 (the category
axis), ADR-089 (workspace trust — the same question asked of files rather than tool calls).

## Context

ADR-084 brings every entry point under one enforcement path and gives every Scheme primitive a
required tier. That closes the "the gate is only on one path" defect. It does not close the defect its
own threat model describes.

ADR-084's stated threat model is that **the AI agent acts on the user's authority but is driven by
content the user did not author** — a cloned repository, a federated KB, fetched web content, tool
output. The evidence supports that framing strongly: NIST defines agent hijacking in exactly these
terms, and the most-exploited documented chain against AI IDEs is "cloned repo → hidden prompt
injection → file write → config modification → code execution."

But the check ADR-084 installs consults the **ambient session tier**. It answers *"is this session
allowed to spawn a process?"* It cannot answer *"did the argument to this spawn originate from the user,
or from text in a README the user cloned?"* Only the second question is the confused-deputy question,
and the confused deputy is the whole threat model.

This is a known-bad shape, not a novel risk. Hardy's 1988 paper describes Tymshare's actual fix for the
original confused deputy — a system call letting the compiler select which of its two authorities to act
under, i.e. an ambient authority the privileged operation consults — and records the outcome: *"Note the
increase in complexity! … it soon became clear that more than two 'authorities' were necessary … there
were other authority mechanisms besides access to files. Generalizations were not obvious and the
modifications to the system were not localized."* Miller, Yee & Shapiro give the definition MAE's design
currently matches verbatim: ambient authority is *"authority that is exercised, but not selected, by its
user"*, where *"the caller … does not choose any credentials to present with the request; the request
merely succeeds or fails."*

And the general law: *"When designators and authorities take separate paths through a system, their
recombination is likely to lead to confused deputies."*

## Decision (proposed)

**Authority travels with the request, tagged by provenance; effects evaluate the tag, not the session.**

1. Values entering an agent's context are tagged with their origin — user keystroke, project file,
   fetched URL, federated KB node, tool output — and the tag propagates through the call chain that
   consumes them.
2. Effect-level enforcement points evaluate the tag of the *arguments* alongside the session tier. An
   effect whose arguments are content-derived is held to a stricter policy than one whose arguments are
   user-derived, and fails closed when provenance is unknown.
3. MAE already has the seam for the minimum viable version: `Editor::with_ai_dispatch_scope`
   (`crates/core/src/editor/window_ops.rs`) already wraps every MCP-originated dispatch. Tagging
   AI-originated work as content-derived at that boundary is a strictly smaller change than full value
   tagging and captures the dominant case.

The worked precedent is CaMeL (*Defeating Prompt Injections by Design*, Debenedetti et al., 2025): a
custom interpreter that "tracks provenance and enforces security policies," with capabilities as
per-value tags recording "the value's sources and allowed readers," and policies evaluated at the tool
call against those tags, such that untrusted data "can never impact the program flow."

## Why this is deferred rather than done now

- It is a data-flow change across the agent context, the Scheme VM, and the tool-dispatch path. ADR-084
  is a precondition and has not shipped.
- ADR-084's compiler-proven allow-list delivers most of the *containment* benefit for v0.15's actual
  deployment shapes, without which provenance tagging would have nothing to enforce against.
- Getting the tag lattice wrong is worse than not having one, because it would license relaxing the
  tier checks that currently do the work.

## Consequences of deferring

Stated plainly so the gap is not forgettable: until this ships, **MAE's permission tier bounds what a
session may do, not what untrusted content may cause it to do.** A `shell`-tier session — the current
default — will execute a process whose command string came from a cloned repository's README exactly as
readily as one the user typed. `SECURITY.md` must say this, and ADR-084's Consequences section does.

## Verification (when implemented)

Adversarial by construction, per principle #14: a corpus of injected content (repo file, KB node,
fetched page, tool output) each attempting to reach an effect, asserting refusal — and the matching
user-originated request for the same effect, asserting success. A test that only shows the effect is
reachable proves nothing here.
