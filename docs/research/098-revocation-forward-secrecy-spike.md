# Revocation forward-secrecy spike

**Status:** Run, 2026-08-05, against `main` at `33282fb9`.
**Gates:** ADR-098. Third and last of the multi-device arc — read after
`098-multi-device-identity-spike.md` and `098-device-secret-storage-and-revocation-spike.md`.
**Artifacts:** `shared/sync/tests/revocation_forward_secrecy.rs` (7 tests, passing).

**Bottom line up front.** Forward secrecy works: after a rotation, a revoked device cannot open
new content. But two things it does *not* do decide the design.

1. **Revocation is forward-only.** A revoked device keeps every op sealed under the old key,
   permanently. No owner action takes that back.
2. **Rotation truncates history for the innocent.** A legitimate, continuously-authorised member
   who re-syncs from scratch after a rotation loses *all* pre-rotation content — and the loss is
   cumulative across rotations. This makes "rotate on every device revocation" unworkable as
   stated.

**But the blocker is removable, and cheaply.** Retaining every key ever wrapped to a member
restores complete history *without* giving the revoked device anything — the two properties are
independent. That is already filed as issue #176 with `FIXME(#237)` in the code. This spike
reclassifies it: not an unrelated lifecycle bug, but a **prerequisite for ADR-098**.

## Why this spike

Both earlier reports ended on the same unresolved point, and it was the one that could invalidate
the whole direction:

> A revoked device already held the content key. Revoking it does not un-share what it has. So a
> real revocation must also rotate the content key — an owner action. Can device revocation avoid
> dragging the owner back in?

The previous spikes worked at the membership layer. This one works at the **op-set layer**, where
the encryption actually happens, using real `KbNodeDoc` updates rather than opaque blobs so what
is sealed is what actually syncs.

## What holds

`a_revoked_device_cannot_open_post_rotation_content`: with 3 pre-rotation and 4 post-rotation ops,
a device holding only the old key opens exactly 3 and reports 4 undecryptable. Forward secrecy is
real and is the entire security value of rotating on removal.

`a_device_with_an_unrelated_key_opens_nothing` and `without_a_rotation_one_key_opens_the_entire_op_set`
are the negative controls: a stranger's key opens nothing (and every op is *reported* undecryptable
rather than silently absent), and with no rotation one key opens everything. Without these, the
counts below could be artefacts of the harness rather than of rotation.

## What does not hold — finding 1: revocation cannot un-share history

`a_revoked_device_retains_every_pre_rotation_op_permanently`: the revoked device opens all 5
pre-rotation ops, forever.

This is documented intent, not a defect. `find_wrapped_content_key`'s own doc comment says a
removed member "keeps their OLD wrapped blob … and so can read history they already had — the
intended ADR-037 §D3 semantics, NOT a leak."

For ADR-098 it is a hard boundary to design around rather than discover: **revoking a lost laptop
protects future content only.** Anything the device already had access to must be assumed
compromised, and if that is unacceptable for a given KB the answer is not revocation but treating
the content itself as disclosed.

## What does not hold — finding 2: rotation truncates history for the innocent

`a_legitimate_member_resyncing_after_rotation_loses_all_pre_rotation_content`: a member who was
authorised throughout, but whose local state was lost (reinstall, new device, offline catch-up),
derives only the current key and replays the full op-set — opening **3 of 9 ops**, with 6
legitimately-authored ops permanently unreadable.

The mechanism: `open_new_ops` decrypts with a single `ContentKey` and skips what does not open;
`find_wrapped_content_key` returns only the latest wrap.

`every_additional_rotation_strands_strictly_more_history` quantifies the compounding. Four epochs
of two ops, three rotations between them — i.e. three device revocations over a KB's life — leaves
a re-syncing member able to read **2 of 8 ops**. Holding an older key opens exactly that key's own
epoch and nothing else, confirming the stranding is per-epoch rather than an ordering artefact.

So with one rotation per device revocation, a KB's readable history for anyone who re-syncs shrinks
to "whatever was written since the most recent device was revoked". In an organisation with many
users and several devices each, that window is short and shrinking. **This disqualifies
rotate-per-revocation as stated.**

## The blocker is removable, and that is the useful result

`trying_every_retained_key_restores_full_history_without_helping_the_revoked_device`: a member
retaining both keys and trying each per blob recovers all 9 ops. The revoked device, retaining only
the old key, still opens 6 — exactly what it had before.

The two properties are **independent**. Restoring history for legitimate members does not weaken
forward secrecy, because the revoked device is never wrapped the new key in the first place. That
is why the current single-key behaviour is a defect rather than a deliberate trade-off, and it is
precisely the fix issue #176 already recommends ("have each member retain all content keys wrapped
to them … and have `open_new_ops` try each key per blob").

The op-set layer needs no change at all — only the caller's key handling. This spike demonstrates
that using the shipped API unmodified.

## What ADR-098 can now take as established

1. **Forward secrecy on revocation works**, and is forward-only. Pre-revocation content is gone.
2. **Rotate-per-device-revocation is unworkable until #176 is fixed** — and is workable after. #176 is therefore a **prerequisite of ADR-098**, not an adjacent bug. That is the single most actionable output of this spike.
3. **Content-key rotation remains owner-authored.** This spike did not find a path around that; combined with the previous two spikes, the owner is in the loop for delivery *and* rotation. Self-service device management for E2E KBs requires changing that rule deliberately, with its own threat-model argument.
4. **The independence of the two properties** is the lever: history availability and forward secrecy can be satisfied simultaneously, so ADR-098 does not have to trade one against the other.

## What this still does *not* establish

- **Whether a member may author a rotation at all.** Tested at the membership layer previously (a member's wrap is ignored); not re-tested here for a rotation specifically. Likely the same answer, but "likely" is not "measured".
- **Key-history delivery.** Retaining keys assumes a member *has* the old wraps. A member who joined after a rotation never did — the `FIXME(#237)` re-admit gap, explicitly a v0.16 item (ADR-037 §D4). A device enrolled after a rotation is the same case.
- **Rotation cost at scale.** Each rotation re-wraps to every member; nothing here measures that against a realistic member count.
- **Write fencing.** ADR-023's epoch fence may fence a revoked device's *writes* independently of key rotation, which would decouple the two halves of revocation. Untested — it lives in `daemon/src/collab_handler`, outside this crate.

## Reproducing

```bash
cargo test -p mae-sync --test revocation_forward_secrecy
```

No external harness required.
