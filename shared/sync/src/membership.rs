//! Signed, hash-chained membership operations (ADR-026) — the capability-based
//! membership protocol for the P2P mesh.
//!
//! Each membership mutation (admit / remove / role-change / revoke) is an
//! **Ed25519-signed op** whose validity *any* peer derives locally, without
//! trusting a relaying daemon. The design composes prior art:
//! - **UCAN** — a grant names its issuer (`author`) + subject and carries a
//!   timebox (`expires_at`); the `can_invite` capability is a delegation.
//! - **Keybase sigchains** — ops are **hash-chained** (`prev_hash`), so any
//!   reorder/omission/forgery breaks the chain.
//! - **p2panda-auth** — an op is valid only if the author held the capability at
//!   its causal position; concurrent conflicts resolve deterministically.
//!
//! This module is the cryptographic + canonical-encoding foundation: the
//! [`MembershipOp`] struct, its deterministic [`MembershipOp::canonical_bytes`]
//! (what is signed + hashed), and sign/verify/chain. Validity *derivation*
//! (timebox, revocation, capability, the resolver) and the `KbCollectionDoc`
//! wiring build on top of this in later slices.

use crate::kb::Role;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The membership change an op performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipAction {
    /// Admit `subject` at `role` (with optional `can_invite`), by `author`.
    Admit,
    /// Remove `subject` from the KB.
    Remove,
    /// Change `subject`'s role.
    SetRole,
    /// Revoke an outstanding invite / admission for `subject`.
    Revoke,
}

impl MembershipAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MembershipAction::Admit => "admit",
            MembershipAction::Remove => "remove",
            MembershipAction::SetRole => "set_role",
            MembershipAction::Revoke => "revoke",
        }
    }
    pub fn parse(s: &str) -> Option<MembershipAction> {
        match s {
            "admit" => Some(MembershipAction::Admit),
            "remove" => Some(MembershipAction::Remove),
            "set_role" => Some(MembershipAction::SetRole),
            "revoke" => Some(MembershipAction::Revoke),
            _ => None,
        }
    }
}

/// A signed membership operation. The [`canonical_bytes`](Self::canonical_bytes)
/// are what get signed + hash-chained; the signature proves `author` authored it.
/// Validity (capability-at-epoch, timebox, revocation, cascade) is derived
/// per-peer (ADR-026), not stored as a verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipOp {
    /// The KB this op mutates.
    pub kb_id: String,
    /// What the op does.
    pub action: MembershipAction,
    /// The principal acted on (Ed25519 key fingerprint, `SHA256:…`).
    pub subject: String,
    /// The granted role (Admit / SetRole).
    pub role: Option<Role>,
    /// Whether the op grants `subject` the delegable invite capability (Admit).
    pub can_invite: bool,
    /// The issuer principal (= the `invited_by` audit field) — the fingerprint of
    /// the key that signs this op.
    pub author: String,
    /// Issue time (unix seconds) — anchors causal/timebox checks.
    pub issued_at: u64,
    /// Expiry (unix seconds); `None` = no timebox.
    pub expires_at: Option<u64>,
    /// The ADR-023 authorization **epoch** this op assigns to `subject` — *signed*,
    /// so every peer derives the *same* epoch (a per-peer random token would break
    /// convergence). A fresh grant stays at epoch 0; a role change / re-admit carries
    /// the daemon's unpredictable `fresh_epoch_token()` (#72), chosen at authoring
    /// time. Peers read this to compute `derive_kb_client_id` for the content fence.
    pub epoch: u64,
    /// Hex of the previous op's [`chain_hash`](Self::chain_hash); `""` = genesis.
    pub prev_hash: String,
}

impl MembershipOp {
    /// Deterministic canonical encoding — the exact bytes that are signed +
    /// hashed. Version-tagged + NUL-separated so it is stable across platforms
    /// and serde versions (no field-ordering ambiguity). NUL never appears in a
    /// fingerprint, role, or decimal, so the separation is unambiguous.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn field(b: &mut Vec<u8>, s: &str) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        let mut b = Vec::new();
        field(&mut b, "maememb/v1");
        field(&mut b, &self.kb_id);
        field(&mut b, self.action.as_str());
        field(&mut b, &self.subject);
        field(&mut b, self.role.map(|r| r.as_str()).unwrap_or(""));
        field(&mut b, if self.can_invite { "1" } else { "0" });
        field(&mut b, &self.author);
        field(&mut b, &self.issued_at.to_string());
        field(
            &mut b,
            &self.expires_at.map(|e| e.to_string()).unwrap_or_default(),
        );
        field(&mut b, &self.epoch.to_string());
        field(&mut b, &self.prev_hash);
        b
    }

    /// Sign with the author's Ed25519 secret seed (the daemon's own identity, for
    /// a KB it owns/manages). Returns the 64-byte signature.
    pub fn sign(&self, secret: &[u8; 32]) -> Vec<u8> {
        SigningKey::from_bytes(secret)
            .sign(&self.canonical_bytes())
            .to_bytes()
            .to_vec()
    }

    /// Verify `sig` was produced over this op by the holder of `author_pubkey`.
    /// (The caller must separately confirm `author_pubkey`'s fingerprint equals
    /// `self.author` — see [`fingerprint_matches`](Self::fingerprint_matches).)
    pub fn verify(&self, sig: &[u8], author_pubkey: &[u8; 32]) -> bool {
        let vk = match VerifyingKey::from_bytes(author_pubkey) {
            Ok(k) => k,
            Err(_) => return false,
        };
        let arr: [u8; 64] = match sig.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        vk.verify(&self.canonical_bytes(), &Signature::from_bytes(&arr))
            .is_ok()
    }

    /// True iff `pubkey`'s `SHA256:<base64>` fingerprint equals `self.author` —
    /// binds the verifying key to the claimed author principal.
    pub fn fingerprint_matches(&self, pubkey: &[u8; 32]) -> bool {
        fingerprint_of(pubkey) == self.author
    }

    /// The hash this op contributes as the *next* op's `prev_hash`:
    /// `hex(sha256(canonical_bytes ‖ sig))` — Keybase-style tamper-evident
    /// chaining (binds the signature, not just the payload).
    pub fn chain_hash(&self, sig: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(self.canonical_bytes());
        h.update(sig);
        hex::encode(h.finalize())
    }
}

