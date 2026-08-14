# ADR-105 Phase 0: prior-art review

**Purpose.** Brief ADR-105's decisions against published practice *before* implementing, per the
standing practice that grounding ADR-084/085 this way reversed one decision outright and corrected
two more. Written **before** the implementation, which is the right order.

**Method.** Each decision is stated as a falsifiable claim, then tested against the strongest
contrary source found. A decision that survives is *holds*; one that does not is *refuted* or
*rationale corrected* — the latter meaning the decision stands for different reasons than were
first given, which matters because a decision defended by a wrong reason gets re-litigated the
moment that reason is challenged.

**Verdict up front.** Three claims hold. **One rationale is corrected, and it is the most useful
finding here: namespacing does not replace the authorization check — published consensus is
explicitly that both are required.** The original framing ("scope the namespace *instead of*
adding a check") is wrong, and implementing it that way would have removed a control that prior
art says must stay. One further claim is *reframed*: the flat namespace was not a mistake, it was
a correct single-tenant default that multi-tenancy invalidated.

---

## C1 — A multi-tenant document store must carry the tenant in the key, not only in a check

**Claim.** `kb:{node_id}` is insufficient for a daemon hosting multiple tenants; the tenant must
be part of the storage key.

**Prior art.** Uniform and explicit. The [OWASP Multi-Tenant Security Cheat
Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Multi_Tenant_Security_Cheat_Sheet.html) and
[AWS's tenant-isolation guidance](https://aws.amazon.com/blogs/security/tag/tenant-isolation)
both describe tenant-prefixed storage paths (`s3://bucket/{tenant-id}/…`) as the standard
storage-level isolation primitive, paired with policies scoped to that prefix.
[Redis's multi-tenant guidance](https://redis.io/blog/data-isolation-multi-tenant-saas/) states
the same for cache keys: prefix every key with the tenant identifier, and include `tenant_id` in
every query, cache key, and storage path.

**Verdict: holds.** Notably, "a shared cache key" is named in that literature as a *canonical*
multi-tenant failure mode — which is precisely the shape of this bug, in a CRDT document store
rather than a cache.

---

## C2 — Structural prevention is preferable to a check, because a check can be forgotten

**Claim.** Making the collision impossible to express beats detecting it at each call site.

**Prior art.** Supported, and stated more sharply than the claim itself:
[brotcode's multi-tenant isolation write-up](https://brotcode.com/blog/engineering/data-isolation-security-multi-tenant-systems/)
observes that *"the moment isolation depends on developer discipline, it's already fragile"*, and
enumerates the recurring failures as "a missing filter in one query, a shared cache key, or an
'admin' role that isn't properly scoped".

MAE has already paid this exact tax twice: #571 fixed the unscoped `node_id` on the **read**
path, and the identical shape survived on the **write** path (#718) for the entire interval —
with an `@ai-caution` marker acknowledging it. That is the "missing filter in one query" failure
observed in-house, not in theory.

**Verdict: holds.**

---

## C3 — Namespacing lets the per-call authorization check be removed

**Claim (as originally framed).** With `kb:{kb_id}:{node_id}`, `require_node_in_kb` becomes
redundant — the collision is impossible, so there is nothing to check.

**Prior art. This is refuted.** The consensus across every source checked is explicitly
*defense-in-depth*: tenant-prefixed keys **and** authorization checks, neither sufficient alone.
OWASP's guidance is that tenant boundaries are enforced "at the token level, in APIs, inside
policies, and down to the database" — four layers, not one. LoginRadius's
[multi-tenant authorization guidance](https://www.loginradius.com/blog/identity/what-is-multi-tenant-authorization)
puts it as: every permission check should be scoped by tenant id, and a user should never
interact with unscoped global resources — both halves.

The concrete reason the check must stay: namespacing guarantees that a *well-formed* address
cannot collide. It does **not** authorize the caller for the KB named in that address. A caller
supplying `kb_id: "team-b"` still needs `kb_access` to prove membership, and
`require_node_in_kb` still answers a different question — whether the node is in *that KB's
manifest* — which matters for a node the caller has not been granted within an otherwise
readable KB.

**Verdict: rationale corrected.** The decision to namespace stands; the justification "so we can
drop the check" does not. ADR-105 must keep `require_node_in_kb` and say why.

---

## C4 — The flat namespace was a design error

**Claim (as originally framed).** `kb:{node_id}` was a mistake from the start.

**Prior art. Reframed.** [y-websocket](https://docs.yjs.dev/ecosystem/connection-provider/y-websocket),
the reference Yjs server, identifies documents by a **flat room name** — all clients on the same
room name edit the same document, with no tenant dimension. That is the ambient default for CRDT
servers, and it is correct for a single-tenant deployment, which is what MAE was when the scheme
was chosen. Multi-tenant Yjs deployments layer namespacing *on top*: `y-socket.io` scopes rooms
into socket namespaces (`ws://host/yjs|room`), and sharded deployments hash the document id to a
server.

**Verdict: reframed, and the reframing matters for the ADR's tone.** This is not "we got the
addressing wrong"; it is "ADR-060 introduced multi-tenancy and did not revisit an addressing
scheme that multi-tenancy invalidated". The lesson to record is about *derived invariants* — a
decision that was sound under one assumption needs re-deriving when that assumption changes —
not about carelessness.

---

## C5 — Do it before first hosted deployment

**Claim.** The change is cheapest now, because there are no deployed peers to negotiate a wire
change with and no persisted stores to migrate.

**Prior art.** No contrary source; this is standard expand/contract reasoning. The relevant
external practice is what it costs *later*: a rename of storage keys after deployment is the
textbook expand/contract (dual-write, backfill, cut over, contract) migration, which for a
CRDT store also requires the wire-format skew window to be handled — and MAE has **no
editor↔daemon protocol version negotiation at all** (#649, open), so a skewed pair today
connects and proceeds with undefined behaviour.

**Verdict: holds, and is the strongest sequencing argument available** — not because the change
is hard now, but because #649 makes doing it later disproportionately harder.

---

## What this brief changed

1. **`require_node_in_kb` stays.** The original plan to treat it as redundant is refuted by the
   defense-in-depth consensus (C3). This is the finding that would have caused a real regression.
2. **The ADR's framing changes** from "fix a design error" to "re-derive an invariant that
   multi-tenancy invalidated" (C4).
3. **The sequencing argument sharpens**: the reason to act now is #649, not convenience (C5).
