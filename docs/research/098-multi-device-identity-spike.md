# Multi-device identity spike: can one member hold several device keys?

**Status:** Run, 2026-08-05, against `main` at `33282fb9`.
**Gates:** ADR-098 (durable identity for network clients). Do not write that ADR before reading this.
**Artifacts:** `shared/sync/tests/multi_device_identity.rs` (7 tests, all passing).

**Bottom line up front.** No. `Rebind` (ADR-040) is **succession, not concurrency** — enrolling a
second device retires the first, and a retired device cannot enrol a third, so there is no
fan-out path. Separately, even where membership resolves, an E2E content key reaches a new device
only via an **owner-authored wrap**, which is an owner action per device per KB. Both walls are
now demonstrated by executable tests rather than inferred from the ADR text.

One unexpected finding: **the owner already has working multi-device**, by a mechanism the member
path lacks, and that asymmetry is not documented in ADR-040.

## Why this was the next spike

CC3 in the browser-KB plan identified this as the single most likely thing to make Browser MAE
unusable in practice. A browser gives every profile its own keypair, so one human across a
laptop, desktop and phone is three fingerprints — against a membership model keyed on a single
fingerprint inside a signed, peer-verifiable, append-only op-log (ADR-026).

ADR-040 looked like it might already solve this: its `Rebind` op has an old key cross-sign a new
one, additively, with no history rewrite, and it cites Matrix cross-signing (MSC1680) and Keybase
sigchains as prior art. The spike existed to check whether it *is* the answer, by running it.

## Findings

### 1. `Rebind` is succession — enrolling a device retires the previous one

`enrolling_a_second_device_via_rebind_retires_the_first`: a member admitted on a laptop enrols a
phone the only way the op-log allows; the phone becomes a member and **the laptop stops being
one**. "Add a device" and "replace a device" are the same operation, so a browser enrolment
silently logs the user out everywhere else.

This is what ADR-040 §2 designed — *"the derived membership entry for `old_fp` is **transferred**
to `new_fp` … and `old_fp` is **retired**"* — and the crate's own unit tests already say so in
their names (`rebind_transfers_membership_to_successor_and_retires_predecessor`, and chained
rebinds "resolve to `c`; all predecessors retired"). It is correct for key rotation. It cannot
express multi-device.

`a_retired_device_cannot_enrol_a_further_device` closes the obvious workaround: rebinding once
per device does not fan out, because the first rebind retires the authority needed for the
second. Membership converges to exactly the owner plus **one** live device, never a device set.

### 2. E2E content access is owner-gated per device, structurally

`a_newly_enrolled_device_holds_no_content_key_until_an_owner_wrap_exists`: the enrolled device is
a member and still reads nothing, because the content key was sealed to the *previous* key and
nothing has sealed it to the new one.

`the_owner_re_wrap_does_deliver_the_key_to_the_new_device` is the paired complement, so the
finding is a real authority boundary rather than an artefact of a lookup that never finds
anything — without it the first test would pass against a broken implementation.

ADR-040 §3 names this deliberately: *"For an E2e KB the owner is therefore implicitly in the loop
— a rebind isn't readable until the owner re-wraps — which is the correct authority boundary."*
Correct for rotation; for multi-device it is N devices × M KBs of owner actions, which is the
scaling wall CC3 predicted.

### 3. The owner already has multi-device, and ADR-040 does not say so

`an_owner_keeps_every_rotated_key_authoritative_unlike_a_member`: after the owner rebinds to a
second key, **both** keys still satisfy `is_owner_principal`.

`owner_principal_chain` (`shared/sync/src/membership.rs`) is a forward-closure **set** that only
ever inserts, never retires — deliberately, with a soundness argument in its own doc comment
(each link carries the predecessor's signature and is fingerprint-bound, and E2e KBs are
SingleOwner so the owner is irrevocable). Owner-rooted readers accept an op from any principal in
that set.

So the additive-set shape multi-device needs **already exists in the codebase** — it is simply
not available to members.

Two consequences worth separating:

