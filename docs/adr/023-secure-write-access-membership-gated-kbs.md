# ADR-023: Secure write-access for membership-gated KBs (epoch-fenced rebase)

**Status:** Accepted (design). Core enforcement implemented; unpredictable-token hardening + the
ADR-021 audit log are follow-ups.
**Extends:** ADR-017 (mTLS-as-identity), ADR-018 (identity-anchored RBAC access control), ADR-020
(replicated KB CRDT artifact), ADR-022 (SV-reconcile on (re)join — the cascade vector).

## Context (B-19)

ADR-018 gates KB node writes at the daemon on the member's **current** role (`kb_access(KbOp::Edit)`),
and ADR-020 ships node edits as **opaque, client-authored** yrs updates the daemon merges blindly. The
daemon has **no per-op attribution** and **no role-at-authorship binding**. Consequence (B-19, found
reasoning through the live T7 role test):

> A member who edits a node while a **viewer** applies the op to their **local** `crdt_doc` (the editor
> always applies locally) and is denied at the daemon. The op survives in their local lineage
> (local-ahead of the hub). When the member is later granted **editor**, the ADR-022 reconcile pushes
> their local-ahead, the daemon's *current-role* gate now passes, and **all accumulated viewer-era edits
> silently cascade to every peer** — a deferred privilege escalation the owner never sees at grant time.

**MAE is open-source — the client must be assumed hostile.** Client-side "revert-on-reject" is therefore
security theatre: a modified client simply keeps the local-ahead ops and lets the reconcile launder them.
Enforcement must be **daemon-side** and must hold against a malicious client.

### Why not capability-signed operations

A natural idea is to sign each op and stamp the grant it was authored under, so peers verify the author
had edit rights. A **naive** version fails against a malicious client: the client controls the stamp and
**backdates** — it authors edit `E` as a viewer, then once granted editor re-stamps `E` as "authored
under grant V" and signs it with its own (legitimately held) key. The verifier sees a valid signature +
a valid editor grant and accepts. Only **causal-hash-DAG anchoring** (each op references the *hashes* of
its causal predecessors; a grant is a node in that DAG; an op is valid iff it cryptographically descends
from a grant and precedes any revocation — Kleppmann's "local-first access control", Matrix's auth-DAG)
defeats backdating, because the client cannot fabricate a hash-history where its viewer-era op follows a
grant it had not received. That anchoring is research-grade and is deferred.

## Decision

Adopt a **server-authoritative** model (the daemon is the sole canonical authority; no client lineage is
blindly trusted), realized by **epoch-fenced rebase** — chosen over re-stamping (which degrades concurrent
same-node edits to last-writer-wins) and full hosted-edit (which removes offline editing for protected
KBs), because it **preserves the character-level CRDT merge + offline editing** validated in T4/T5 for
*continuously-authorized* editors.

**Invariant:** *no write is ever applied except as a fresh op authored under the member's CURRENT
authorization; a member's pre-grant divergent lineage can never be accepted — it must be discarded
(rebase) and any kept content re-authored as explicit, current-authorized, auditable edits.*

**Mechanism (three parts):**

1. **Per-member authorization epoch** on the collection doc (`kbc:`): a monotonic counter the **daemon**
   advances **only when an *existing* member's role actually changes** (the B-19 vector, e.g.
   viewer→editor). A *fresh* grant (a new member, owner seed, approve of a previously-denied pending peer)
   has **no prior write-capable lineage to fence**, so it stays at **epoch 0** — which is exactly what
   lets owners and directly-added editors author under the legacy/base (epoch-0) client_id with **no
   editor-side epoch sync required**, so the validated T1–T7 epoch-0 flows cannot regress. Membership ops
   are already daemon-authored, so the epoch is **unforgeable by the client**. Members read their epoch
   from `kbc:` on join and on every membership broadcast. (Epoch is not persisted across remove/re-add —
   monotonicity there is the documented hardening follow-up.)

2. **Epoch-rotated KB client_id:** `derive_kb_client_id(fingerprint, epoch)`. The editor authors node ops
   under its **current-epoch** client_id. A role change rotates the client_id; a continuously-authorized
   editor's epoch is stable, so its client_id never rotates → full CRDT merge + offline preserved.
   Viewer-era ops are under the *old-epoch* client_id.