/// A [`MembershipOp`] together with its signature + the author's public key — one
/// record in the collection's signed op-log (ADR-026). `author_pubkey` is stored
/// so any peer can verify the signature locally without an external lookup; it is
/// bound to `op.author` via the fingerprint check in [`verify_signed`](Self::verify_signed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedMembershipOp {
    pub op: MembershipOp,
    pub sig: Vec<u8>,
    pub author_pubkey: [u8; 32],
}

impl SignedMembershipOp {
    /// This record's hash — its key in the op-log and the `prev_hash` of any op
    /// that builds on it.
    pub fn chain_hash(&self) -> String {
        self.op.chain_hash(&self.sig)
    }

    /// Verify the record's signature **and** that the signing key belongs to the
    /// claimed author: `fingerprint_of(author_pubkey) == op.author` AND the
    /// signature verifies. This is the per-record cryptographic check; capability,
    /// timebox, revocation, and the resolver are layered on in `derive_valid_members`.
    pub fn verify_signed(&self) -> bool {
        self.op.fingerprint_matches(&self.author_pubkey)
            && self.op.verify(&self.sig, &self.author_pubkey)
    }
}

/// A member as **derived** from the signed op-log (ADR-026) — never read as a
/// stored verdict. Carries the current role, the delegable invite capability, the
/// `invited_by` provenance, and the ADR-023 authorization `epoch` (for the content
/// fence). Every honest peer derives the identical set via [`derive_valid_members`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidMember {
    pub principal: String,
    pub role: Role,
    /// Whether this member may delegate invites (UCAN `can_invite` capability).
    pub can_invite: bool,
    /// The principal that admitted this member (the owner is self-admitted).
    pub invited_by: String,
    /// ADR-023 authorization epoch assigned by the latest authorizing op.
    pub epoch: u64,
}

/// What happens to a removed inviter's downstream members (ADR-026 §A3 cascade).
/// The strong-removal resolver already invalidates a member's actions *concurrent*
/// with their removal; this per-KB policy governs members the inviter validly
/// admitted *before* their removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InviterRemovalPolicy {
    /// Keep members the inviter admitted before removal; only drop invites not yet
    /// effective. In the pure membership op-log there is no separate "pending"
    /// state (the hub `pending` map is orthogonal), so this is operationally
    /// equivalent to [`Retain`](Self::Retain) — and it is the conservative default.
    #[default]
    PendingOnly,
    /// Removing an inviter transitively removes their whole invite subtree
    /// (delegated-trust revocation): every member whose `invited_by` chain passes
    /// through a non-member is dropped.
    CascadeAll,
    /// Keep every member the inviter validly admitted before their removal.
    Retain,
}

impl InviterRemovalPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            InviterRemovalPolicy::PendingOnly => "pending_only",
            InviterRemovalPolicy::CascadeAll => "cascade_all",
            InviterRemovalPolicy::Retain => "retain",
        }
    }
    pub fn parse(s: &str) -> Option<InviterRemovalPolicy> {
        match s {
            "pending_only" => Some(InviterRemovalPolicy::PendingOnly),
            "cascade_all" => Some(InviterRemovalPolicy::CascadeAll),
            "retain" => Some(InviterRemovalPolicy::Retain),
            _ => None,
        }
    }
}

/// Local, per-peer options for deriving membership (ADR-026 §A4). The signed
/// op-log is shared and global; *this* is the self-protection a peer applies on
/// top, which never changes the log and needs no consensus:
/// - `cascade` — the inviter-removal policy (slice 2b-5);
/// - `blocklist` — principals this peer refuses to accept. A blocked principal's
///   authored ops are ignored (so they can't grant or manage — even the *owner*,
///   which severs this peer from the KB) and they are dropped from the derived
///   set. Unilateral + immediate; it only restricts what this daemon accepts.
///
/// (Quorum governance — an admin set + m-of-n co-signed removal — extends this in
/// a later slice.)
#[derive(Clone, Debug, Default)]
pub struct MembershipView {
    pub cascade: InviterRemovalPolicy,
    pub blocklist: BTreeSet<String>,
}

