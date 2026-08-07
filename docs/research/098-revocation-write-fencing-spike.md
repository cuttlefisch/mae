# Revocation write-fencing spike: does the epoch fence decouple revocation from rotation?

**Status:** Run, 2026-08-05, against `main` at `33282fb9`.
**Gates:** ADR-098. Fourth and last of the multi-device arc.
**Artifacts:** `daemon/src/collab_handler/tests/collab_handler_device_revocation_fence_tests.rs`
(5 tests, passing).

**Bottom line up front.** The hypothesis was half right, and the half that holds is the useful one.

- **Yes — revocation is decoupled from rotation.** Removing a device stops its writes immediately,
  with **no content-key rotation anywhere in the path**. So a lost laptop can be silenced without
  paying #176's history-stranding cost.
- **No — it is not decoupled from the owner.** The thing that stops the write is the `kb_access`
  authorization gate, not the epoch fence, and it needs an owner-authored `Remove`. The fence
  cannot carry revocation at all, for a reason that is structural rather than incidental.

## The hypothesis

The forward-secrecy spike ended on this, explicitly flagged as untested:

> ADR-023's epoch fence may fence a revoked device's *writes* independently of key rotation, which
> would decouple the two halves of revocation.

It mattered because rotation is expensive — it permanently strands history for every member who
later re-syncs (#176). If write-stopping needs no rotation, "device lost" and "content must be
protected going forward" become two separately-priced operations rather than one.

Tested against the real dispatch path (`handle_doc_request_inner`, the same entry a live peer
hits), not against the fence function in isolation, so what is measured is what actually happens
to a revoked device's write.

## Finding 1 — removal stops writes, with no rotation

`a_removed_device_stops_writing_with_no_key_rotation_involved`: a device is admitted as editor,
joins, and writes successfully; the owner removes it; its next write is rejected. No rotation, no
re-wrap, no key change occurs anywhere in the test.

This is the valuable result. It means ADR-098 can treat the two halves of revocation as separately
priced:

| Operation | Authority | Cost |
|---|---|---|
| Stop a lost device writing | owner-authored `Remove` | none — no history impact |
| Stop it *reading* new content | owner-authored rotation | strands history for every re-syncing member until #176 is fixed |

That is a materially better position than the forward-secrecy spike left things in, where rotation
looked mandatory for any revocation. A device-loss response can be immediate and cheap; the
expensive operation can be batched, or reserved for cases where future confidentiality genuinely
matters.

## Finding 2 — the access gate does the work, not the fence

`a_removed_device_is_stopped_by_the_access_gate_not_the_epoch_fence`: the rejection message is
asserted **not** to be the fence's distinctive `"rebase required … stale-epoch client"`
(`enforce_epoch_fence_with_coll`). The removed device is rejected earlier, by `kb_access`.

This matters because the two have different requirements. The access gate needs an owner-authored
`Remove` to have propagated to the peer; the fence needs only an epoch bump. Knowing which one
does the work tells you what a self-service design would actually have to change — and it is the
harder of the two.

## Finding 3 — why the fence *cannot* carry revocation

`the_fence_discriminator_is_unchanged_by_revoking_an_epoch_zero_device`.

The fence discriminates on `derive_kb_client_id(principal, epoch_now)`. Two documented facts
combine badly:

- `kb_member_epoch`: *"Absent member ⇒ 0."*
- `MembershipOp::epoch`: *"A fresh grant stays at epoch 0."*

So for the common case — a device admitted by a fresh grant and never role-changed — the client id
the fence computes is **identical before and after revocation**. The fence has nothing to
discriminate on.

The test also asserts the complement (`derive_kb_client_id(fp, 0) != derive_kb_client_id(fp, 7)`),
so the first assertion cannot be misread as "the fence never discriminates". It does — across
epoch *changes*, which is its actual ADR-023 job of forcing a rebase after a grant or role change.
It is simply not a revocation signal, and was never meant to be.

## Finding 4 — revocation stays owner-authored at the real API

`a_non_owner_member_cannot_revoke_a_device` confirms at dispatch level what the membership-layer
spike found: a member cannot revoke, and the attempt does not half-apply — the target keeps
writing afterwards, so the rejection is real rather than an error returned after a partial effect.

`revocation_does_not_disturb_a_bystander_device` is the negative control: a second authorised
device on the same KB is unaffected, so revocation is per-principal rather than a KB-wide freeze.
Without it, "the revoked device is stopped" could pass on an implementation that simply broke the
KB.

## What the arc now establishes for ADR-098

Across all four spikes:

1. **A device set is expressible** (delegated invite), but modelling devices as members applies
   every membership semantic to them, several of which are wrong. Devices must be something a
   member *has*.
2. **Content-key delivery is owner-rooted**, proven twice by independent routes.
3. **Revocation is owner-rooted**, proven at both the membership and dispatch layers.
4. **Write-revocation is cheap and rotation-free**; read-revocation is expensive and requires
   rotation.
5. **#176 is a prerequisite**, not an adjacent bug — and fixing it removes the only thing making
   rotation-on-revocation unworkable.

The consistent shape: the *owner* is in the loop for every authority-changing operation, and no
existing mechanism routes around that. A self-service device design must change that rule
deliberately, with its own threat-model argument — it is not an oversight to patch.

## What this still does *not* establish

- **The partition window.** A peer that has not yet received the `Remove` will still accept the revoked device's writes. That is inherent to a distributed signed log, but its practical size — and whether a browser client makes it worse — is unmeasured.
- **Mesh relay parity.** `enforce_epoch_fence`'s doc claims it is the one fence shared by the hub and the mesh dialer (#157 N1). This spike exercised the hub path only.
- **Whether a member may author a rotation.** Still inferred from the wrap-authorship rule, not measured directly.
- **Anything about OIDC or AD groups.** Unchanged: still the separate, still-open layer.

## Reproducing

```bash
cd daemon && cargo test --lib device_revocation
```