3. **Daemon enforcement** in `kb/node_update` (after the existing `KbOp::Edit` gate): decode the update
   (`yrs::Update::decode_v1`), compute the **new** ops (beyond the daemon's node SV), and **reject unless
   every new op is authored by the sender's current-epoch client_id** `C_now = derive_kb_client_id(fp,
   epoch_now)`. Reject (`"rebase required"`) if the member's epoch advanced since their last accepted write
   or any new op is from a stale client_id — exactly the viewer-era-lineage cascade. The ADR-022
   reconcile/local-ahead push routes through this same gate, closing the cascade vector there too.

**Rebase flow (daemon-enforced, client-untrusted):** on `"rebase required"` the daemon sends authoritative
state (ADR-022 path); the editor **adopts** it (`adopt_remote_node` — discards its divergent pre-grant
ops) and re-authors any kept edits under `C_now`. A malicious client that *re-sends* the divergent update
is rejected again (its new ops are still from the stale client_id). Re-making content under `C_now` is
just an editor making current edits (allowed + auditable) — the dangerous property, silent bulk laundering
of a pre-grant lineage, is gone.

## Adversarial exploit-path review

- **Backdating via signing** → why capability-signing was rejected (above).
- **Re-send the divergent update after grant** → rejected: the new ops are from the stale (old-epoch)
  client_id, not `C_now`.
- **Offline-authored, never-submitted viewer edits** → rejected on rejoin: epoch advanced ⇒ stale
  client_id ⇒ fenced. (A naive "denied-op watermark" misses this case — the daemon never saw/denied the
  ops, so a watermark wouldn't fence them; the epoch fence does.)
- **Pre-rotation attack** (sophisticated): a malicious client *pre-computes* `derive(fp, E+1)` and authors
  its viewer-era ops under the *future* editor epoch's client_id, so on grant they appear as `C_now`.
  **Defense: the epoch token must be daemon-issued and UNPREDICTABLE** (a random nonce / state-hash the
  client cannot precompute), so the future-epoch client_id is unknowable until granted. The **core uses a
  predictable counter** (kills the honest + naive-malicious cascade); the unpredictable token is the
  documented hardening pass.
- **Concurrent grant/revoke, multi-node, migration of pre-feature ops** → hardening. The new-ops check
  grandfathers existing canonical ops (only ops *beyond the daemon SV* are fenced), so pre-feature KBs
  keep working; a member's epoch initializes on first post-feature interaction.
- **Audit (ADR-021):** every accepted write / rejection / epoch bump becomes an append-only hash-chained
  record, so even allowed post-grant edits are attributable and reviewable by the owner.

## Consequences

- **Crash-safe + secure by construction:** the daemon never applies a member's pre-grant lineage; a
  granted editor can only contribute fresh, current-epoch, auditable ops. The silent cascade is closed at
  the daemon — independent of client behaviour.
- **No regression for honest continuous editors:** a stable-epoch editor's client_id never rotates, so
  T4/T5 concurrent merge + offline editing are unchanged.
- **Cost:** a role-toggled member loses *unsynced divergent* edits (must adopt + redo) — acceptable at a
  security boundary, and rare. Membership changes now carry an epoch; KB client_id derivation takes an
  epoch arg (back-compatible: epoch 0 = legacy).
- **Reviewer guardrail:** a `kb/node_update` accepted when the member's epoch advanced and the update
  carries stale-client_id ops is a B-19 regression — reject it.

## Verification

Unit: epoch bump on role change; `derive_kb_client_id(fp, epoch)` rotates + stays 53-bit (B-17). E2E
(`daemon/tests/collab_e2e.rs`): `viewer_era_edits_do_not_cascade_on_grant` — viewer edits (denied) →
promoted to editor → pushes local-ahead → **daemon rejects the pre-grant ops; the owner's value is
unchanged; only a fresh post-grant edit is accepted** (red before the fix, green after). Malicious-client
variant: re-send the divergent update post-rebase → stays rejected. Live (optional): re-run T7 + the
exploit on two machines → confirm no silent cascade.