/// Derive the current valid membership by replaying the signed op-log against the
/// **external trust anchor** (ADR-026 §A1–A3). Pure + deterministic: every honest
/// peer that holds the same ops derives the identical map, with no coordinator and
/// without trusting a relay. An op contributes only if every check passes:
/// 1. **anchor** — the genesis op is a self-admit signed by `anchor_owner_pubkey`
///    (the owner pubkey for an owned KB / the join-ticket node-id for a joined one);
///    no valid genesis ⇒ no members;
/// 2. **signature + binding** — `verify_signed()` (sig valid AND
///    `fingerprint_of(author_pubkey) == author`);
/// 3. **capability** — the author is a current member with the needed capability
///    (owner or `can_invite` to admit; owner to change-role/remove in single-owner
///    governance);
/// 4. **attenuation** — a grant cannot exceed the author's own role;
/// 5. **timebox** — an op past `expires_at` (relative to `now`) never takes effect.
///
/// **Resolver (slice 2b-4 — p2panda-auth "strong removal", implemented verbatim,
/// NOT invented):** validity is a *fixpoint* over the op tree, so concurrent
/// conflicts resolve deterministically and identically on every peer:
/// - a **removal/revoke** of `S` invalidates `S`'s actions that are *concurrent*
///   with the removal, **transitively** (their dependents cascade out);
/// - **mutual removal** (A removes B ∥ B removes A) ⇒ **both removals apply** (a
///   removal is never undone by a concurrent counter-removal), their other
///   concurrent actions invalidated;
/// - **re-add** ⇒ valid again, but pre-removal concurrent ops stay invalid (a
///   later re-admit causally dominates the removal);
/// - **removal dominates** a concurrent role-change of the same subject;
/// - the tiebreak for a genuine same-target conflict is the **higher `chain_hash`**
///   (deterministic + Sybil-resistant) — explicitly *not* seniority or wall-clock.
///
/// Quorum governance + the local blocklist layer on in slice 2b-5b.
///
/// Uses the default [`MembershipView`] (no blocklist, `PendingOnly` cascade); call
/// [`derive_valid_members_with`] to apply a local blocklist or cascade policy.
pub fn derive_valid_members(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    now: u64,
) -> BTreeMap<String, ValidMember> {
    derive_valid_members_with(ops, anchor_owner_pubkey, now, &MembershipView::default())
}

/// As [`derive_valid_members`], with a local [`MembershipView`] (ADR-026 §A3/§A4):
/// the inviter-removal cascade policy + a per-peer blocklist. Both are local — they
/// shape only what *this* peer accepts, never the shared op-log.
pub fn derive_valid_members_with(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    now: u64,
    view: &MembershipView,
) -> BTreeMap<String, ValidMember> {
    // 1. Keep only cryptographically-valid records (sig + author↔key binding), and
    //    drop ops authored by a locally-blocked principal (this peer ignores their
    //    authority entirely — even the owner's). Indexed by chain_hash.
    let crypto: Vec<&SignedMembershipOp> = ops
        .iter()
        .filter(|o| o.verify_signed() && !view.blocklist.contains(&o.op.author))
        .collect();
    let by_hash: BTreeMap<String, &SignedMembershipOp> =
        crypto.iter().map(|o| (o.chain_hash(), *o)).collect();

    // 2. Anchor: the genesis op is a self-admit signed by the external trust root.
    let genesis_hash = match crypto.iter().find(|o| {
        o.op.prev_hash.is_empty()
            && o.op.action == MembershipAction::Admit
            && o.op.subject == o.op.author
            && &o.author_pubkey == anchor_owner_pubkey
    }) {
        Some(g) => g.chain_hash(),
        None => return BTreeMap::new(), // no trusted root ⇒ no members
    };
    let owner_principal = by_hash[&genesis_hash].op.subject.clone();

    // 3. Deterministic causal order + each op's strict ancestor set (the op tree is
    //    single-parent via prev_hash, so ancestors are a linear chain to genesis).
    let order = causal_order(&by_hash, &genesis_hash);
    let anc: BTreeMap<String, BTreeSet<String>> = order
        .iter()
        .map(|h| (h.clone(), ancestors(&by_hash, h)))
        .collect();

    // 4. Validity fixpoint. `valid` shrinks monotonically (removing an op only ever
    //    removes more), so the loop converges. An op survives iff:
    //    (b) its author held the capability in the op's *own valid causal past*; and
    //    (c) it is not a non-removal action by a member who is concurrently removed
    //        by a *valid* removal — the strong-removal rule. Removals are exempt
    //        from (c), so mutual removals both stand.
    let mut valid: BTreeSet<String> = order.iter().cloned().collect();
    loop {
        let mut next: BTreeSet<String> = BTreeSet::new();
        for h in &order {
            let op = &by_hash[h].op;
            if op.expires_at.is_some_and(|exp| now >= exp) {
                continue; // timebox — never takes effect
            }
            if *h == genesis_hash {
                next.insert(h.clone());
                continue;
            }
            // (b) capability judged on the op's valid causal past (M_P).
            let mp = membership_at(&by_hash, &order, &valid, &anc, &anc[h], &genesis_hash, now);
            if !authorized(&mp, op, &owner_principal) {
                continue;
            }
            // (c) strong concurrent removal: a non-removal op by S dies if some
            //     valid removal of S is concurrent with it (neither is the other's
            //     ancestor). Ops in the removal's past survive; a removal itself is
            //     exempt (mutual removal ⇒ both apply).
            if !is_removal(op.action) {
                let killed = order.iter().any(|rh| {
                    if rh == h || !valid.contains(rh) {
                        return false;
                    }
                    let r = &by_hash[rh].op;
                    is_removal(r.action)
                        && r.subject == op.author
                        && !anc[rh].contains(h) // op not in the removal's past …
                        && !anc[h].contains(rh) // … and removal not in op's past ⇒ concurrent
                });
                if killed {
                    continue;
                }
            }
            next.insert(h.clone());
        }
        if next == valid {
            break;
        }
        valid = next;
    }

    // 5. Build the final member map from the surviving ops (removal-dominant; a
    //    causally-later re-admit wins; higher-hash SetRole wins concurrent ties).
    let mut members = build_members(&by_hash, &order, &valid, &anc, &genesis_hash, now);

    // 6. Inviter-removal cascade (ADR-026 §A3). `CascadeAll` transitively drops any
    //    member whose inviter is no longer present — delegated-trust revocation
    //    reaching members admitted *before* the inviter's removal (the resolver
    //    already handled those admitted *concurrently* with it). The owner is
    //    self-invited, so it roots the tree and is never cascaded. `PendingOnly`
    //    (default) and `Retain` keep validly-admitted members as-is.
    if view.cascade == InviterRemovalPolicy::CascadeAll {
        loop {
            let present: BTreeSet<String> = members.keys().cloned().collect();
            let orphans: Vec<String> = members
                .values()
                .filter(|m| m.principal != owner_principal && !present.contains(&m.invited_by))
                .map(|m| m.principal.clone())
                .collect();
            if orphans.is_empty() {
                break;
            }
            for o in orphans {
                members.remove(&o);
            }
        }
    }

    // 7. Local blocklist: drop blocked principals from the derived set. Their
    //    authored ops were already ignored in step 1; this also drops a blocked
    //    principal that some *other* member admitted. Local + immediate.
    if !view.blocklist.is_empty() {
        members.retain(|p, _| !view.blocklist.contains(p));
    }
    members
}

