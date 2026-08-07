# Device secret-storage and revocation spike

**Status:** Run, 2026-08-05, against `main` at `33282fb9`.
**Gates:** ADR-098 (durable identity for network clients). Read with its sibling,
`docs/research/098-multi-device-identity-spike.md`.
**Artifacts:** `shared/sync/tests/device_secret_storage_and_revocation.rs` (11 tests, passing).

**Bottom line up front.** A cheaper path than a new op set exists and half works: **delegated
invite (`can_invite`) genuinely expresses a concurrent device set**, which `Rebind` could not. But
it fails on both halves that matter — a member cannot deliver the content key to their own device,
and a member cannot revoke their own device. Worse, modelling devices as members makes every
membership semantic apply wrongly to them.

The conclusion is the opposite of the hypothesis tested, which is why it was worth testing:
**devices must be a distinct concept, not members.**

Separately, the SSSS analogue MAE needs is **buildable from primitives already shipped**, with no
new dependency.

## Why this spike

The first spike left two questions open, and named them as the ones that reshape a design rather
than fill in under it: revocation (enrolment without it is a liability, not a feature) and secret
storage (how member material reaches a new device without an owner round-trip).

Before proposing new ops, this one tested a cheaper hypothesis the first spike had not considered:
MAE already has delegated invite and an inviter-removal cascade. Perhaps a member can simply admit
their own devices and remove them again.

## Part 1 — delegated invite as a device-enrolment path

### It works for membership

`a_member_with_can_invite_can_enrol_their_own_second_device`: a member holding `can_invite` admits
their own phone; laptop and phone are **both** members simultaneously. This is exactly the
concurrency `Rebind` could not express, using machinery that already ships.

`an_enrolled_device_cannot_exceed_the_enrolling_identity_role` confirms the escalation guard holds
(`author.role.includes(op.role)`), so a viewer cannot enrol an editor device.

### It fails on content-key delivery — the same wall by a different route

`a_member_cannot_deliver_the_content_key_to_their_own_device`: the member *holds* the content key,
and authors an Admit carrying a wrap to their own device. The wrap is **ignored**.
`find_wrapped_content_key` honours a wrap only when `owners.contains(&o.author)` — the
owner-principal chain — so an E2E KB still needs an owner action per device.

This is the same wall the first spike hit through `Rebind`, reached by a completely different
mechanism. That it appears twice, independently, is what makes it a property of the **design**
rather than of one op kind: content-key delivery is owner-rooted by construction.

### It fails on revocation

`a_member_cannot_revoke_their_own_device`: `Remove` requires `author.role == Role::Owner`, so the
member's removal op is inert. **The person who knows the laptop was stolen is precisely the person
who cannot act on it.**

`the_owner_can_revoke_one_device_without_disturbing_the_others` is the complement: the owner *can*
revoke a single device while the member's other devices survive. So per-device revocation is
representable — the gap is purely **who may author it**, which is a smaller and more tractable
problem than "the model cannot express this".

### And the membership semantics are wrong for devices

`removing_a_member_leaves_their_devices_behind_unless_cascade_is_opted_into`: under the **default**
`InviterRemovalPolicy::PendingOnly`, removing a member leaves every device they enrolled still a
member. `CascadeAll` does remove the subtree, but it is opt-in — and it is a **local, per-peer**
setting (`MembershipView`), so two peers can legitimately disagree about whether a revoked user's
devices are still members.

That default is correct for humans (don't cascade-delete a team when a manager leaves) and clearly
wrong for devices. Which is the real conclusion of Part 1: once devices are members, every
membership semantic applies to them, and several of them are wrong:

- removal does not cascade to devices by default, and cascade is per-peer rather than agreed;
- devices appear in member lists, counts and role tables as if they were people;
- the owner must wrap content keys per device;
- the member cannot revoke their own device.

**So the cheap path is rejected on evidence, not taste.** ADR-098 should model a device as
something a member *has*, not as a member.

## Part 2 — the SSSS analogue is buildable today

Matrix stores private cross-signing material server-side as ciphertext, unlocked by a user-held
recovery key. MAE needs the same shape. These tests confirm it can be built from `content_crypto`
as it already exists — XChaCha20-Poly1305 AEAD plus a sha2 derivation — with **no new
dependency**, which matters because MAE deliberately derives with sha2 rather than pulling an
`hkdf` crate (a second `digest` major would break the crypto-coherence constraint recorded in
`shared/sync/Cargo.toml`).

| Property | Test |
|---|---|
| The correct recovery key opens the sealed member secret | `a_recovery_key_sealed_member_secret_round_trips` |
| A wrong recovery key opens nothing — four distinct wrong values, including a one-byte difference | `a_wrong_recovery_key_opens_nothing` |
| Every tampered byte is rejected — exhaustive sweep over nonce, ciphertext and tag | `every_tampered_byte_of_the_sealed_blob_is_rejected` |
| A key-blind host learns nothing — byte-level canary scan, whole value *and* a 16-byte prefix | `the_sealed_blob_leaks_no_plaintext_to_a_key_blind_host` |
| The leak oracle can actually fire | `the_leak_oracle_catches_an_unencrypted_blob` |

The derivation used here is a placeholder (`sha2(domain ‖ recovery)`), deliberately domain-separated
so it cannot collide with a content key derived elsewhere. A real design must specify this properly
— Matrix uses base58 plus HKDF with a MAC-based checksum so a mistyped recovery key is caught before
it produces a wrong key. That is ADR-098's work, not this spike's.

## What ADR-098 can now take as established

1. **Devices are not members.** Evidence-backed rejection of the cheapest available design.
2. **Per-device revocation is representable; the gap is authorship.** The design problem is "let a member revoke their own device", not "make revocation expressible".
3. **Content-key delivery is owner-rooted by construction**, confirmed twice by independent routes. Any design where a member self-services devices must change that rule deliberately, with its own threat-model argument — it is not an oversight to route around.
4. **Secret storage needs no new dependency**, and its adversarial properties hold with the primitives already shipped.

## What this still does *not* establish

- **Forward secrecy on revocation is untested and is the sharpest open question.** A revoked device *already held* the content key. Revoking it does not un-share what it has, so a real revocation must also rotate the content key — which is `author_rotate_on_remove`, an owner action. Whether device revocation can avoid dragging the owner back in is unresolved, and it may be the constraint that decides the whole design.
- **Nothing about device-to-device key sharing.** Matrix gossips secrets to newly-verified devices; MAE has no such channel and this spike did not model one.
- **No recovery-key UX.** Generation, display, storage, and the mistyped-key path are untested.
- **Nothing about OIDC or AD groups.** Still the separate, still-open layer.
- **The derivation is a placeholder**, not a proposal.

## Reproducing

```bash
cargo test -p mae-sync --test device_secret_storage_and_revocation
cargo test -p mae-sync --test multi_device_identity
```

No external harness required.
