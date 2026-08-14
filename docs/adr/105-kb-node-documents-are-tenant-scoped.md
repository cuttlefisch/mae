# ADR-105: KB identity and node-document addressing on a multi-tenant daemon

**Status:** Accepted. Staged (1–4); Stage 1 is a prerequisite refactor with no behaviour change.
**Extends:** ADR-005 (KB CRDT), ADR-029 (CRDT-as-truth), ADR-060 (daemon multi-tenancy).
**Relates to:** ADR-023 (epoch fence), ADR-036 (signed content ops), ADR-037 (E2E), ADR-053 (live
query surface); issues #571 (read-side instance), #718 (write-side instance), #649 (no protocol
version negotiation), #632 (`checkpoint.rs` unreachable), #654 (`max_scan_nodes`).
**Prior art:** `docs/research/105-cross-kb-node-addressing-prior-art.md` — briefed for refutation
*before* implementation. It corrected D6 and reframed the Context.

## Context

`DocAddress` (`shared/sync/src/lib.rs`) names collaborative documents. KB **collection** docs are
KB-scoped — `kbc:{kb_id}` — but KB **node** docs are not:

```rust
DocAddress::KbNode { node_id }     => format!("kb:{node_id}")
DocAddress::KbCollection { kb_id } => format!("kbc:{kb_id}")
```

On a daemon hosting more than one KB, two KBs whose manifests contain the same node id therefore
resolve to **the same document**. This is what the code computes, not an inference:
`handle_kb_node_update` and `handle_kb_node_fetch` both build `format!("kb:{node_id}")`.

**Two consequences; only one was filed.**