- **Useful:** ADR-098 does not have to invent a new derivation shape. There is a working,
  reasoned precedent in the same file.
- **Concerning:** ADR-040 §2 describes rebind uniformly as transfer-and-retire, and its threat
  model asserts *"a retired key's post-rebind ops are fenced"* — true for members, **false for the
  owner**. A rotated-away owner key retains full authority permanently. That is consistent with
  the ADR's stated rotation-vs-compromise fork (an owner is irrevocable; compromise recovery is
  out of scope), but it is an undocumented exception to a stated security property. Filed
  separately.

### 4. The safety properties that must survive any redesign

Pinned now so ADR-098 cannot regress them:
`enrolling_a_device_cannot_elevate_the_role_it_inherits` (a viewer's device inherits exactly
Viewer) and `an_unvouched_device_is_not_a_member` (the negative control for the whole file — if
this ever passed, every other assertion would be vacuous).

## What the prior art says the answer is

MAE's Rebind maps onto Matrix's **master key** rotation, not onto the part of Matrix that
actually does multi-device. Matrix separates three keys:

- **Master key** — the root of a user's cross-signing identity.
- **Self-signing key (SSK)** — signs *the user's own devices*. This is the multi-device primitive MAE has no analogue for.
- **User-signing key (USK)** — signs other users' master keys.

Devices signed by their owner's SSK are trusted transitively, and **keys are only shared with
cross-signed devices** — so signing authority and content-key delivery are solved by the same
mechanism, without the room owner being involved per device.

The remaining problem — how the private cross-signing material reaches a new device at all — is
solved by **SSSS** (secure secret storage and sharing): the private keys are stored encrypted in
server-side account data, under a key derived from a user-held base58 recovery key via HKDF. The
server holds only ciphertext.

Mapping that onto MAE, the shape ADR-098 should evaluate:

1. **Membership names a stable member key, not a device key.** The member key is the master; it lives in secret storage, not on a device.
2. **A new additive op lets the member key sign device keys** — the SSK role — reusing the `owner_principal_chain` set-derivation shape that already exists rather than inventing one.
3. **The content key is wrapped once to the member's X25519 wrap key** (ADR-041 already separated signing from wrapping), not per device. The owner leaves the per-device loop entirely.
4. **A MAE SSSS analogue** delivers the member secret to a new device, encrypted under a user-held recovery key. MAE is already positioned for this: the daemon is proven key-blind by `scripts/collab-encrypted-e2e.sh`'s canary test, and a recovery-key registry already exists in `membership.rs` (`recovery_registry`, `is_recovery_rebind`) — though it registers a *public* key authorized to sign a Rebind, which is not the same thing as encrypted secret storage.

ADR-040 anticipated exactly this and deferred it: its open question Q2 defers *"a pre-registered
offline recovery key that can override a compromised signing key, à la Matrix's master-key
reset."*

**The cost, stated plainly.** Matrix's model carries a documented UX price MAE would inherit: an
unverified session shows "Unable to decrypt" rather than failing usefully, verification has been
repeatedly reported as confusing, and QR-code flows were added specifically to make it bearable.
Any ADR-098 that adopts this shape must own that cost rather than assume it away — a browser user
who clears site data will land in exactly that state.

## What this does *not* establish

- **No design is decided.** This spike falsifies the "Rebind is enough" hypothesis and identifies the shape of the answer. The op-set design, its derivation rules and its adversarial tests are ADR-098's work.
- **The SSSS analogue is unvalidated.** Nothing here tests secret storage, recovery-key derivation, or device-to-device key sharing. That likely needs its own spike before ADR-098 is final.
- **Nothing about OIDC or AD groups.** This is purely the cryptographic-subject layer. How an OIDC principal binds to a member key, and how AD groups drive provisioning, are separate and still open.
- **Nothing about revocation.** Removing a lost device from a member's device set — the counterpart of enrolment, and the thing that makes the whole model safe — was not tested and has no mechanism today.

## Reproducing

```bash
cargo test -p mae-sync --test multi_device_identity
```

No external harness required.