/// Strict ancestors of `h` (its causal past, excluding `h`): walk the `prev_hash`
/// chain up to and including genesis. Single-parent links ⇒ a linear chain.
fn ancestors(by_hash: &BTreeMap<String, &SignedMembershipOp>, h: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut cur = by_hash
        .get(h)
        .map(|r| r.op.prev_hash.clone())
        .unwrap_or_default();
    while !cur.is_empty() {
        if !set.insert(cur.clone()) {
            break; // defensive cycle guard (a well-formed log is acyclic)
        }
        cur = match by_hash.get(&cur) {
            Some(r) => r.op.prev_hash.clone(),
            None => break,
        };
    }
    set
}

fn is_removal(action: MembershipAction) -> bool {
    matches!(action, MembershipAction::Remove | MembershipAction::Revoke)
}

/// Whether `op`'s author held the capability to perform it, given the membership
/// `mp` derived from the op's valid causal past. Owner or a delegated inviter may
/// `Admit` (attenuated: cannot grant above the author's own role); only the owner
/// may `SetRole`/`Remove`/`Revoke` in single-owner governance, and the owner is
/// never removable via the chain (quorum + blocklist relax this in 2b-5b).
fn authorized(mp: &BTreeMap<String, ValidMember>, op: &MembershipOp, owner: &str) -> bool {
    let author = match mp.get(&op.author) {
        Some(a) => a,
        None => return false,
    };
    match op.action {
        MembershipAction::Admit => {
            if author.role != Role::Owner && !author.can_invite {
                return false;
            }
            author.role.includes(op.role.unwrap_or(Role::Viewer))
        }
        MembershipAction::SetRole => author.role == Role::Owner && mp.contains_key(&op.subject),
        MembershipAction::Remove | MembershipAction::Revoke => {
            author.role == Role::Owner && op.subject != owner && mp.contains_key(&op.subject)
        }
    }
}

/// Membership as of one op's causal position — the final member map over just the
/// **valid ancestors** of that op (its causal past, a linear chain). Used to judge
/// capability (b). Passes the full `anc` map through so removal-dominance and
/// SetRole overlays resolve correctly within the restricted ancestor set.
#[allow(clippy::too_many_arguments)]
fn membership_at(
    by_hash: &BTreeMap<String, &SignedMembershipOp>,
    order: &[String],
    valid: &BTreeSet<String>,
    anc: &BTreeMap<String, BTreeSet<String>>,
    target_anc: &BTreeSet<String>,
    genesis_hash: &str,
    now: u64,
) -> BTreeMap<String, ValidMember> {
    let sub: BTreeSet<String> = valid.intersection(target_anc).cloned().collect();
    build_members(by_hash, order, &sub, anc, genesis_hash, now)
}

/// Construct the member map from a set of already-validated ops, in causal order.
/// Removal-dominant per p2panda-auth: a principal is present iff some valid `Admit`
/// of them is **not** overridden by a valid removal outside that admit's causal
/// past (so a concurrent/later removal wins, but a causally-later *re-admit*
/// supersedes the removal). Roles overlay causally-later valid `SetRole`s (higher
/// `chain_hash` wins concurrent ties via `order`).
fn build_members(
    by_hash: &BTreeMap<String, &SignedMembershipOp>,
    order: &[String],
    valid: &BTreeSet<String>,
    anc: &BTreeMap<String, BTreeSet<String>>,
    genesis_hash: &str,
    now: u64,
) -> BTreeMap<String, ValidMember> {
    let in_admit_past =
        |admit: &str, other: &str| anc.get(admit).map(|s| s.contains(other)).unwrap_or(false);
    let mut out: BTreeMap<String, ValidMember> = BTreeMap::new();
    for h in order {
        if !valid.contains(h) {
            continue;
        }
        let op = &by_hash[h].op;
        let is_genesis = h == genesis_hash;
        if !(is_genesis || op.action == MembershipAction::Admit) {
            continue;
        }
        let principal = op.subject.clone();
        // Is this admit overridden by a valid removal of the same principal that is
        // not in the admit's causal past? (Removal dominates concurrent/later.)
        let dominated = order.iter().any(|rh| {
            if !valid.contains(rh) {
                return false;
            }
            let r = &by_hash[rh].op;
            is_removal(r.action) && r.subject == principal && !in_admit_past(h, rh)
        });
        if dominated {
            continue;
        }
        let role = op.role.unwrap_or(if is_genesis {
            Role::Owner
        } else {
            Role::Viewer
        });
        let mut entry = ValidMember {
            principal: principal.clone(),
            role,
            can_invite: is_genesis || op.can_invite,
            invited_by: op.author.clone(),
            epoch: op.epoch,
        };
        // Overlay causally-later valid SetRoles (causal order ⇒ higher hash last).
        for sh in order {
            if !valid.contains(sh) {
                continue;
            }
            let s = &by_hash[sh].op;
            if s.action == MembershipAction::SetRole
                && s.subject == principal
                && anc.get(sh).map(|a| a.contains(h)).unwrap_or(false)
            {
                if let Some(nr) = s.role {
                    entry.role = nr;
                    entry.epoch = s.epoch;
                }
            }
        }
        // Timebox on the establishing admit.
        if op.expires_at.is_some_and(|exp| now >= exp) {
            continue;
        }
        out.insert(principal, entry); // causal order ⇒ a later re-admit wins
    }
    out
}