1. **Authorization (#718, and #571 before it).** `kb_access` authorizes the caller for `kb_id`; the
   write then lands on a globally-addressed document. The ADR-023 epoch fence does not close it —
   the author writes under their own KB's epoch, which the fence accepts.
2. **Correctness, previously unrecorded.** Two *honest* tenants who both use `concept:architecture`
   silently share one CRDT document. Observed while researching this ADR: a tenant's first write to
   their own KB failed with `rebase required: … carries an op from stale-epoch client …` — the
   epoch fence firing because another tenant's KB had touched the same document. Cross-tenant
   interference surfacing as a spurious availability failure in an unrelated tenant.

Consequence 2 is why this is an **addressing** defect. The authorization hole is a symptom; no
amount of checking makes two tenants' documents distinct.

**Framing (prior art C4).** This is not an original design error. A flat document namespace is the
ambient default for CRDT servers — y-websocket identifies documents by a flat room name — and it is
correct for a single-tenant deployment, which is what MAE was when the scheme was chosen. ADR-060
introduced multi-tenancy and did not re-derive an addressing invariant that multi-tenancy
invalidated. The lesson is about derived invariants, not carelessness.

### What a design review found before implementing

The first draft of this ADR was incomplete and, implemented naively, **actively dangerous**.

**A. `kb_id` is signature-bound.** `MembershipOp::canonical_bytes` emits `field(&mut b,
&self.kb_id)` as its second field. Changing an existing KB's id invalidates every signed membership
op: `derive_valid_members` finds nothing (membership evaporates) and `derive_encryption` returns
`None`, so an **E2E KB silently reads as plaintext** — #573's exact failure mode. Renaming existing
KBs is therefore impossible, which reshaped D4.

**B. Node doc *names* are safe to change.** They are not in the signed bytes (ADR-036 binds author +
epoch + payload), and `seal_op(op_set_state, content_key, plaintext_update, client_id)` takes no doc
name or node id, so E2E ciphertext is not address-bound. `derive_kb_client_id(fingerprint, epoch)`
ignores `kb_id`, so the epoch fence is unaffected.

**C. A naive string rename introduces two auth bypasses, a signature bypass, and data loss.** Five
guards key on the literal `"kb:"` prefix, and every one fails **open or destructive** if it stops
matching: the raw-read gate (`collab_handler/mod.rs`) would return node plaintext to any client;
`sync_methods.rs` would skip signature verification, `kb_access` *and* the fence; `mod.rs`'s
`verify_relayed_content_op` would treat node ops as "not a content op"; `doc_store.rs`'s
`is_durable_doc` would make node docs non-durable, so they are **evicted and deleted**; and
`dialer.rs` would stop fencing P2P writes. This is why D1 exists.

**D. The leak extends into the cozo projection.** `projector.rs`'s `node_to_kbs` is a *set*, and a
node change re-projects into **every KB whose manifest lists it** — so a collision copies one
tenant's content into another's cozo store. Scoping makes this 1:1.

**E. `kb/share` has no owner check.** A second share of an existing id is silently "preserved" and
the caller subscribed. A `kb_id` is claimed first-come-first-served and held forever
(`kb/unregister` removes only metadata; `durable_kb_doc_survives_idle_eviction` confirms survival).

**F. Every editor's primary syncs as the literal `"default"`.** With E, the **second tenant to
connect cannot share their primary**: `kb/share` returns success, then every operation is denied by
`kb_access`. Silent, and fatal for a multi-tenant deployment.

## Decisions

### D1 — The doc-name taxonomy is type-driven; guards match variants, not string prefixes

Every guard routes through `DocAddress::parse` and matches **exhaustively** on the variant, so the
compiler — not grep — enforces coverage when the taxonomy changes. Unknown or unparseable names fail
**closed** at the raw-read gate.

Finding C is the whole justification: without this, the rename is a set of string edits in which a
single miss is an authorization bypass or silent data loss. With it, a missed case does not compile.

### D2 — Node documents are addressed `kbn:{kb_id}:{node_id}`

`DocAddress::KbNode` gains `kb_id`. The prefix is **new**, not a reuse of `kb:`: `kb:concept:buffer`
would otherwise parse as `kb_id="concept", node_id="buffer"`, leaving migration unable to
distinguish legacy from scoped. Discovered by an existing test during implementation.

Parsing splits on the first colon — `node_id` routinely contains colons, `kb_id` may not (D3).

### D3 — `kb_id` must be colon-free, validated where ids enter

`kb_id_is_addressable()`. A KB id was historically unvalidated; it now participates in an address,
so the constraint is enforced at the boundary with an actionable error rather than left as an
undocumented assumption `DocAddress::parse` silently depends on.

**Alternative rejected:** hashing the id into a fixed-length prefix is unambiguous with no new
constraint, but makes stored names opaque to operators and prevents `parse` from recovering the id.

### D4 — New shares mint an opaque unique id; existing shares keep theirs

Per finding A, an existing KB's id cannot change. So `generate_uuid()` at *first* share, stored in
the already-existing `primary_collab_id` / `KbInstance.collab_id` rather than falling back to
`KB_DEFAULT_NAME`. The human-facing name lives in collection metadata, where `kb/register`'s `name`
already is. Existing name-ids stay valid — they are already claimed and held.

This is what lets two tenants each have a primary KB (finding F).

**The storage is trivial; the cost is de-conflating name from id.** "Is this the primary?" is
currently `kb_name == KB_DEFAULT_NAME || kb_name == "primary"`, and the editor passes
`active_instance_name()` — a *name* — where the daemon expects a collab *id*, at six or more sites.
Under D4 a uuid-id primary would silently stop being recognised. Those sites are part of this
decision, not follow-up cleanup.

**Alternative rejected:** owner-qualified ids (`{owner_fp}:{name}`) — fingerprints contain `:`,
breaking D2's split, and ownership transfer would change a KB's identity.

### D5 — `kb/share` refuses an id already owned by a different principal

Turns finding E's silent merge into a named conflict. With D4 this should be unreachable, which is
exactly why it must fail loudly if it is ever reached.

### D6 — `require_node_in_kb` stays, and the write path gains it

**This reverses the first draft**, on the prior-art brief. That draft held that namespacing makes
the check redundant. Published consensus (OWASP, AWS, Redis) is the opposite: tenant-prefixed keys
**and** authorization checks, neither sufficient alone.

They answer different questions. Namespacing guarantees a *well-formed* address cannot collide. It
does not authorize the caller for the KB named in that address (`kb_access` does), nor establish
that the node is in that KB's manifest (`require_node_in_kb` does) — which still matters for a node
not granted within an otherwise readable KB. Removing it would have been a regression introduced by
this ADR, and is recorded because a decision defended by a wrong reason gets re-litigated the moment
that reason is challenged.

### D7 — Node creation is ordered manifest-first, as a stated contract

The editor drains `pending_kb_updates` before `pending_kb_manifest`, so a new node's first update
arrives before its `kb/collection_node_add`. That inverts. Not load-bearing for isolation (D2 is);
it exists so a legitimate create is not refused by D6, which is what the `@ai-caution` marker at
`kb_content.rs` predicted would break.

### D8 — Staged, and the addressing change is taken before first hosted deployment

1. **Stage 1 — D1 alone**, no rename: convert the five guards to exhaustive `DocAddress` matching
   against today's names. No behaviour change, independently reviewable, and the safety net for
   everything after.
2. **Stage 2 — D2/D3/D6/D7**: the addressing change, correct under today's accidental uniqueness.
3. **Stage 3 — D4/D5**: KB identity; fixes finding F.
4. **Stage 4 — migration**: enumerate `list_documents()`, rewrite legacy `kb:` keys via each `kbc:`
   manifest, and **halt on a node ambiguous across two manifests** rather than guess. Includes
   `checkpoint.rs`, which restores `kb:{id}` and would restore into the wrong namespace once wired
   up (it is currently unreachable — #632).

**Why now.** Document names cross the wire (`"doc"` in every `sync/*` call) and are storage keys.
Doing this after deployment is expand/contract *plus* a wire-skew window — and MAE has **no
editor↔daemon protocol version negotiation** (#649, open), so a skewed pair connects and proceeds
with undefined behaviour. The cost is not that the change is hard now; it is that #649 makes it
disproportionately harder later.

## Success criteria

Effects, not returned values. Each must **fail against `main`** before being kept.

0. **Every cross-KB test uses the same node id in both KBs.** The existing #571 adversarial test
   seeds `concept:a-own` in one and `concept:b-secret` in the other — deliberately distinct, and
   therefore structurally incapable of observing this collision. That is the "unicorn value"
   failure of CLAUDE.md #14 occurring inside a test written to be adversarial, and it is why this
   bug survived a review. Reusing that shape rebuilds the blind spot.
1. Two tenants, same node id → each reads **its own** content.
2. No cross-tenant epoch interference: the observed `rebase required: … stale-epoch client …` no
   longer occurs when another tenant holds a same-named node.
3. **Two tenants can both share a primary KB** (finding F).
4. A foreign-node write is refused **and the victim's content is unchanged** — asserted on content,
   since a refused-but-applied write would pass a response-only assertion.
5. Node creation still works end-to-end through the real editor drain, including a brand-new node's
   first update.
6. `kb/share` of an already-owned id is refused with a named conflict.
7. `DocAddress` round-trips for node ids containing colons; legacy `kb:` forms do **not** parse.
8. **Guard coverage (finding C):** raw `sync/full_state` on a node doc is still refused; a
   `sync/update` on a node doc still requires signature + `kb_access` + fence; node docs are still
   durable. These are regression tests for bypasses *this ADR could introduce*.
9. An E2E KB is still E2E after the change (finding A's failure mode, asserted directly).
10. Full daemon + editor suites; `make audit-metrics-check` clean, no baseline bless.

## Consequences

- **The wire format changes.** `"doc"` values for KB nodes gain a `kb_id` segment. Pre-deployment
  this costs nothing; it is exactly what #649 would otherwise have to negotiate.
- **Storage keys change**, requiring Stage 4 for any existing store — including MAE's own dogfood
  stores.
- **A hosted daemon will hold a mix of name-ids and uuid-ids indefinitely** (finding A). Not
  cosmetic: `kb_id_is_addressable` must hold for both.
- **KB names lose one character** (`:`). No existing name is known to use it.
- **`kb:` and `kbc:` become consistent** — both `{prefix}:{kb_id}:…`.
- **`require_node_in_kb` remains** (D6): this ADR adds a structural guarantee *beneath* the existing
  check, it does not replace it.
- The `@ai-caution: [kb-scoping]` marker at `kb_content.rs` is removed, because the hazard it
  describes stops existing rather than becoming someone else's to remember.