/// Deterministic causal (topological) order of the op tree rooted at `genesis`.
/// Each op links to exactly one parent via `prev_hash`, so the reachable set is a
/// tree; nodes are emitted parent-before-child, concurrent siblings in ascending
/// `chain_hash` order — so every honest peer replays identically regardless of the
/// order ops arrived in. Ops **not** reachable from the anchored genesis (orphans
/// with a dangling `prev_hash`, or a forged second root) are never emitted.
fn causal_order(by_hash: &BTreeMap<String, &SignedMembershipOp>, genesis: &str) -> Vec<String> {
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    let mut order: Vec<String> = Vec::new();
    loop {
        // Every not-yet-emitted node whose parent is already emitted (genesis has
        // no parent) — one causal "generation" per pass.
        let mut ready: Vec<String> = by_hash
            .keys()
            .filter(|h| !emitted.contains(*h))
            .filter(|h| h.as_str() == genesis || emitted.contains(&by_hash[*h].op.prev_hash))
            .cloned()
            .collect();
        if ready.is_empty() {
            break;
        }
        ready.sort(); // ascending chain_hash — deterministic sibling tiebreak
        for h in ready {
            emitted.insert(h.clone());
            order.push(h);
        }
    }
    order
}

/// The `SHA256:<base64>` fingerprint of an Ed25519 public key — the membership
/// **principal**. Matches `mae_mcp::identity::PublicKey::fingerprint()` so a
/// member's principal is identical whether derived here or there.
pub fn fingerprint_of(pubkey: &[u8; 32]) -> String {
    use base64::Engine;
    // MUST match `mae_mcp::identity::PublicKey::fingerprint()` exactly
    // (STANDARD_NO_PAD) so a principal is byte-identical across crates.
    let digest = Sha256::digest(pubkey);
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_op(prev_hash: &str) -> MembershipOp {
        MembershipOp {
            kb_id: "concept:x".into(),
            action: MembershipAction::Admit,
            subject: "SHA256:bob".into(),
            role: Some(Role::Editor),
            can_invite: false,
            author: "SHA256:owner".into(),
            issued_at: 1_700_000_000,
            expires_at: Some(1_700_086_400),
            epoch: 0,
            prev_hash: prev_hash.into(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let secret = [7u8; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let op = sample_op("");

        let sig = op.sign(&secret);
        assert_eq!(sig.len(), 64);
        assert!(op.verify(&sig, &pubkey), "a fresh signature must verify");
    }

    #[test]
    fn tampering_any_field_breaks_the_signature() {
        let secret = [7u8; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let op = sample_op("");
        let sig = op.sign(&secret);

        // Each mutation must invalidate the signature (canonical bytes change).
        let mut t = op.clone();
        t.role = Some(Role::Owner); // privilege escalation attempt
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.subject = "SHA256:mallory".into();
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.expires_at = None; // strip the timebox
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.can_invite = true; // self-grant the invite capability
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.epoch = 42; // bump the authorization epoch to fence a member
        assert!(!t.verify(&sig, &pubkey));
    }

    #[test]
    fn a_different_key_does_not_verify() {
        let op = sample_op("");
        let sig = op.sign(&[7u8; 32]);
        let other_pub = SigningKey::from_bytes(&[9u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(!op.verify(&sig, &other_pub), "wrong author key must fail");
        // A malformed signature also fails (not panics).
        assert!(!op.verify(b"too short", &other_pub));
    }

    #[test]
    fn fingerprint_binding_matches_mcp_format() {
        let pubkey = SigningKey::from_bytes(&[3u8; 32])
            .verifying_key()
            .to_bytes();
        let fp = fingerprint_of(&pubkey);
        assert!(fp.starts_with("SHA256:"));
        let op = MembershipOp {
            author: fp.clone(),
            ..sample_op("")
        };
        assert!(op.fingerprint_matches(&pubkey));
        // A different key's fingerprint does not match the claimed author.
        let other = SigningKey::from_bytes(&[4u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(!op.fingerprint_matches(&other));
    }

    #[test]
    fn chain_hash_is_deterministic_and_binds_signature() {
        let op = sample_op("");
        let sig = op.sign(&[7u8; 32]);
        let h1 = op.chain_hash(&sig);
        let h2 = op.chain_hash(&sig);
        assert_eq!(h1, h2, "chain hash is deterministic");
        assert_eq!(h1.len(), 64, "sha256 hex");
        // A different signature ⇒ a different chain hash (binds the sig).
        let sig2 = op.sign(&[8u8; 32]);
        assert_ne!(op.chain_hash(&sig2), h1);
        // The next op linking to this one carries h1 as prev_hash.
        let next = sample_op(&h1);
        assert_eq!(next.prev_hash, h1);
    }

    #[test]
    fn signed_record_verifies_and_binds_author_to_key() {
        let secret = [7u8; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let mut op = sample_op("");
        op.author = fingerprint_of(&pubkey); // author principal = the signer's key
        let sig = op.sign(&secret);
        let rec = SignedMembershipOp {
            op: op.clone(),
            sig: sig.clone(),
            author_pubkey: pubkey,
        };
        assert!(
            rec.verify_signed(),
            "correctly-signed, author-bound record verifies"
        );
        assert_eq!(rec.chain_hash(), op.chain_hash(&sig));

        // A record claiming a pubkey that doesn't match its `author` principal is
        // rejected even before the signature check (fingerprint binding).
        let other = SigningKey::from_bytes(&[9u8; 32])
            .verifying_key()
            .to_bytes();
        let mismatched = SignedMembershipOp {
            author_pubkey: other,
            ..rec.clone()
        };
        assert!(!mismatched.verify_signed());

        // A tampered op (with the original sig) fails the signature check.
        let mut tampered = rec.clone();
        tampered.op.role = Some(Role::Owner);
        assert!(!tampered.verify_signed());
    }

    // --- derive_valid_members (slice 2b-3) ---

    /// A test identity: signing seed, public key, and principal fingerprint.
    struct Id {
        secret: [u8; 32],
        pubkey: [u8; 32],
        fp: String,
    }
    fn id(seed: u8) -> Id {
        let secret = [seed; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let fp = fingerprint_of(&pubkey);
        Id { secret, pubkey, fp }
    }

    /// Build + sign a membership op authored by `author`, linked to `prev`.
    #[allow(clippy::too_many_arguments)]
    fn make(
        author: &Id,
        action: MembershipAction,
        subject: &str,
        role: Option<Role>,
        can_invite: bool,
        expires_at: Option<u64>,
        prev: &str,
    ) -> SignedMembershipOp {
        let op = MembershipOp {
            kb_id: "KB".into(),
            action,
            subject: subject.into(),
            role,
            can_invite,
            author: author.fp.clone(),
            issued_at: 1,
            expires_at,
            epoch: 0,
            prev_hash: prev.into(),
        };
        let sig = op.sign(&author.secret);
        SignedMembershipOp {
            op,
            sig,
            author_pubkey: author.pubkey,
        }
    }

    /// Genesis = the owner self-admit (the anchored root of every valid log).
    fn genesis(owner: &Id) -> SignedMembershipOp {
        make(
            owner,
            MembershipAction::Admit,
            &owner.fp,
            Some(Role::Owner),
            true,
            None,
            "",
        )
    }

    #[test]
    fn derive_owner_genesis_admits_the_owner() {
        let owner = id(1);
        let g = genesis(&owner);
        let m = derive_valid_members(&[g], &owner.pubkey, 100);
        assert_eq!(m.len(), 1);
        let o = &m[&owner.fp];
        assert_eq!(o.role, Role::Owner);
        assert!(o.can_invite);
        assert_eq!(o.invited_by, owner.fp);
    }

    #[test]
    fn derive_anchor_mismatch_returns_empty() {
        let owner = id(1);
        let stranger = id(2);
        let g = genesis(&owner);
        // The genesis is real + self-signed, but the verifier's anchor is a
        // DIFFERENT key (a relay shipping a self-attested collection). No root.
        let m = derive_valid_members(&[g], &stranger.pubkey, 100);
        assert!(
            m.is_empty(),
            "genesis not signed by the anchor ⇒ no members"
        );
    }

    #[test]
    fn derive_valid_inviter_chain_admits_transitively() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        // Owner admits alice as an editor WITH the invite capability.
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        // Alice (a delegated inviter) admits bob as a viewer.
        let admit_bob = make(
            &alice,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_alice.chain_hash(),
        );
        let m = derive_valid_members(&[g, admit_alice, admit_bob], &owner.pubkey, 100);
        assert_eq!(m.len(), 3);
        assert_eq!(m[&alice.fp].role, Role::Editor);
        assert!(m[&alice.fp].can_invite);
        assert_eq!(m[&bob.fp].role, Role::Viewer);
        assert_eq!(m[&bob.fp].invited_by, alice.fp, "provenance recorded");
    }

    #[test]
    fn derive_op_by_a_non_member_is_ignored() {
        let owner = id(1);
        let mallory = id(9);
        let g = genesis(&owner);
        // Mallory self-signs an admit making herself owner — but she is not a
        // member, and her op isn't the anchored genesis, so it contributes nothing.
        let forged = make(
            &mallory,
            MembershipAction::Admit,
            &mallory.fp,
            Some(Role::Owner),
            true,
            None,
            &g.chain_hash(),
        );
        let m = derive_valid_members(&[g, forged], &owner.pubkey, 100);
        assert_eq!(m.len(), 1);
        assert!(!m.contains_key(&mallory.fp));
    }

    #[test]
    fn derive_attenuation_blocks_granting_above_yourself() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        // Alice is an editor-with-invite.
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        // Alice tries to admit bob as OWNER — above her own role ⇒ rejected.
        let over_grant = make(
            &alice,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Owner),
            false,
            None,
            &admit_alice.chain_hash(),
        );
        let m = derive_valid_members(&[g, admit_alice, over_grant], &owner.pubkey, 100);
        assert!(!m.contains_key(&bob.fp), "over-attenuated grant absent");
    }

    #[test]
    fn derive_expired_op_does_not_take_effect() {
        let owner = id(1);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            Some(50), // expires at t=50
            &g.chain_hash(),
        );
        let ops = [g, admit_bob];
        // Before expiry bob is a member; at/after expiry he is not.
        assert!(derive_valid_members(&ops, &owner.pubkey, 49).contains_key(&bob.fp));
        assert!(!derive_valid_members(&ops, &owner.pubkey, 50).contains_key(&bob.fp));
    }

    #[test]
    fn derive_owner_removes_a_member() {
        let owner = id(1);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let remove_bob = make(
            &owner,
            MembershipAction::Remove,
            &bob.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let m = derive_valid_members(&[g, admit_bob, remove_bob], &owner.pubkey, 100);
        assert!(!m.contains_key(&bob.fp), "removed member dropped");
        assert!(m.contains_key(&owner.fp));
    }

    #[test]
    fn derive_inviter_cannot_remove_in_single_owner_governance() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &admit_alice.chain_hash(),
        );
        // Alice (delegated inviter, not owner) tries to remove bob — management is
        // owner-only in single-owner governance, so the removal is ignored.
        let alice_removes_bob = make(
            &alice,
            MembershipAction::Remove,
            &bob.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let m = derive_valid_members(
            &[g, admit_alice, admit_bob, alice_removes_bob],
            &owner.pubkey,
            100,
        );
        assert!(m.contains_key(&bob.fp), "non-owner removal has no effect");
    }

    #[test]
    fn derive_owner_is_not_removable_via_the_chain() {
        let owner = id(1);
        let g = genesis(&owner);
        // Even an owner-authored remove of the owner is a no-op (single-owner).
        let remove_owner = make(
            &owner,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &g.chain_hash(),
        );
        let m = derive_valid_members(&[g, remove_owner], &owner.pubkey, 100);
        assert!(m.contains_key(&owner.fp), "owner survives a remove op");
    }

    #[test]
    fn derive_orphan_op_with_dangling_prev_is_excluded() {
        let owner = id(1);
        let bob = id(3);
        let g = genesis(&owner);
        // An admit whose prev_hash points nowhere is unreachable from genesis.
        let orphan = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            "deadbeefdeadbeef",
        );
        let m = derive_valid_members(&[g, orphan], &owner.pubkey, 100);
        assert!(!m.contains_key(&bob.fp), "orphan op never applied");
    }

    #[test]
    fn derive_is_independent_of_input_order() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        let admit_bob = make(
            &alice,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_alice.chain_hash(),
        );
        let forward = derive_valid_members(
            &[g.clone(), admit_alice.clone(), admit_bob.clone()],
            &owner.pubkey,
            100,
        );
        // Same ops, reversed arrival order ⇒ identical derived state.
        let reversed = derive_valid_members(&[admit_bob, admit_alice, g], &owner.pubkey, 100);
        assert_eq!(forward, reversed);
    }

    // --- strong-removal resolver (slice 2b-4) ---
    //
    // NOTE: mutual removal (A removes B ∥ B removes A ⇒ both apply) needs two
    // members with removal authority, which single-owner governance does not allow
    // (only the owner removes, and the owner is irrevocable via the chain). That
    // oracle lives with the quorum-governance slice (2b-5b), where multiple admins
    // exist. The cases below are all expressible under single-owner governance.

    #[test]
    fn strong_removal_invalidates_concurrent_actions_transitively() {
        let owner = id(1);
        let alice = id(2);
        let x = id(3);
        let y = id(4);
        let g = genesis(&owner);
        // Owner admits alice as an editor WITH the invite capability.
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        // CONCURRENT branches off admit_alice:
        //  (1) alice admits x (and delegates can_invite to x);
        //  (2) owner removes alice.
        let admit_x = make(
            &alice,
            MembershipAction::Admit,
            &x.fp,
            Some(Role::Editor),
            true,
            None,
            &admit_alice.chain_hash(),
        );
        let remove_alice = make(
            &owner,
            MembershipAction::Remove,
            &alice.fp,
            None,
            false,
            None,
            &admit_alice.chain_hash(),
        );
        // x (had it been valid) admits y — causally after admit_x.
        let admit_y = make(
            &x,
            MembershipAction::Admit,
            &y.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_x.chain_hash(),
        );
        let ops = [g, admit_alice, admit_x, remove_alice, admit_y];
        let m = derive_valid_members(&ops, &owner.pubkey, 100);
        assert!(m.contains_key(&owner.fp), "owner stays");
        assert!(!m.contains_key(&alice.fp), "alice removed");
        assert!(
            !m.contains_key(&x.fp),
            "x's admission is concurrent with alice's removal ⇒ invalidated"
        );
        assert!(
            !m.contains_key(&y.fp),
            "y depends on x's invalidated admission ⇒ transitive cascade"
        );

        // Determinism: the same ops in a different arrival order derive identically.
        let mut shuffled = ops.to_vec();
        shuffled.rotate_left(2);
        shuffled.reverse();
        assert_eq!(derive_valid_members(&shuffled, &owner.pubkey, 100), m);
    }

    #[test]
    fn re_add_restores_member_but_old_concurrent_ops_stay_fenced() {
        let owner = id(1);
        let bob = id(2);
        let carol = id(3);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        // CONCURRENT off admit_bob: bob admits carol ∥ owner removes bob.
        let admit_carol = make(
            &bob,
            MembershipAction::Admit,
            &carol.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let remove_bob = make(
            &owner,
            MembershipAction::Remove,
            &bob.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        // Owner re-admits bob AFTER the removal (causally dominates it).
        let readd_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Viewer),
            false,
            None,
            &remove_bob.chain_hash(),
        );
        let m = derive_valid_members(
            &[g, admit_bob, admit_carol, remove_bob, readd_bob],
            &owner.pubkey,
            100,
        );
        assert!(m.contains_key(&bob.fp), "bob re-added");
        assert_eq!(m[&bob.fp].role, Role::Viewer, "re-add sets the new role");
        assert!(
            !m.contains_key(&carol.fp),
            "carol's pre-removal concurrent admission stays fenced after re-add"
        );
    }

    #[test]
    fn concurrent_set_role_resolves_by_higher_chain_hash() {
        let owner = id(1);
        let bob = id(2);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        // Two CONCURRENT role changes of bob (owner-authored), to distinct roles.
        let set_viewer = make(
            &owner,
            MembershipAction::SetRole,
            &bob.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let set_owner = make(
            &owner,
            MembershipAction::SetRole,
            &bob.fp,
            Some(Role::Owner),
            false,
            None,
            &admit_bob.chain_hash(),
        );
        // The higher chain_hash deterministically wins (not seniority/clock).
        let expected = if set_owner.chain_hash() > set_viewer.chain_hash() {
            Role::Owner
        } else {
            Role::Viewer
        };
        let m = derive_valid_members(
            &[
                g.clone(),
                admit_bob.clone(),
                set_viewer.clone(),
                set_owner.clone(),
            ],
            &owner.pubkey,
            100,
        );
        assert_eq!(m[&bob.fp].role, expected, "higher-hash SetRole wins");

        // Independent of arrival order.
        let m2 = derive_valid_members(&[set_owner, g, set_viewer, admit_bob], &owner.pubkey, 100);
        assert_eq!(m2[&bob.fp].role, expected);
    }

    #[test]
    fn removal_dominates_a_concurrent_role_change() {
        let owner = id(1);
        let bob = id(2);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        // Concurrent: owner demotes bob ∥ owner removes bob. Removal must win.
        let set_viewer = make(
            &owner,
            MembershipAction::SetRole,
            &bob.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let remove_bob = make(
            &owner,
            MembershipAction::Remove,
            &bob.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let m = derive_valid_members(&[g, admit_bob, set_viewer, remove_bob], &owner.pubkey, 100);
        assert!(
            !m.contains_key(&bob.fp),
            "removal dominates the concurrent role change"
        );
    }

    // --- inviter-removal cascade policy (slice 2b-5) ---

    #[test]
    fn inviter_removal_cascade_policy_governs_pre_removal_subtree() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let carol = id(4);
        let g = genesis(&owner);
        // Linear chain — each admit is causally BEFORE alice's removal (so the
        // strong-removal resolver keeps them; only the cascade policy decides).
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        let admit_bob = make(
            &alice,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            true, // bob may invite further (depth-2 subtree)
            None,
            &admit_alice.chain_hash(),
        );
        let admit_carol = make(
            &bob,
            MembershipAction::Admit,
            &carol.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let remove_alice = make(
            &owner,
            MembershipAction::Remove,
            &alice.fp,
            None,
            false,
            None,
            &admit_carol.chain_hash(),
        );
        let ops = [g, admit_alice, admit_bob, admit_carol, remove_alice];

        // PendingOnly (default) + Retain: bob & carol were validly admitted before
        // alice's removal, so they stay; only alice is gone.
        for policy in [
            InviterRemovalPolicy::PendingOnly,
            InviterRemovalPolicy::Retain,
        ] {
            let view = MembershipView {
                cascade: policy,
                ..Default::default()
            };
            let m = derive_valid_members_with(&ops, &owner.pubkey, 100, &view);
            assert!(!m.contains_key(&alice.fp), "{policy:?}: alice removed");
            assert!(m.contains_key(&bob.fp), "{policy:?}: bob retained");
            assert!(m.contains_key(&carol.fp), "{policy:?}: carol retained");
        }
        // The 3-arg entry point uses the default (PendingOnly).
        assert!(derive_valid_members(&ops, &owner.pubkey, 100).contains_key(&bob.fp));

        // CascadeAll: removing alice transitively drops her whole invite subtree.
        let cascade = derive_valid_members_with(
            &ops,
            &owner.pubkey,
            100,
            &MembershipView {
                cascade: InviterRemovalPolicy::CascadeAll,
                ..Default::default()
            },
        );
        assert!(!cascade.contains_key(&alice.fp));
        assert!(
            !cascade.contains_key(&bob.fp),
            "cascade drops direct invitee"
        );
        assert!(
            !cascade.contains_key(&carol.fp),
            "cascade is transitive through the subtree"
        );
        assert!(cascade.contains_key(&owner.fp), "owner is never cascaded");
    }

    // --- local blocklist (slice 2b-5b) ---

    #[test]
    fn local_blocklist_drops_a_directly_admitted_member() {
        let owner = id(1);
        let bob = id(2);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let ops = [g, admit_bob];
        let view = MembershipView {
            blocklist: BTreeSet::from([bob.fp.clone()]),
            ..Default::default()
        };
        let m = derive_valid_members_with(&ops, &owner.pubkey, 100, &view);
        assert!(m.contains_key(&owner.fp), "owner unaffected");
        assert!(
            !m.contains_key(&bob.fp),
            "a blocked member is dropped even when admitted by the owner"
        );
        // The block is purely local — unblocked, bob is a member.
        assert_eq!(derive_valid_members(&ops, &owner.pubkey, 100).len(), 2);
    }

    #[test]
    fn local_blocklist_ignores_a_blocked_inviters_downstream() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        let admit_bob = make(
            &alice,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Viewer),
            false,
            None,
            &admit_alice.chain_hash(),
        );
        let ops = [g, admit_alice, admit_bob];
        let view = MembershipView {
            blocklist: BTreeSet::from([alice.fp.clone()]),
            ..Default::default()
        };
        let m = derive_valid_members_with(&ops, &owner.pubkey, 100, &view);
        assert!(m.contains_key(&owner.fp));
        assert!(!m.contains_key(&alice.fp), "blocked principal dropped");
        assert!(
            !m.contains_key(&bob.fp),
            "a blocked inviter's authored admits are ignored ⇒ their invitees vanish"
        );
    }

    #[test]
    fn local_blocklist_can_block_even_the_owner() {
        let owner = id(1);
        let bob = id(2);
        let g = genesis(&owner);
        let admit_bob = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let ops = [g, admit_bob];
        // Blocking the owner severs this peer from the KB: the owner-authored
        // genesis is ignored, so there is no trust root ⇒ empty local view.
        let view = MembershipView {
            blocklist: BTreeSet::from([owner.fp.clone()]),
            ..Default::default()
        };
        let m = derive_valid_members_with(&ops, &owner.pubkey, 100, &view);
        assert!(m.is_empty(), "blocking the owner collapses the local view");
        // Purely local: without the block, both are members.
        assert_eq!(derive_valid_members(&ops, &owner.pubkey, 100).len(), 2);
    }
}
