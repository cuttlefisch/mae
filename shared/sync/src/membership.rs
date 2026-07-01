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
    /// Set the KB's [`Governance`] (ADR-026 §A4). Owner-authored; `subject` carries
    /// the governance spec (see [`Governance::to_spec`]), not a principal. Inert to
    /// membership derivation (it admits/removes no one — `build_members` skips
    /// non-`Admit` ops); read separately by [`derive_governance`].
    SetGovernance,
    /// Set the KB's content-encryption mode (ADR-039 F2). Owner-authored; `subject`
    /// carries the mode (`"e2e"`). Putting the mode in the SIGNED log (not the unsigned
    /// collection flag) stops a relay downgrading `e2e→none` to coax a victim into
    /// emitting plaintext. Inert to membership derivation; read by [`derive_encryption`],
    /// which is monotonic (`e2e` is one-way — no downgrade).
    SetEncryption,
    /// Identity key rotation (ADR-040). The holder of `author` (the OLD key) cross-signs a
    /// successor: `subject` = the NEW key's fingerprint, and the op carries the new key's
    /// published Ed25519 (`new_pubkey`) + X25519 wrap (`new_wrap_pubkey`, ADR-041/I1) keys.
    /// Honored only if `author` is a current member at the op's causal point (you can only
    /// rotate an identity you hold). On derivation the successor inherits the predecessor's
    /// EXACT role/epoch/invited_by/can_invite and the predecessor is retired (its post-rebind
    /// ops stop being honored) — additive, no history rewrite. The owner separately re-wraps
    /// the content key to `new_wrap_pubkey`. NOT compromise-recovery (the old key signs, so a
    /// thief could too) — that path is owner-eviction + re-join (ADR-040 §fork).
    Rebind,
    /// Register a pre-shared **recovery key** for `author` (ADR-040 §Recovery-key, v2). The
    /// principal records — signed by its PRIMARY key while uncompromised — the public key of a
    /// second, offline Ed25519 keypair (`recovery_pubkey`) authorized to rotate it later. The
    /// op self-registers: `subject == author` (you register your OWN recovery key). Inert to the
    /// member map; read by the derive's recovery registry so a `Rebind` for this principal signed
    /// by `recovery_pubkey` is honored even when the primary is lost/compromised (the attacker
    /// who holds only the primary cannot forge it — they don't have the offline recovery key).
    /// Latest-wins per principal (a new registration supersedes a leaked recovery key).
    RegisterRecoveryKey,
}

impl MembershipAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MembershipAction::Admit => "admit",
            MembershipAction::Remove => "remove",
            MembershipAction::SetRole => "set_role",
            MembershipAction::Revoke => "revoke",
            MembershipAction::SetGovernance => "set_governance",
            MembershipAction::SetEncryption => "set_encryption",
            MembershipAction::Rebind => "rebind",
            MembershipAction::RegisterRecoveryKey => "register_recovery_key",
        }
    }
    pub fn parse(s: &str) -> Option<MembershipAction> {
        match s {
            "admit" => Some(MembershipAction::Admit),
            "remove" => Some(MembershipAction::Remove),
            "set_role" => Some(MembershipAction::SetRole),
            "revoke" => Some(MembershipAction::Revoke),
            "set_governance" => Some(MembershipAction::SetGovernance),
            "set_encryption" => Some(MembershipAction::SetEncryption),
            "rebind" => Some(MembershipAction::Rebind),
            "register_recovery_key" => Some(MembershipAction::RegisterRecoveryKey),
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
    /// ADR-037 §D2: on an `Admit`, the per-KB content key **wrapped to `subject`'s
    /// key** (`content_crypto::wrap_to_member`), so the signed log itself delivers the
    /// content key to a new member — no key server. `None` for unencrypted KBs and
    /// non-Admit ops. When `Some`, the op uses the `maememb/v2` canonical encoding (the
    /// wrapped key is part of the signed bytes); `None` keeps the byte-identical `v1`
    /// encoding so every pre-encryption op still verifies.
    pub wrapped_key: Option<Vec<u8>>,
    /// ADR-040 §1: on a `Rebind`, the successor's published **Ed25519** key (`subject` is
    /// its fingerprint). Carried so peers learn the new node-id and the owner can bind the
    /// re-wrap; redundant-but-explicit (the fingerprint already commits to it). `Some` only
    /// on `Rebind` ops, which use the `maememb/v3` canonical encoding (both rebind keys are
    /// part of the signed bytes). `None` everywhere else keeps `v1`/`v2` byte-identical.
    pub new_pubkey: Option<[u8; 32]>,
    /// ADR-040 §3 + ADR-041/I1: on a `Rebind`, the successor's published **X25519 wrap** key
    /// — what the owner seals the current content key to so the new identity can decrypt
    /// (the wrap key is NOT derivable from the fingerprint, so it MUST be published here).
    /// `Some` only on `Rebind` ops (`maememb/v3`).
    pub new_wrap_pubkey: Option<[u8; 32]>,
    /// ADR-040 §Recovery-key (v2): on a `RegisterRecoveryKey` op, the **Ed25519 public key of
    /// the offline recovery keypair** `author` authorizes to rotate it. `Some` only on
    /// `RegisterRecoveryKey` ops, which use the `maememb/v4` canonical encoding (the recovery
    /// key is part of the signed bytes). `None` everywhere else keeps `v1`/`v2`/`v3`
    /// byte-identical.
    pub recovery_pubkey: Option<[u8; 32]>,
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
        // Versioned for backward compatibility. A `Rebind` op (ADR-040) is `v3` and appends
        // both new keys; a `wrapped_key`-bearing op (ADR-037) is `v2` and appends the wrapped
        // key; an op without either emits BYTE-IDENTICAL `v1` bytes, so every signature
        // created before encryption/rotation still verifies. (Rebind never carries a
        // wrapped_key, and only Rebind carries the new keys, so the three are disjoint.)
        let version = if self.action == MembershipAction::Rebind {
            "maememb/v3"
        } else if self.action == MembershipAction::RegisterRecoveryKey {
            "maememb/v4"
        } else if self.wrapped_key.is_some() {
            "maememb/v2"
        } else {
            "maememb/v1"
        };
        field(&mut b, version);
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
        if self.action == MembershipAction::Rebind {
            // Both new keys are part of the signed bytes — empty string for a malformed
            // Rebind missing one (it will fail to be honored in derivation regardless).
            field(
                &mut b,
                &self.new_pubkey.map(hex::encode).unwrap_or_default(),
            );
            field(
                &mut b,
                &self.new_wrap_pubkey.map(hex::encode).unwrap_or_default(),
            );
        } else if self.action == MembershipAction::RegisterRecoveryKey {
            // The recovery key is part of the signed bytes (so the primary commits to it).
            field(
                &mut b,
                &self.recovery_pubkey.map(hex::encode).unwrap_or_default(),
            );
        } else if let Some(wk) = &self.wrapped_key {
            field(&mut b, &hex::encode(wk));
        }
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

/// Per-KB governance for *who* can remove a member, and how many it takes
/// (ADR-026 §A4). Global (agreed by all peers via owner-signed state), unlike the
/// local [`MembershipView`]. The admin set is the members at [`Role::Owner`];
/// members are uniform — no creator immunity — so under quorum even the founding
/// owner is removable by enough co-signatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Governance {
    /// One irrevocable owner; only the owner manages. The default (matches v0.14).
    #[default]
    SingleOwner,
    /// Removing any admin/owner needs `threshold` **distinct** admin co-signatures
    /// (each a `Revoke`/`Remove` op for the same target); a lone compromised admin
    /// cannot unilaterally remove another. `m`-of-`n` over the `Role::Owner` set.
    Quorum { threshold: usize },
}

impl Governance {
    /// Canonical spec string carried in a `SetGovernance` op's `subject` field (so
    /// it is signed + hash-chained like any op): `single-owner` | `quorum:N`.
    pub fn to_spec(self) -> String {
        match self {
            Governance::SingleOwner => "single-owner".to_string(),
            Governance::Quorum { threshold } => format!("quorum:{threshold}"),
        }
    }

    /// Parse a `SetGovernance` spec. `quorum:N` requires `N >= 1` (a 0 threshold is
    /// meaningless and rejected); `quorum:1` is exactly single-owner-removal and is
    /// accepted as `Quorum{1}` (the tally generalizes it). Unknown ⇒ `None`.
    pub fn parse_spec(s: &str) -> Option<Governance> {
        let s = s.trim();
        if s == "single-owner" {
            return Some(Governance::SingleOwner);
        }
        if let Some(n) = s.strip_prefix("quorum:") {
            let threshold: usize = n.parse().ok()?;
            if threshold == 0 {
                return None;
            }
            return Some(Governance::Quorum { threshold });
        }
        None
    }
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

/// ADR-040 §Recovery-key — the per-principal registered **recovery key**, derived from
/// crypto-valid (PRIMARY-signed) `RegisterRecoveryKey` self-registrations. Latest-wins per
/// principal by `(issued_at, chain_hash)` so a fresh registration deterministically
/// supersedes a leaked recovery key. Built from the `verify_signed` set — only the holder of
/// the principal's *primary* key can register/replace its recovery key, so a forger cannot.
fn build_recovery_registry(ops: &[SignedMembershipOp]) -> BTreeMap<String, [u8; 32]> {
    let mut best_rank: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut keys: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    for o in ops {
        if o.op.action != MembershipAction::RegisterRecoveryKey
            || o.op.author != o.op.subject
            || !o.verify_signed()
        {
            continue;
        }
        let Some(rpk) = o.op.recovery_pubkey else {
            continue;
        };
        let rank = (o.op.issued_at, o.chain_hash());
        if best_rank
            .get(&o.op.subject)
            .map(|c| rank > *c)
            .unwrap_or(true)
        {
            best_rank.insert(o.op.subject.clone(), rank);
            keys.insert(o.op.subject.clone(), rpk);
        }
    }
    keys
}

/// ADR-040 §Recovery-key — a `Rebind` whose record is signed NOT by the rotating principal's
/// primary (so [`SignedMembershipOp::verify_signed`] is false — the signing key's fingerprint
/// is not `op.author`) but by that principal's **registered recovery key** `R`. Honored so a
/// principal that LOST/compromised its primary can still rotate to a fresh key, while a forger
/// holding neither key cannot (and one holding only the primary gains nothing — it can rotate
/// directly). The successor still passes the standard `Rebind` validity (member, fresh,
/// fingerprint-bound) in [`authorized`].
fn is_recovery_signed_rebind(
    o: &SignedMembershipOp,
    registry: &BTreeMap<String, [u8; 32]>,
) -> bool {
    o.op.action == MembershipAction::Rebind
        && registry.get(&o.op.author) == Some(&o.author_pubkey)
        && o.op.verify(&o.sig, &o.author_pubkey)
}

/// The cryptographically-honored op set: every `verify_signed` record PLUS recovery-key-signed
/// `Rebind`s (ADR-040 §Recovery-key). The single filter every `derive_*` reader shares so the
/// recovery path is honored uniformly (membership, governance, encryption, key delivery).
fn crypto_valid(ops: &[SignedMembershipOp]) -> Vec<&SignedMembershipOp> {
    let registry = build_recovery_registry(ops);
    ops.iter()
        .filter(|o| o.verify_signed() || is_recovery_signed_rebind(o, &registry))
        .collect()
}

/// ADR-040 §Recovery-key — the registered recovery-key map (principal fingerprint → recovery
/// Ed25519 pubkey) derived from `ops`. Exposed so an *authorization point* outside the deriving
/// readers (the daemon's `kb/collection_op` write gate) can honor a recovery-signed `Rebind`
/// against the recovery keys a collection's existing op-log already registers. Latest-registration
/// wins per principal, and only PRIMARY-signed registrations count — see [`build_recovery_registry`].
pub fn recovery_registry(ops: &[SignedMembershipOp]) -> BTreeMap<String, [u8; 32]> {
    build_recovery_registry(ops)
}

/// ADR-040 §Recovery-key — whether `op` is a `Rebind` validly signed by the recovery key that
/// `registry` records for `op.author` (the rotating principal). Pair with [`recovery_registry`]
/// (built from the *pre-existing* op-log) at an authorization point so a peer/daemon honors a
/// recovery rotation the same way [`crypto_valid`] does, without re-deriving the member set.
pub fn is_recovery_rebind(op: &SignedMembershipOp, registry: &BTreeMap<String, [u8; 32]>) -> bool {
    is_recovery_signed_rebind(op, registry)
}

/// As [`derive_valid_members`], with a local [`MembershipView`] (ADR-026 §A3/§A4)
/// under single-owner governance.
pub fn derive_valid_members_with(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    now: u64,
    view: &MembershipView,
) -> BTreeMap<String, ValidMember> {
    derive_valid_members_governed(ops, anchor_owner_pubkey, now, Governance::default(), view)
}

/// As [`derive_valid_members_with`], with explicit [`Governance`] (global) +
/// [`MembershipView`] (local). Under [`Governance::Quorum`], removing an
/// admin/owner requires the threshold of distinct admin co-signatures, so a single
/// compromised admin cannot unilaterally remove others; the strong-removal resolver
/// settles concurrency. The removal tally generalizes single-owner exactly
/// (threshold 1 = the sole owner's removal).
pub fn derive_valid_members_governed(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    now: u64,
    governance: Governance,
    view: &MembershipView,
) -> BTreeMap<String, ValidMember> {
    // 1. Keep only cryptographically-valid records (sig + author↔key binding) — PLUS
    //    recovery-key-signed Rebinds (ADR-040 §Recovery-key) — and drop ops authored by a
    //    locally-blocked principal (this peer ignores their authority entirely — even the
    //    owner's). Indexed by chain_hash.
    let recovery_registry = build_recovery_registry(ops);
    let crypto: Vec<&SignedMembershipOp> = ops
        .iter()
        .filter(|o| {
            (o.verify_signed() || is_recovery_signed_rebind(o, &recovery_registry))
                && !view.blocklist.contains(&o.op.author)
        })
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
    // Safety bound: the fixpoint converges in a few passes for well-formed logs.
    // Quorum tallies create a theoretical re-validation path, so cap iterations to
    // keep termination + determinism (same inputs ⇒ same passes ⇒ same result on
    // every peer regardless of convergence).
    let max_iter = order.len() + 2;
    for _ in 0..max_iter {
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
            let mp = membership_at(
                &by_hash,
                &order,
                &valid,
                &anc,
                &anc[h],
                &genesis_hash,
                governance,
                now,
            );
            if !authorized(&mp, op, &owner_principal, governance) {
                continue;
            }
            // (c) strong concurrent removal: a non-removal op by S dies if S is
            //     *effectively* removed (≥threshold distinct admin co-signatures
            //     under quorum; one owner removal under single-owner) and that
            //     effective removal is concurrent with the op. Ops in the removal's
            //     causal past survive; removals themselves are exempt (mutual
            //     removal ⇒ both apply).
            if !is_removal(op.action) {
                if let Some(rh) =
                    effective_removal(&op.author, governance, &by_hash, &order, &valid)
                {
                    if !anc[&rh].contains(h) && !anc[h].contains(&rh) {
                        continue; // concurrent with the effective removal ⇒ killed
                    }
                }
            }
            next.insert(h.clone());
        }
        let stable = next == valid;
        valid = next;
        if stable {
            break;
        }
    }

    // 5. Build the final member map from the surviving ops (removal-dominant; a
    //    causally-later re-admit wins; higher-hash SetRole wins concurrent ties).
    let mut members = build_members(
        &by_hash,
        &order,
        &valid,
        &anc,
        &genesis_hash,
        governance,
        now,
    );

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

/// The `chain_hash` of the removal op that makes `target` *effectively* removed
/// under `governance` — the op at which the count of **distinct** admin authors who
/// validly removed `target` reaches the threshold, in causal order — or `None` if
/// not (yet) removed. `SingleOwner` ⇒ threshold 1 (the sole owner's removal; this
/// reduces to the per-op rule). `Quorum{m}` ⇒ the m-th distinct admin's
/// co-signature. Valid removal ops are admin-authored (gated by [`authorized`]), so
/// counting distinct authors counts distinct admins, and a lone admin's removal of
/// another never reaches threshold.
fn effective_removal(
    target: &str,
    governance: Governance,
    by_hash: &BTreeMap<String, &SignedMembershipOp>,
    order: &[String],
    valid: &BTreeSet<String>,
) -> Option<String> {
    let needed = match governance {
        Governance::SingleOwner => 1,
        Governance::Quorum { threshold } => threshold.max(1),
    };
    let mut authors: BTreeSet<String> = BTreeSet::new();
    for h in order {
        if !valid.contains(h) {
            continue;
        }
        let op = &by_hash[h].op;
        if is_removal(op.action) && op.subject == target {
            authors.insert(op.author.clone());
            if authors.len() >= needed {
                return Some(h.clone());
            }
        }
    }
    None
}

/// Whether `op`'s author held the capability to perform it, given the membership
/// `mp` derived from the op's valid causal past. Owner or a delegated inviter may
/// `Admit` (attenuated: cannot grant above the author's own role); an admin
/// (`Role::Owner`) may `SetRole`/`Remove`/`Revoke`. Under `SingleOwner` the owner
/// is irrevocable via the chain; under `Quorum` every member is uniform (the owner
/// is removable, gated by the co-signature threshold in [`effective_removal`]).
fn authorized(
    mp: &BTreeMap<String, ValidMember>,
    op: &MembershipOp,
    owner: &str,
    governance: Governance,
) -> bool {
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
            let owner_protected = governance == Governance::SingleOwner && op.subject == owner;
            author.role == Role::Owner && !owner_protected && mp.contains_key(&op.subject)
        }
        // Governance is owner-managed (ADR-026 §A4). The op is inert to the member
        // map (`build_members` skips non-Admit ops); this only keeps it in the
        // valid set when owner-authored so `derive_governance` can read it.
        MembershipAction::SetGovernance => author.role == Role::Owner,
        // Encryption mode is owner-managed (ADR-039 F2). Inert to the member map; kept in
        // the valid set when owner-authored so `derive_encryption` can read it.
        MembershipAction::SetEncryption => author.role == Role::Owner,
        // Identity rotation (ADR-040 §1): the author rotates THEIR OWN identity. The
        // `mp.get` above already confirmed the author is a current member; here we require
        // the op be well-formed and non-elevating:
        //  - both successor keys present;
        //  - `subject` is BOUND to `new_pubkey` (its fingerprint) — the endorsed successor
        //    IS the named principal, so a member can't alias their seat onto an unrelated
        //    key's fingerprint;
        //  - `subject` is a FRESH identity, not already a member — you rotate INTO a new key,
        //    never ONTO an existing member (which the post-pass would clobber/downgrade);
        //  - not a self-rebind (`subject == author` is a no-op).
        // The successor inherits the author's EXACT role/epoch in the derive post-pass, so a
        // Rebind grants NO new authority (no self-elevation) — `author.role` is irrelevant.
        MembershipAction::Rebind => match op.new_pubkey {
            Some(npk) => {
                op.new_wrap_pubkey.is_some()
                    && fingerprint_of(&npk) == op.subject
                    && op.subject != op.author
                    && !mp.contains_key(&op.subject)
            }
            None => false,
        },
        // Recovery-key registration (ADR-040 §Recovery-key): the author registers THEIR OWN
        // offline recovery key. `mp.get` above already confirmed the author is a current
        // member; require it be self-targeted (`subject == author`) and carry a key. Inert to
        // the member map (`build_members` skips it); the derive's recovery registry reads the
        // key. Grants NO authority change — `author.role` is irrelevant.
        MembershipAction::RegisterRecoveryKey => {
            op.subject == op.author && op.recovery_pubkey.is_some()
        }
    }
}

/// The set of fingerprints that ARE the anchored owner across identity rotations
/// (ADR-040): the genesis owner plus every successor reachable through a chain of
/// crypto-valid, fingerprint-bound `Rebind`s the owner (or a prior successor) signed.
/// The owner-rooted readers ([`derive_governance`], [`derive_encryption`],
/// [`find_wrapped_content_key`]) accept an op authored by ANY principal in this set, so a
/// rotated owner keeps authoring governance / encryption / key-delivery ops. This is sound
/// WITHOUT the full membership derive because each link carries the predecessor's signature
/// (filtered by `verify_signed` upstream) and is fingerprint-bound — only the real
/// key-holder can extend the chain — and E2e KBs are SingleOwner (the owner is irrevocable,
/// so the owner's rebind chain is always honored; cf. `authorized`'s Rebind arm).
fn owner_principal_chain(crypto: &[&SignedMembershipOp], genesis_owner: &str) -> BTreeSet<String> {
    let mut chain: BTreeSet<String> = BTreeSet::new();
    chain.insert(genesis_owner.to_string());
    // Forward closure to a fixpoint so chained rotations (owner → o' → o'') all resolve.
    // Termination: every growing pass inserts a NEW subject (guarded by `!chain.contains`), and
    // the chain holds at most one entry per distinct Rebind subject, so it converges in
    // ≤ crypto.len() passes even on a maliciously cyclic/forged rebind set. The explicit
    // `max_passes` ceiling keeps that guarantee defensive against a future refactor that breaks
    // the set-growth invariant.
    // PERF: O(passes × crypto.len()); part of the per-access derive cost the derive cache
    // addresses (Workstream B / ADR-042, #247).
    let max_passes = crypto.len().saturating_add(1);
    for _ in 0..max_passes {
        let mut grew = false;
        for o in crypto {
            if o.op.action != MembershipAction::Rebind {
                continue;
            }
            let bound =
                o.op.new_pubkey
                    .map(|pk| fingerprint_of(&pk) == o.op.subject)
                    .unwrap_or(false);
            if bound
                && o.op.subject != o.op.author
                && chain.contains(&o.op.author)
                && !chain.contains(&o.op.subject)
            {
                chain.insert(o.op.subject.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    chain
}

/// Is `fp` a current owner principal of this KB — the genesis owner **or** any of its
/// cross-signed rotation successors (ADR-040)? Resolves the same owner-anchored chain
/// [`derive_governance`] / [`derive_valid_members_governed`] use internally, so callers
/// outside this module (e.g. the editor's reactive content-key re-wrap) can ask "does this
/// key still speak for the owner?" after the owner has itself rotated — where the collection's
/// meta `owner()` field still points at the genesis fingerprint. Returns `false` when there is
/// no anchored genesis (untrusted / un-anchored log). Authority for a *write* is still enforced
/// by the daemon per-op; this is the local planning check.
pub fn is_owner_principal(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    fp: &str,
) -> bool {
    let crypto: Vec<&SignedMembershipOp> = crypto_valid(ops);
    let Some(genesis) = crypto.iter().find(|o| {
        o.op.prev_hash.is_empty()
            && o.op.action == MembershipAction::Admit
            && o.op.subject == o.op.author
            && &o.author_pubkey == anchor_owner_pubkey
    }) else {
        return false; // no trusted root ⇒ nobody is an owner principal
    };
    owner_principal_chain(&crypto, &genesis.op.subject).contains(fp)
}

/// Derive the KB's active [`Governance`] from the signed op-log (ADR-026 §A4).
/// Owner-rooted + deterministic: the **latest** crypto-valid `SetGovernance` op
/// authored by the anchored owner, in causal order, wins; absent/unparseable ⇒
/// the [`Governance::SingleOwner`] default (matches v0.14). Governance is read
/// *before* membership (`derive_valid_members_governed`) and passed in — the owner
/// is the trust anchor, so owner-authored governance ops are trusted without the
/// circularity of "quorum decides who the owner is." A compromised owner is
/// handled by every peer's local blocklist (§A4), not here.
pub fn derive_governance(ops: &[SignedMembershipOp], anchor_owner_pubkey: &[u8; 32]) -> Governance {
    // Crypto-valid ops only, indexed by chain_hash (mirrors derive_valid_members §1).
    let crypto: Vec<&SignedMembershipOp> = crypto_valid(ops);
    let by_hash: BTreeMap<String, &SignedMembershipOp> =
        crypto.iter().map(|o| (o.chain_hash(), *o)).collect();
    // Anchor: the genesis owner self-admit signed by the external trust root.
    let genesis = match crypto.iter().find(|o| {
        o.op.prev_hash.is_empty()
            && o.op.action == MembershipAction::Admit
            && o.op.subject == o.op.author
            && &o.author_pubkey == anchor_owner_pubkey
    }) {
        Some(g) => g,
        None => return Governance::SingleOwner, // no trusted root ⇒ default
    };
    let owner = genesis.op.subject.clone();
    // Resolve the owner across identity rotations (ADR-040): accept any successor.
    let owners = owner_principal_chain(&crypto, &owner);
    // Latest owner-authored SetGovernance in deterministic causal order.
    let order = causal_order(&by_hash, &genesis.chain_hash());
    let mut gov = Governance::SingleOwner;
    for h in &order {
        let o = &by_hash[h].op;
        if o.action == MembershipAction::SetGovernance && owners.contains(&o.author) {
            if let Some(g) = Governance::parse_spec(&o.subject) {
                gov = g; // later ops in causal order override earlier ones
            }
        }
    }
    gov
}

/// ADR-039 F2: derive the KB's content-encryption mode from the signed op-log — the
/// anti-downgrade counterpart to [`derive_governance`]. Owner-rooted and **monotonic**:
/// any owner-authored `SetEncryption("e2e")` op makes the KB E2e **permanently**, so a
/// later op (forged, or even owner-authored `"none"`) cannot downgrade it — the seal path
/// reads this value and stays fail-closed once it is `E2e`. Absent ⇒ [`Encryption::None`].
/// Because the mode lives in the signed log, a relay cannot flip the unsigned collection
/// flag to coax a victim into emitting plaintext.
pub fn derive_encryption(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
) -> crate::kb::Encryption {
    let crypto: Vec<&SignedMembershipOp> = crypto_valid(ops);
    let genesis = match crypto.iter().find(|o| {
        o.op.prev_hash.is_empty()
            && o.op.action == MembershipAction::Admit
            && o.op.subject == o.op.author
            && &o.author_pubkey == anchor_owner_pubkey
    }) {
        Some(g) => g,
        None => return crate::kb::Encryption::None,
    };
    let owner = genesis.op.subject.clone();
    // Resolve the owner across identity rotations (ADR-040): a rotated owner still latches.
    let owners = owner_principal_chain(&crypto, &owner);
    // Monotonic: ANY owner-authored `e2e` op latches E2e (one-way, no downgrade). No
    // causal ordering needed — once it's asserted, it stays.
    if crypto.iter().any(|o| {
        o.op.action == MembershipAction::SetEncryption
            && owners.contains(&o.op.author)
            && o.op.subject == "e2e"
    }) {
        crate::kb::Encryption::E2e
    } else {
        crate::kb::Encryption::None
    }
}

/// ADR-037 §D2: recover **this peer's** per-KB content key from the signed op-log —
/// the confidentiality counterpart to [`derive_governance`]. Deterministic +
/// owner-rooted: the **latest** crypto-valid op authored by the anchored owner that
/// targets me (`subject == my_fingerprint`) and carries a `wrapped_key`, in causal
/// order, wins — so a rotation re-wrap supersedes the original admit. Unwrap it with
/// my Ed25519 secret. `None` ⇒ an unencrypted KB, no key delivered to me, or a wrap
/// that doesn't open for me. Trustless + peer-derivable, no key server.
///
/// Removal-correctness falls out for free: on rotation the owner re-wraps the new key
/// **only to remaining members**, so a removed member has no new wrapped op and this
/// returns their *old* key — they can still read history they already had, but not
/// content encrypted under the rotated key (ADR-037 §D3).
pub fn derive_content_key(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    my_fingerprint: &str,
    my_ed25519_secret: &[u8; 32],
) -> Option<crate::content_crypto::ContentKey> {
    let wrapped = find_wrapped_content_key(ops, anchor_owner_pubkey, my_fingerprint)?;
    crate::content_crypto::unwrap_as_member(&wrapped, my_ed25519_secret).ok()
}

/// The **find** half of [`derive_content_key`]: my CURRENT wrapped content key in the
/// signed op-log — the wrapped blob from the LATEST owner-authored op targeting me, in
/// causal order (rotation supersedes the original admit). Pure, and crucially **needs
/// no secret**, so the editor's main thread (which holds the collection doc but not the
/// identity key) can extract the blob and hand it to the network task, which unwraps it
/// with the secret. `None` ⇒ an unencrypted KB or no key delivered to me.
///
/// **#169 L1 — why this is not over-broad despite not intersecting the strong-removal
/// resolver.** It counts ONLY `author == owner` wraps (the anchored genesis owner), and an
/// E2e KB is **SingleOwner** by construction (ADR-039 F4: `kb/set_encryption` refuses any
/// other governance). SingleOwner ⇒ no delegated inviters ⇒ no inviter-removal **cascade**,
/// so a "resolver-invalidated" member simply cannot exist here. A non-owner-authored admit
/// (the only thing a cascade could touch) carries no owner wrap, so it's ignored regardless.
/// The one principal who is "not a current member yet still derives a key" is a member the
/// owner explicitly removed: they keep their OLD wrapped blob (no re-key targets them) and so
/// can read history they already had — the intended ADR-037 §D3 semantics, NOT a leak. Hence
/// the deliberate choice to NOT intersect [`derive_valid_members`]: doing so would also strip
/// the removed member's historical key. The "derives a key" set is exactly {owner} ∪
/// {members the owner directly wrapped to}, current or §D3-removed.
// FIXME(#237): join-after-removal cannot open pre-rotation ops. This returns the CURRENT wrapped
// content key; a member who joins (or re-joins) after a removal + content-key rotation has no wrap
// for the PREVIOUS key, so ops sealed before their access are undecryptable to them. Intended
// (removal re-keys forward) but the boundary is a design gap for re-admits — key-history/rewrap is
// a v0.16 item (ADR-037 §D4). Documented in E2E_USER_GUIDE §7.
pub fn find_wrapped_content_key(
    ops: &[SignedMembershipOp],
    anchor_owner_pubkey: &[u8; 32],
    my_fingerprint: &str,
) -> Option<Vec<u8>> {
    // Crypto-valid ops only, indexed by chain_hash (mirrors derive_governance §1).
    let crypto: Vec<&SignedMembershipOp> = crypto_valid(ops);
    let by_hash: BTreeMap<String, &SignedMembershipOp> =
        crypto.iter().map(|o| (o.chain_hash(), *o)).collect();
    let genesis = crypto.iter().find(|o| {
        o.op.prev_hash.is_empty()
            && o.op.action == MembershipAction::Admit
            && o.op.subject == o.op.author
            && &o.author_pubkey == anchor_owner_pubkey
    })?;
    let owner = genesis.op.subject.clone();
    // Resolve the owner across identity rotations (ADR-040): a rotated owner's re-wraps
    // (incl. the re-wrap delivering the key to a rotated MEMBER's new fingerprint) count.
    let owners = owner_principal_chain(&crypto, &owner);
    let order = causal_order(&by_hash, &genesis.chain_hash());
    // Latest owner-authored wrapped key targeting me wins (rotation supersedes admit).
    let mut latest: Option<Vec<u8>> = None;
    for h in &order {
        let o = &by_hash[h].op;
        if owners.contains(&o.author) && o.subject == my_fingerprint {
            if let Some(wk) = &o.wrapped_key {
                latest = Some(wk.clone());
            }
        }
    }
    latest
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
    governance: Governance,
    now: u64,
) -> BTreeMap<String, ValidMember> {
    let sub: BTreeSet<String> = valid.intersection(target_anc).cloned().collect();
    build_members(by_hash, order, &sub, anc, genesis_hash, governance, now)
}

/// Construct the member map from a set of already-validated ops, in causal order.
/// Removal-dominant per p2panda-auth: a principal is present iff some valid `Admit`
/// of them is **not** overridden by an *effective* removal (under `governance`)
/// outside that admit's causal past (so a concurrent/later removal wins, but a
/// causally-later *re-admit* supersedes it). Roles overlay causally-later valid
/// `SetRole`s (higher `chain_hash` wins concurrent ties via `order`).
#[allow(clippy::too_many_arguments)]
fn build_members(
    by_hash: &BTreeMap<String, &SignedMembershipOp>,
    order: &[String],
    valid: &BTreeSet<String>,
    anc: &BTreeMap<String, BTreeSet<String>>,
    genesis_hash: &str,
    governance: Governance,
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
        // Is this admit overridden by an *effective* removal of the principal that
        // is not in the admit's causal past? (Removal dominates concurrent/later;
        // under quorum the removal is effective only at the co-signature threshold.)
        let dominated = effective_removal(&principal, governance, by_hash, order, valid)
            .map(|rh| !in_admit_past(h, &rh))
            .unwrap_or(false);
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
    // ADR-040 identity-rotation post-pass. Apply honored `Rebind`s in causal order: each
    // TRANSFERS the predecessor's derived entry to the successor (same role / epoch /
    // invited_by / can_invite) and RETIRES the predecessor. Because this runs inside every
    // `build_members` call — including `membership_at`, which builds over an op's causal
    // past — a Rebind in an op's past retires the old key for THAT op's capability check, so
    // a rotated-away key's later ops are not honored (retirement is causal, not global).
    // Chained rebinds (a→b→c) compose by walking causal order. A Rebind whose predecessor is
    // not a current member here (already retired/removed/never admitted) contributes nothing
    // (fail-closed) — `authorized` already required a fresh, fingerprint-bound successor.
    for h in order {
        if !valid.contains(h) {
            continue;
        }
        let op = &by_hash[h].op;
        if op.action != MembershipAction::Rebind {
            continue;
        }
        if let Some(mut entry) = out.remove(&op.author) {
            entry.principal = op.subject.clone();
            out.insert(op.subject.clone(), entry);
            // Re-point provenance: members the predecessor invited now trace to the
            // successor, so an inviter-removal cascade (CascadeAll) does NOT orphan a
            // rotated inviter's invitees. The just-inserted successor entry keeps the
            // predecessor's own `invited_by`, so it can't self-match here.
            for m in out.values_mut() {
                if m.invited_by == op.author {
                    m.invited_by = op.subject.clone();
                }
            }
        }
    }
    out
}

/// Deterministic causal (topological) order of the op tree rooted at `genesis`.
/// Each op links to exactly one parent via `prev_hash`, so the reachable set is a
/// tree; nodes are emitted parent-before-child, concurrent siblings in ascending
/// `chain_hash` order — so every honest peer replays identically regardless of the
/// order ops arrived in. Ops **not** reachable from the anchored genesis (orphans
/// with a dangling `prev_hash`, or a forged second root) are never emitted.
// ADR-042 (#247): O(n log n) BFS from the genesis via a children-adjacency map, replacing the prior
// O(depth × n) generation-scan (membership ops form a near-linear chain ⇒ depth≈n ⇒ O(n²) on every
// per-access derive). The emit order is IDENTICAL to the prior impl — BFS by causal generation, each
// generation emitted in ascending `chain_hash` order — so every honest peer still replays identically
// (property-tested against the reference impl on random trees). Each op has exactly one parent via
// `prev_hash`, so the reachable set is a tree; orphans (dangling `prev_hash`) and forged second roots
// are never reached from `genesis`, so they are dropped exactly as before.
fn causal_order(by_hash: &BTreeMap<String, &SignedMembershipOp>, genesis: &str) -> Vec<String> {
    if !by_hash.contains_key(genesis) {
        return Vec::new(); // no anchored root ⇒ nothing reachable
    }
    // parent chain_hash → its child hashes (BTreeSet keeps the sibling order deterministic).
    let mut children: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (h, o) in by_hash {
        children
            .entry(o.op.prev_hash.as_str())
            .or_default()
            .insert(h.as_str());
    }
    let mut order: Vec<String> = Vec::with_capacity(by_hash.len());
    let mut emitted: BTreeSet<&str> = BTreeSet::new();
    // Frontier = one causal generation; genesis is generation 0.
    let mut frontier: Vec<&str> = vec![genesis];
    while !frontier.is_empty() {
        frontier.sort_unstable(); // ascending chain_hash across the whole generation (matches prior)
        for h in &frontier {
            if emitted.insert(*h) {
                order.push((*h).to_string());
            }
        }
        // Next generation = children of everything just emitted (each has a single parent, so no
        // node is enqueued twice; the `emitted` guard is defensive).
        let mut next: Vec<&str> = Vec::new();
        for h in &frontier {
            if let Some(cs) = children.get(*h) {
                next.extend(cs.iter().copied().filter(|c| !emitted.contains(c)));
            }
        }
        frontier = next;
    }
    order
}

/// The `SHA256:<base64>` fingerprint of an Ed25519 public key — the membership
/// **principal**. Matches `mae_mcp::identity::PublicKey::fingerprint()` so a
/// member's principal is identical whether derived here or there.
///
// KLUDGE(#246): the fingerprint format is UNVERSIONED — all authority binds to this exact
// `SHA256:` + STANDARD_NO_PAD encoding. If the encoding ever changes (padding, hash, prefix),
// every prior op silently stops verifying and legitimate members vanish. Not a bug today (it was
// fixed at v0.1), but a format version tag would make a future migration safe. No repo change
// without a coordinated op-log migration.
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
            wrapped_key: None,
            new_pubkey: None,
            new_wrap_pubkey: None,
            recovery_pubkey: None,
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
    impl Id {
        /// ADR-041 (#158 I1): the identity's PUBLISHED X25519 wrap key — what the owner
        /// wraps the content key to (no longer the ed25519 pubkey).
        fn wrap_pub(&self) -> [u8; 32] {
            crate::content_crypto::wrap_public_for(&self.secret)
        }
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
            wrapped_key: None,
            new_pubkey: None,
            new_wrap_pubkey: None,
            recovery_pubkey: None,
        };
        let sig = op.sign(&author.secret);
        SignedMembershipOp {
            op,
            sig,
            author_pubkey: author.pubkey,
        }
    }

    /// Build + sign an `Admit` that carries an ADR-037 `wrapped_key` (a v2 op).
    fn make_wrapped(
        author: &Id,
        subject: &str,
        wrapped: Vec<u8>,
        prev: &str,
    ) -> SignedMembershipOp {
        let op = MembershipOp {
            kb_id: "KB".into(),
            action: MembershipAction::Admit,
            subject: subject.into(),
            role: Some(Role::Editor),
            can_invite: false,
            author: author.fp.clone(),
            issued_at: 1,
            expires_at: None,
            epoch: 0,
            prev_hash: prev.into(),
            wrapped_key: Some(wrapped),
            new_pubkey: None,
            new_wrap_pubkey: None,
            recovery_pubkey: None,
        };
        let sig = op.sign(&author.secret);
        SignedMembershipOp {
            op,
            sig,
            author_pubkey: author.pubkey,
        }
    }

    #[test]
    fn v1_op_stays_compatible_and_v2_binds_the_wrapped_key() {
        let owner = id(1);
        let m = id(2);
        // No wrapped key ⇒ byte-identical v1 encoding, still signs + verifies (the
        // regression guard for the canonical-bytes change — existing ops are safe).
        let v1 = make(
            &owner,
            MembershipAction::Admit,
            &m.fp,
            Some(Role::Editor),
            false,
            None,
            "",
        );
        assert!(
            v1.op.canonical_bytes().starts_with(b"maememb/v1\0"),
            "no wrap ⇒ v1"
        );
        assert!(v1.verify_signed(), "a v1 op still verifies");
        // A wrapped key ⇒ v2, signs + verifies, and tampering the key breaks the sig
        // (the wrapped key is part of the signed bytes — a relay can't swap it).
        let v2 = make_wrapped(&owner, &m.fp, vec![1, 2, 3, 4], "");
        assert!(
            v2.op.canonical_bytes().starts_with(b"maememb/v2\0"),
            "wrap ⇒ v2"
        );
        assert!(v2.verify_signed(), "a v2 op verifies");
        let mut tampered = v2.clone();
        tampered.op.wrapped_key = Some(vec![9, 9, 9, 9]);
        assert!(
            !tampered.verify_signed(),
            "tampering the wrapped key breaks the signature"
        );
    }

    #[test]
    fn derive_content_key_delivers_to_members_excludes_others_and_rotates() {
        use crate::content_crypto::{wrap_to_member, ContentKey};
        let owner = id(1);
        let m1 = id(2);
        let m2 = id(3);
        let stranger = id(4);
        let k = ContentKey::generate();

        let g = genesis(&owner);
        // Owner admits m1 + m2, each with k wrapped to THEM; plus a no-wrap path.
        let a1 = make_wrapped(
            &owner,
            &m1.fp,
            wrap_to_member(&k, &m1.wrap_pub()).unwrap(),
            &g.chain_hash(),
        );
        let a2 = make_wrapped(
            &owner,
            &m2.fp,
            wrap_to_member(&k, &m2.wrap_pub()).unwrap(),
            &a1.chain_hash(),
        );
        let ops = vec![g.clone(), a1.clone(), a2.clone()];

        let derived = |ops: &[SignedMembershipOp], who: &Id| {
            derive_content_key(ops, &owner.pubkey, &who.fp, &who.secret).map(|c| *c.as_bytes())
        };
        assert_eq!(derived(&ops, &m1), Some(*k.as_bytes()), "m1 recovers k");
        assert_eq!(derived(&ops, &m2), Some(*k.as_bytes()), "m2 recovers k");
        assert!(
            derived(&ops, &stranger).is_none(),
            "a non-member recovers nothing"
        );

        // A NON-OWNER cannot inject a key: m1 forges a wrapped op targeting the
        // stranger — derive ignores it (only the anchored owner distributes keys).
        let forged = make_wrapped(
            &m1,
            &stranger.fp,
            wrap_to_member(&k, &stranger.wrap_pub()).unwrap(),
            &a2.chain_hash(),
        );
        let mut ops_forged = ops.clone();
        ops_forged.push(forged);
        assert!(
            derived(&ops_forged, &stranger).is_none(),
            "non-owner key injection ignored"
        );

        // ROTATION (selective): owner re-wraps a NEW key k' to m1 only (m2 is being
        // removed). m1 follows to k'; m2 has no re-wrap ⇒ stuck at the old k — proving
        // exclusion alongside continued access, not a dead no-op.
        let k2 = ContentKey::generate();
        let rewrap = make_wrapped(
            &owner,
            &m1.fp,
            wrap_to_member(&k2, &m1.wrap_pub()).unwrap(),
            &a2.chain_hash(),
        );
        let ops2 = vec![g, a1, a2, rewrap];
        assert_eq!(
            derived(&ops2, &m1),
            Some(*k2.as_bytes()),
            "m1 follows the rotation to k'"
        );
        assert_eq!(
            derived(&ops2, &m2),
            Some(*k.as_bytes()),
            "m2 excluded from k', stuck at old k"
        );
    }

    // #169 L1: a "resolver-invalidated" member (the cascade case — admitted by a non-owner
    // inviter whose authority is later revoked) must derive NO key. Under E2e ⇒ SingleOwner
    // (F4) such a member can't even exist, but we pin the underlying bound: ONLY the anchored
    // owner's wraps count, so a crypto-valid admit authored by a NON-owner — carrying a
    // (necessarily attacker-built) wrapped blob to its subject — confers nothing. This is the
    // resolver-invalidated test the L1 finding asked for; it also guards the §D3 boundary by
    // showing the owner-wrapped sibling is unaffected.
    #[test]
    fn derive_content_key_denies_a_non_owner_conferred_member_l1() {
        use crate::content_crypto::{wrap_to_member, ContentKey};
        let owner = id(1);
        let inviter = id(2); // owner-admitted member; stands in for a would-be delegator
        let victim = id(3); // "admitted" by the inviter, not the owner
        let k = ContentKey::generate();

        let g = genesis(&owner);
        let ai = make_wrapped(
            &owner,
            &inviter.fp,
            wrap_to_member(&k, &inviter.wrap_pub()).unwrap(),
            &g.chain_hash(),
        );
        // The inviter (NOT the owner) authors an admit of the victim carrying a wrapped blob.
        // In a real E2e KB the inviter never holds k; here we hand it one anyway to prove the
        // op is rejected on AUTHORSHIP (author != owner), not on a crypto/availability detail.
        let av = make_wrapped(
            &inviter,
            &victim.fp,
            wrap_to_member(&k, &victim.wrap_pub()).unwrap(),
            &ai.chain_hash(),
        );
        let ops = vec![g, ai, av];

        assert!(
            derive_content_key(&ops, &owner.pubkey, &victim.fp, &victim.secret).is_none(),
            "a member whose ONLY wrap is non-owner-authored derives no key (#169 L1)"
        );
        assert_eq!(
            derive_content_key(&ops, &owner.pubkey, &inviter.fp, &inviter.secret)
                .map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "the owner-wrapped sibling is unaffected (the bound is precise, not a blanket deny)"
        );
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
    fn derive_encryption_latches_e2e_and_resists_downgrade_and_forgery() {
        use crate::kb::Encryption;
        let owner = id(1);
        let attacker = id(2); // a non-owner member with can_invite
        let g = genesis(&owner);

        // No SetEncryption op ⇒ None (the default; unencrypted KBs unaffected).
        assert_eq!(
            derive_encryption(std::slice::from_ref(&g), &owner.pubkey),
            Encryption::None,
            "absent ⇒ None"
        );

        // Owner asserts e2e ⇒ E2e.
        let enable = make(
            &owner,
            MembershipAction::SetEncryption,
            "e2e",
            None,
            false,
            None,
            &g.chain_hash(),
        );
        assert_eq!(
            derive_encryption(&[g.clone(), enable.clone()], &owner.pubkey),
            Encryption::E2e,
            "owner SetEncryption(e2e) ⇒ E2e"
        );

        // Downgrade resistance: a LATER owner `none` op does NOT un-encrypt (monotonic).
        let downgrade = make(
            &owner,
            MembershipAction::SetEncryption,
            "none",
            None,
            false,
            None,
            &enable.chain_hash(),
        );
        assert_eq!(
            derive_encryption(&[g.clone(), enable.clone(), downgrade], &owner.pubkey),
            Encryption::E2e,
            "e2e is one-way — a later owner 'none' cannot downgrade"
        );

        // Forgery: a NON-owner asserting e2e is ignored (only the anchored owner counts).
        let forged = make(
            &attacker,
            MembershipAction::SetEncryption,
            "e2e",
            None,
            false,
            None,
            &g.chain_hash(),
        );
        assert_eq!(
            derive_encryption(&[g.clone(), forged.clone()], &owner.pubkey),
            Encryption::None,
            "a non-owner SetEncryption(e2e) does NOT enable encryption"
        );

        // Tamper: corrupt the enable op's signature ⇒ it fails verify_signed ⇒ ignored.
        let mut tampered = enable.clone();
        tampered.author_pubkey = attacker.pubkey; // claim a different author key
        assert_eq!(
            derive_encryption(&[g, tampered], &owner.pubkey),
            Encryption::None,
            "an op whose signature doesn't match its claimed author key is ignored"
        );
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

    /// Build + sign a `SetGovernance` op authored by `author` (spec in `subject`).
    fn set_gov(author: &Id, gov: Governance, prev: &str) -> SignedMembershipOp {
        make(
            author,
            MembershipAction::SetGovernance,
            &gov.to_spec(),
            None,
            false,
            None,
            prev,
        )
    }

    #[test]
    fn governance_spec_roundtrips() {
        for g in [
            Governance::SingleOwner,
            Governance::Quorum { threshold: 1 },
            Governance::Quorum { threshold: 3 },
        ] {
            assert_eq!(Governance::parse_spec(&g.to_spec()), Some(g));
        }
        assert_eq!(Governance::parse_spec("quorum:0"), None, "0 rejected");
        assert_eq!(Governance::parse_spec("garbage"), None);
        assert_eq!(Governance::parse_spec("quorum:x"), None);
    }

    #[test]
    fn governance_defaults_to_single_owner() {
        let owner = id(1);
        let g = genesis(&owner);
        assert_eq!(
            derive_governance(&[g], &owner.pubkey),
            Governance::SingleOwner
        );
    }

    #[test]
    fn governance_owner_sets_quorum_latest_wins() {
        let owner = id(1);
        let g = genesis(&owner);
        let sg1 = set_gov(&owner, Governance::Quorum { threshold: 2 }, &g.chain_hash());
        let sg2 = set_gov(&owner, Governance::SingleOwner, &sg1.chain_hash());
        // Only sg1: quorum is active.
        assert_eq!(
            derive_governance(&[g.clone(), sg1.clone()], &owner.pubkey),
            Governance::Quorum { threshold: 2 }
        );
        // sg1 then sg2: the later op (single-owner) wins.
        assert_eq!(
            derive_governance(&[g, sg1, sg2], &owner.pubkey),
            Governance::SingleOwner
        );
    }

    #[test]
    fn governance_non_owner_op_is_ignored() {
        // Adversarial: a member (even an admin) cannot weaken governance — only the
        // anchored owner's SetGovernance counts.
        let owner = id(1);
        let mallory = id(2);
        let g = genesis(&owner);
        // Owner admits mallory as an owner-role admin (the strongest non-anchor case).
        let admit = make(
            &owner,
            MembershipAction::Admit,
            &mallory.fp,
            Some(Role::Owner),
            true,
            None,
            &g.chain_hash(),
        );
        // Mallory tries to set quorum:1 (would let a lone admin remove the owner).
        let forged = set_gov(
            &mallory,
            Governance::Quorum { threshold: 1 },
            &admit.chain_hash(),
        );
        assert_eq!(
            derive_governance(&[g, admit, forged], &owner.pubkey),
            Governance::SingleOwner,
            "a non-owner SetGovernance must not take effect"
        );
    }

    #[test]
    fn governance_op_is_inert_to_membership() {
        // A SetGovernance op's `subject` ("quorum:2") must never be admitted as a
        // member, and the op must not perturb the derived member set.
        let owner = id(1);
        let g = genesis(&owner);
        let sg = set_gov(&owner, Governance::Quorum { threshold: 2 }, &g.chain_hash());
        let m = derive_valid_members(&[g, sg], &owner.pubkey, 100);
        assert_eq!(m.len(), 1, "only the owner is a member");
        assert!(m.contains_key(&owner.fp));
        assert!(!m.contains_key("quorum:2"), "the spec is never a principal");
    }

    #[test]
    fn governance_from_log_feeds_quorum_removal() {
        // End-to-end: governance sourced from the signed log reaches the (already
        // tested) quorum tally — under quorum:2, two distinct admins co-removing the
        // owner takes effect (single-owner would protect the owner).
        let owner = id(1);
        let a = id(2);
        let b = id(3);
        let g = genesis(&owner);
        let sg = set_gov(&owner, Governance::Quorum { threshold: 2 }, &g.chain_hash());
        let admit_a = make(
            &owner,
            MembershipAction::Admit,
            &a.fp,
            Some(Role::Owner),
            true,
            None,
            &sg.chain_hash(),
        );
        let admit_b = make(
            &owner,
            MembershipAction::Admit,
            &b.fp,
            Some(Role::Owner),
            true,
            None,
            &admit_a.chain_hash(),
        );
        let rm_a = make(
            &a,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &admit_b.chain_hash(),
        );
        let rm_b = make(
            &b,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &rm_a.chain_hash(),
        );
        let ops = vec![g, sg, admit_a, admit_b, rm_a, rm_b];

        let gov = derive_governance(&ops, &owner.pubkey);
        assert_eq!(gov, Governance::Quorum { threshold: 2 });
        let members = derive_valid_members_governed(
            &ops,
            &owner.pubkey,
            100,
            gov,
            &MembershipView::default(),
        );
        assert!(
            !members.contains_key(&owner.fp),
            "quorum of 2 admins removes even the owner (governance read from the log)"
        );
        assert!(members.contains_key(&a.fp) && members.contains_key(&b.fp));
    }

    #[test]
    fn quorum_removes_owner_via_concurrent_removal_branches() {
        // The REALISTIC case (the linear test above is the easy one): two admins
        // remove the owner CONCURRENTLY — sibling ops off the same parent, not a
        // tidy sequence. The tally must still reach threshold, and — critically —
        // the derived set must be IDENTICAL under any apply order (it's a CRDT).
        let owner = id(1);
        let a = id(2);
        let b = id(3);
        let g = genesis(&owner);
        let sg = set_gov(&owner, Governance::Quorum { threshold: 2 }, &g.chain_hash());
        let admit_a = make(
            &owner,
            MembershipAction::Admit,
            &a.fp,
            Some(Role::Owner),
            true,
            None,
            &sg.chain_hash(),
        );
        let admit_b = make(
            &owner,
            MembershipAction::Admit,
            &b.fp,
            Some(Role::Owner),
            true,
            None,
            &admit_a.chain_hash(),
        );
        // CONCURRENT removals: both children of admit_b (same prev_hash) — neither
        // is in the other's causal past.
        let rm_a = make(
            &a,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &admit_b.chain_hash(),
        );
        let rm_b = make(
            &b,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &admit_b.chain_hash(),
        );
        let ops = vec![g, sg, admit_a, admit_b, rm_a, rm_b];

        let gov = derive_governance(&ops, &owner.pubkey);
        let derive = |o: &[SignedMembershipOp]| {
            derive_valid_members_governed(o, &owner.pubkey, 100, gov, &MembershipView::default())
        };
        let m = derive(&ops);
        assert!(
            !m.contains_key(&owner.fp),
            "two CONCURRENT admin removals reach quorum:2 and remove the owner"
        );
        assert!(m.contains_key(&a.fp) && m.contains_key(&b.fp));
        // Order-independence — the property that linear tests can't prove.
        let mut rev = ops.clone();
        rev.reverse();
        assert_eq!(derive(&rev), m, "reversed apply order ⇒ identical members");
        let mut rot = ops.clone();
        rot.rotate_left(3);
        assert_eq!(derive(&rot), m, "rotated apply order ⇒ identical members");
        assert_eq!(
            derive_governance(&rev, &owner.pubkey),
            gov,
            "governance derivation is order-independent too"
        );
    }

    #[test]
    fn quorum_threshold_above_voter_count_protects_the_owner() {
        // quorum:3 but only two non-owner admins exist — the tally can never reach
        // three distinct removal authors (the owner won't sign their own removal),
        // so the owner survives even with both others voting. Guards against an
        // off-by-one in the threshold comparison.
        let owner = id(1);
        let a = id(2);
        let b = id(3);
        let g = genesis(&owner);
        let sg = set_gov(&owner, Governance::Quorum { threshold: 3 }, &g.chain_hash());
        let admit_a = make(
            &owner,
            MembershipAction::Admit,
            &a.fp,
            Some(Role::Owner),
            true,
            None,
            &sg.chain_hash(),
        );
        let admit_b = make(
            &owner,
            MembershipAction::Admit,
            &b.fp,
            Some(Role::Owner),
            true,
            None,
            &admit_a.chain_hash(),
        );
        let rm_a = make(
            &a,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &admit_b.chain_hash(),
        );
        let rm_b = make(
            &b,
            MembershipAction::Remove,
            &owner.fp,
            None,
            false,
            None,
            &admit_b.chain_hash(),
        );
        let ops = vec![g, sg, admit_a, admit_b, rm_a, rm_b];
        let gov = derive_governance(&ops, &owner.pubkey);
        let m = derive_valid_members_governed(
            &ops,
            &owner.pubkey,
            100,
            gov,
            &MembershipView::default(),
        );
        assert!(
            m.contains_key(&owner.fp),
            "threshold 3 > 2 voters ⇒ the owner cannot be removed"
        );
    }

    #[test]
    fn concurrent_set_governance_resolves_deterministically() {
        // Two owner-authored SetGovernance ops as CONCURRENT siblings (both off
        // genesis) — a relay could deliver them in any order. derive_governance must
        // pick the SAME one on every peer (causal_order tiebreaks siblings by
        // ascending chain_hash, so the higher-hash sibling is applied last + wins).
        let owner = id(1);
        let g = genesis(&owner);
        let sg_two = set_gov(&owner, Governance::Quorum { threshold: 2 }, &g.chain_hash());
        let sg_three = set_gov(&owner, Governance::Quorum { threshold: 3 }, &g.chain_hash());
        let ops = vec![g, sg_two.clone(), sg_three.clone()];

        let gov = derive_governance(&ops, &owner.pubkey);
        let expected = if sg_two.chain_hash() > sg_three.chain_hash() {
            Governance::Quorum { threshold: 2 }
        } else {
            Governance::Quorum { threshold: 3 }
        };
        assert_eq!(
            gov, expected,
            "higher-chain_hash sibling wins, deterministically"
        );
        let mut rev = ops.clone();
        rev.reverse();
        assert_eq!(derive_governance(&rev, &owner.pubkey), gov);
        let mut rot = ops.clone();
        rot.rotate_left(1);
        assert_eq!(derive_governance(&rot, &owner.pubkey), gov);
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

    // --- quorum governance (slice 2b-5b) ---

    /// Derive under the given governance with no local view overrides.
    fn governed(
        ops: &[SignedMembershipOp],
        owner: &Id,
        gov: Governance,
    ) -> BTreeMap<String, ValidMember> {
        derive_valid_members_governed(ops, &owner.pubkey, 100, gov, &MembershipView::default())
    }

    /// Owner admits `who` at Role::Owner (an admin), with the invite capability.
    fn admit_admin(owner: &Id, who: &Id, prev: &str) -> SignedMembershipOp {
        make(
            owner,
            MembershipAction::Admit,
            &who.fp,
            Some(Role::Owner),
            true,
            None,
            prev,
        )
    }

    #[test]
    fn quorum_any_admin_can_manage() {
        let owner = id(1);
        let alice = id(2);
        let carol = id(3);
        let g = genesis(&owner);
        let admit_alice = admit_admin(&owner, &alice, &g.chain_hash());
        // alice (an admin) admits carol — proves management is not owner-exclusive.
        let admit_carol = make(
            &alice,
            MembershipAction::Admit,
            &carol.fp,
            Some(Role::Editor),
            false,
            None,
            &admit_alice.chain_hash(),
        );
        let m = governed(
            &[g, admit_alice, admit_carol],
            &owner,
            Governance::Quorum { threshold: 2 },
        );
        assert_eq!(m[&alice.fp].role, Role::Owner, "alice is an admin");
        assert_eq!(
            m[&carol.fp].role,
            Role::Editor,
            "admin alice admitted carol"
        );
    }

    #[test]
    fn quorum_single_admin_cannot_remove_another() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_alice = admit_admin(&owner, &alice, &g.chain_hash());
        let admit_bob = admit_admin(&owner, &bob, &admit_alice.chain_hash());
        // alice alone revokes bob — one of two required co-signatures.
        let revoke_bob = make(
            &alice,
            MembershipAction::Revoke,
            &bob.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let m = governed(
            &[g, admit_alice, admit_bob, revoke_bob],
            &owner,
            Governance::Quorum { threshold: 2 },
        );
        assert!(
            m.contains_key(&bob.fp),
            "a lone admin's revoke does not reach the m-of-n threshold"
        );
    }

    #[test]
    fn quorum_threshold_co_signatures_remove_even_the_owner() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_alice = admit_admin(&owner, &alice, &g.chain_hash());
        let admit_bob = admit_admin(&owner, &bob, &admit_alice.chain_hash());
        // alice + bob (two distinct admins) both revoke the OWNER — members are
        // uniform under quorum, so the founder is removable at threshold.
        let revoke_owner_a = make(
            &alice,
            MembershipAction::Revoke,
            &owner.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let revoke_owner_b = make(
            &bob,
            MembershipAction::Revoke,
            &owner.fp,
            None,
            false,
            None,
            &revoke_owner_a.chain_hash(),
        );
        let ops = [g, admit_alice, admit_bob, revoke_owner_a, revoke_owner_b];
        let m = governed(&ops, &owner, Governance::Quorum { threshold: 2 });
        assert!(
            !m.contains_key(&owner.fp),
            "two admin co-signatures remove the owner under quorum"
        );
        assert!(m.contains_key(&alice.fp) && m.contains_key(&bob.fp));
        // Under single-owner governance the same ops never remove the owner.
        let single = governed(&ops, &owner, Governance::SingleOwner);
        assert!(
            single.contains_key(&owner.fp),
            "single-owner: the owner is irrevocable via the chain"
        );
    }

    #[test]
    fn quorum_mutual_removal_both_apply() {
        let owner = id(1);
        let alice = id(2);
        let bob = id(3);
        let g = genesis(&owner);
        let admit_alice = admit_admin(&owner, &alice, &g.chain_hash());
        let admit_bob = admit_admin(&owner, &bob, &admit_alice.chain_hash());
        // CONCURRENT off admit_bob: alice removes bob ∥ bob removes alice. With
        // threshold 1 each removal is immediately effective; removals are exempt
        // from concurrent invalidation, so BOTH apply (the remove-the-remover
        // paradox resolves without forking or emptying the group).
        let alice_removes_bob = make(
            &alice,
            MembershipAction::Remove,
            &bob.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let bob_removes_alice = make(
            &bob,
            MembershipAction::Remove,
            &alice.fp,
            None,
            false,
            None,
            &admit_bob.chain_hash(),
        );
        let m = governed(
            &[
                g,
                admit_alice,
                admit_bob,
                alice_removes_bob,
                bob_removes_alice,
            ],
            &owner,
            Governance::Quorum { threshold: 1 },
        );
        assert!(!m.contains_key(&alice.fp), "alice removed by bob");
        assert!(!m.contains_key(&bob.fp), "bob removed by alice");
        assert!(m.contains_key(&owner.fp), "owner unaffected");
    }

    // --- ADR-040 identity rotation (Rebind) — adversarial set (§Threat model) ---

    /// Build + sign a `Rebind`: predecessor `old` cross-signs successor `new`, publishing
    /// `new`'s Ed25519 + X25519 wrap keys. `old`'s key signs (that is the whole point).
    fn make_rebind(old: &Id, new: &Id, prev: &str) -> SignedMembershipOp {
        let op = MembershipOp {
            kb_id: "KB".into(),
            action: MembershipAction::Rebind,
            subject: new.fp.clone(),
            role: None,
            can_invite: false,
            author: old.fp.clone(),
            issued_at: 1,
            expires_at: None,
            epoch: 0,
            prev_hash: prev.into(),
            wrapped_key: None,
            new_pubkey: Some(new.pubkey),
            new_wrap_pubkey: Some(new.wrap_pub()),
            recovery_pubkey: None,
        };
        let sig = op.sign(&old.secret);
        SignedMembershipOp {
            op,
            sig,
            author_pubkey: old.pubkey,
        }
    }

    /// Happy path: a member rotates their identity; the successor inherits the EXACT
    /// role / epoch / invited_by / can_invite and the predecessor is retired.
    #[test]
    fn rebind_transfers_membership_to_successor_and_retires_predecessor() {
        let owner = id(1);
        let bob = id(2);
        let bob2 = id(3); // bob's new key
        let g = genesis(&owner);
        // Owner admits bob as Editor with the invite capability, at a non-zero epoch.
        let mut admit = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            true,
            None,
            &g.chain_hash(),
        );
        admit.op.epoch = 7;
        admit.sig = admit.op.sign(&owner.secret);
        let rebind = make_rebind(&bob, &bob2, &admit.chain_hash());

        let m = derive_valid_members(&[g, admit, rebind], &owner.pubkey, 100);
        assert!(!m.contains_key(&bob.fp), "predecessor retired");
        let succ = m.get(&bob2.fp).expect("successor present");
        assert_eq!(succ.role, Role::Editor, "inherits exact role");
        assert!(succ.can_invite, "inherits can_invite");
        assert_eq!(succ.epoch, 7, "inherits exact epoch (no rebase forced)");
        assert_eq!(succ.invited_by, owner.fp, "inherits provenance");
    }

    /// A forged Rebind — the op claims `author = bob` but is signed by someone else —
    /// does not verify, so it is never honored: bob stays, the impostor's successor is absent.
    #[test]
    fn forged_rebind_wrong_signer_is_rejected() {
        let owner = id(1);
        let bob = id(2);
        let mallory_succ = id(9);
        let g = genesis(&owner);
        let admit = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        // Mallory forges a rebind of bob→mallory_succ but signs with HER OWN key while
        // claiming author = bob (and presenting bob's pubkey to dodge fingerprint binding).
        let mut forged = make_rebind(&bob, &mallory_succ, &admit.chain_hash());
        forged.sig = forged.op.sign(&mallory_succ.secret); // wrong signer
                                                           // author_pubkey still bob's (so fingerprint_matches passes) but the sig won't verify.
        let m = derive_valid_members(&[g, admit, forged], &owner.pubkey, 100);
        assert!(
            m.contains_key(&bob.fp),
            "bob NOT rotated by a forged rebind"
        );
        assert!(
            !m.contains_key(&mallory_succ.fp),
            "forged successor never admitted"
        );
    }

    /// A Rebind authored by a NON-member contributes nothing (you can only rotate an
    /// identity you actually hold in this KB).
    #[test]
    fn rebind_by_non_member_contributes_nothing() {
        let owner = id(1);
        let stranger = id(5);
        let stranger2 = id(6);
        let g = genesis(&owner);
        let rebind = make_rebind(&stranger, &stranger2, &g.chain_hash());
        let m = derive_valid_members(&[g, rebind], &owner.pubkey, 100);
        assert_eq!(m.len(), 1, "only the owner");
        assert!(!m.contains_key(&stranger2.fp));
    }

    /// No self-elevation: a Viewer who rotates stays a Viewer — the successor inherits the
    /// predecessor's exact (low) role, never more.
    #[test]
    fn rebind_does_not_elevate_role() {
        let owner = id(1);
        let viewer = id(2);
        let viewer2 = id(3);
        let g = genesis(&owner);
        let admit = make(
            &owner,
            MembershipAction::Admit,
            &viewer.fp,
            Some(Role::Viewer),
            false,
            None,
            &g.chain_hash(),
        );
        let rebind = make_rebind(&viewer, &viewer2, &admit.chain_hash());
        let m = derive_valid_members(&[g, admit, rebind], &owner.pubkey, 100);
        assert_eq!(
            m.get(&viewer2.fp).map(|x| x.role),
            Some(Role::Viewer),
            "successor inherits Viewer, not elevated"
        );
        assert!(
            !m.get(&viewer2.fp).unwrap().can_invite,
            "no invite capability gained"
        );
    }

    /// The retired key is fenced: an op authored by the OLD key AFTER its rebind is not
    /// honored, while the SAME op authored by the successor is.
    #[test]
    fn retired_key_later_op_is_fenced_but_successor_can_act() {
        let owner = id(1);
        let alice = id(2); // an admin who can invite
        let alice2 = id(3);
        let carol = id(4);
        let g = genesis(&owner);
        // Owner admits alice as Owner (so she may admit others).
        let admit_alice = make(
            &owner,
            MembershipAction::Admit,
            &alice.fp,
            Some(Role::Owner),
            true,
            None,
            &g.chain_hash(),
        );
        let rebind = make_rebind(&alice, &alice2, &admit_alice.chain_hash());
        // The RETIRED alice key tries to admit carol after the rebind → must be fenced.
        let stale_admit = make(
            &alice,
            MembershipAction::Admit,
            &carol.fp,
            Some(Role::Editor),
            false,
            None,
            &rebind.chain_hash(),
        );
        let m = derive_valid_members(
            &[g.clone(), admit_alice.clone(), rebind.clone(), stale_admit],
            &owner.pubkey,
            100,
        );
        assert!(
            !m.contains_key(&carol.fp),
            "retired predecessor key cannot admit"
        );
        // The SUCCESSOR key admitting carol IS honored (alice2 inherited Owner).
        let good_admit = make(
            &alice2,
            MembershipAction::Admit,
            &carol.fp,
            Some(Role::Editor),
            false,
            None,
            &rebind.chain_hash(),
        );
        let m2 = derive_valid_members(&[g, admit_alice, rebind, good_admit], &owner.pubkey, 100);
        assert!(
            m2.contains_key(&carol.fp),
            "successor (inherited Owner) can admit"
        );
    }

    /// Chained rebinds a→b→c resolve to c; all predecessors retired.
    #[test]
    fn chained_rebinds_resolve_to_the_final_successor() {
        let owner = id(1);
        let a = id(2);
        let b = id(3);
        let c = id(4);
        let g = genesis(&owner);
        let admit = make(
            &owner,
            MembershipAction::Admit,
            &a.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let r1 = make_rebind(&a, &b, &admit.chain_hash());
        let r2 = make_rebind(&b, &c, &r1.chain_hash());
        let m = derive_valid_members(&[g, admit, r1, r2], &owner.pubkey, 100);
        assert!(!m.contains_key(&a.fp), "a retired");
        assert!(!m.contains_key(&b.fp), "b retired");
        assert_eq!(
            m.get(&c.fp).map(|x| x.role),
            Some(Role::Editor),
            "final successor c inherits the role"
        );
    }

    /// Clobber guard: a member cannot rebind ONTO an existing member's fingerprint (which
    /// would overwrite/downgrade them). The successor must be a FRESH identity.
    #[test]
    fn rebind_onto_an_existing_member_is_rejected() {
        let owner = id(1);
        let bob = id(2); // a lowly editor
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
        // bob attempts to rebind bob→owner (subject = owner's fp, new_pubkey = owner's REAL
        // pubkey, so fingerprint binding passes) — to clobber the owner's entry with his own.
        let op = MembershipOp {
            kb_id: "KB".into(),
            action: MembershipAction::Rebind,
            subject: owner.fp.clone(),
            role: None,
            can_invite: false,
            author: bob.fp.clone(),
            issued_at: 1,
            expires_at: None,
            epoch: 0,
            prev_hash: admit_bob.chain_hash(),
            wrapped_key: None,
            new_pubkey: Some(owner.pubkey),
            new_wrap_pubkey: Some(owner.wrap_pub()),
            recovery_pubkey: None,
        };
        let sig = op.sign(&bob.secret);
        let attack = SignedMembershipOp {
            op,
            sig,
            author_pubkey: bob.pubkey,
        };
        let m = derive_valid_members(&[g, admit_bob, attack], &owner.pubkey, 100);
        assert_eq!(
            m.get(&owner.fp).map(|x| x.role),
            Some(Role::Owner),
            "owner NOT downgraded by a clobber-rebind"
        );
        assert_eq!(
            m.get(&bob.fp).map(|x| x.role),
            Some(Role::Editor),
            "bob unchanged (his rebind was not honored)"
        );
    }

    /// Fingerprint binding: a Rebind whose `new_pubkey` does NOT hash to `subject` is not
    /// honored (a member can't endorse an unrelated key under a fingerprint they chose).
    #[test]
    fn rebind_with_unbound_successor_key_is_rejected() {
        let owner = id(1);
        let bob = id(2);
        let claimed = id(3); // the fingerprint bob claims to rotate to
        let actual = id(4); // but publishes a DIFFERENT key
        let g = genesis(&owner);
        let admit = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let op = MembershipOp {
            kb_id: "KB".into(),
            action: MembershipAction::Rebind,
            subject: claimed.fp.clone(),
            role: None,
            can_invite: false,
            author: bob.fp.clone(),
            issued_at: 1,
            expires_at: None,
            epoch: 0,
            prev_hash: admit.chain_hash(),
            wrapped_key: None,
            new_pubkey: Some(actual.pubkey), // mismatched: fp(actual) != claimed.fp
            new_wrap_pubkey: Some(actual.wrap_pub()),
            recovery_pubkey: None,
        };
        let sig = op.sign(&bob.secret);
        let bad = SignedMembershipOp {
            op,
            sig,
            author_pubkey: bob.pubkey,
        };
        let m = derive_valid_members(&[g, admit, bad], &owner.pubkey, 100);
        assert!(
            m.contains_key(&bob.fp),
            "bob not rotated by an unbound rebind"
        );
        assert!(!m.contains_key(&claimed.fp));
    }

    /// Order independence (N-peer convergence): the derived set is identical regardless of
    /// the order ops arrive in (the resolver is causal, relay-independent).
    #[test]
    fn rebind_derivation_is_order_independent() {
        let owner = id(1);
        let bob = id(2);
        let bob2 = id(3);
        let g = genesis(&owner);
        let admit = make(
            &owner,
            MembershipAction::Admit,
            &bob.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let rebind = make_rebind(&bob, &bob2, &admit.chain_hash());
        let forward = derive_valid_members(
            &[g.clone(), admit.clone(), rebind.clone()],
            &owner.pubkey,
            100,
        );
        let reversed = derive_valid_members(&[rebind, admit, g], &owner.pubkey, 100);
        assert_eq!(forward, reversed, "derivation is order-independent");
    }

    /// Owner rotation: a rotated OWNER keeps authoring owner-rooted ops. After owner→owner2,
    /// an `e2e` enable + a member admit authored by owner2 are honored (the owner-principal
    /// chain resolves the successor), and the predecessor owner is retired from membership.
    #[test]
    fn owner_rotation_is_honored_by_owner_rooted_readers() {
        use crate::kb::Encryption;
        let owner = id(1);
        let owner2 = id(2);
        let carol = id(3);
        let g = genesis(&owner);
        let rebind = make_rebind(&owner, &owner2, &g.chain_hash());
        // owner2 (the successor) enables e2e + admits carol.
        let enable = make(
            &owner2,
            MembershipAction::SetEncryption,
            "e2e",
            None,
            false,
            None,
            &rebind.chain_hash(),
        );
        let admit_carol = make(
            &owner2,
            MembershipAction::Admit,
            &carol.fp,
            Some(Role::Editor),
            false,
            None,
            &enable.chain_hash(),
        );
        let ops = [g, rebind, enable, admit_carol];
        // derive_encryption resolves the owner across the rotation.
        assert_eq!(
            derive_encryption(&ops, &owner.pubkey),
            Encryption::E2e,
            "rotated owner can still latch e2e"
        );
        // Membership: owner retired, owner2 is Owner, carol admitted by owner2.
        let m = derive_valid_members(&ops, &owner.pubkey, 100);
        assert!(!m.contains_key(&owner.fp), "predecessor owner retired");
        assert_eq!(m.get(&owner2.fp).map(|x| x.role), Some(Role::Owner));
        assert!(
            m.contains_key(&carol.fp),
            "successor owner can still admit members"
        );
    }

    #[test]
    fn is_owner_principal_accepts_genesis_and_chained_successors_rejects_others() {
        let owner = id(1);
        let owner2 = id(2);
        let owner3 = id(3);
        let member = id(4);
        let stranger = id(5);
        let g = genesis(&owner);
        let admit_member = make(
            &owner,
            MembershipAction::Admit,
            &member.fp,
            Some(Role::Editor),
            false,
            None,
            &g.chain_hash(),
        );
        let rebind1 = make_rebind(&owner, &owner2, &admit_member.chain_hash()); // owner → owner2
        let rebind2 = make_rebind(&owner2, &owner3, &rebind1.chain_hash()); // owner2 → owner3 (chained)
        let ops = [g, admit_member, rebind1, rebind2];

        // The genesis owner AND every cross-signed successor speak for the owner.
        assert!(
            is_owner_principal(&ops, &owner.pubkey, &owner.fp),
            "genesis owner is an owner principal"
        );
        assert!(
            is_owner_principal(&ops, &owner.pubkey, &owner2.fp),
            "the first rotation successor is an owner principal"
        );
        assert!(
            is_owner_principal(&ops, &owner.pubkey, &owner3.fp),
            "a chained (owner2→owner3) successor is an owner principal"
        );

        // A plain member and an unrelated stranger are NOT — the attacker case for the
        // reactive-rewrap authority guard.
        assert!(
            !is_owner_principal(&ops, &owner.pubkey, &member.fp),
            "an admitted member is not an owner principal"
        );
        assert!(
            !is_owner_principal(&ops, &owner.pubkey, &stranger.fp),
            "a stranger never named in the log is not an owner principal"
        );
    }

    #[test]
    fn is_owner_principal_is_false_without_a_genesis_anchored_on_the_given_key() {
        // A genesis exists, but it is anchored on a DIFFERENT key than the one we ask about ⇒
        // no trusted root under this anchor ⇒ nobody (not even that genesis's own subject)
        // resolves as an owner principal for `owner`'s anchor.
        let owner = id(1);
        let other = id(2);
        let g = genesis(&other);
        let ops = [g];
        assert!(
            !is_owner_principal(&ops, &owner.pubkey, &owner.fp),
            "no genesis under this anchor ⇒ not an owner principal"
        );
        assert!(
            !is_owner_principal(&ops, &owner.pubkey, &other.fp),
            "the other genesis is not anchored on our queried key"
        );
    }

    /// Workstream A (#246) — the owner-chain fixpoint must TERMINATE on a maliciously cyclic
    /// rebind set (A→B→A) and still resolve the reachable owner principals. Guards the explicit
    /// `max_passes` bound: if the loop ever failed to terminate this test would hang.
    #[test]
    fn is_owner_principal_terminates_on_a_cyclic_rebind_set() {
        let a = id(1);
        let b = id(2);
        let g = genesis(&a);
        let ab = make_rebind(&a, &b, &g.chain_hash()); // A → B (A signs)
        let ba = make_rebind(&b, &a, &ab.chain_hash()); // B → A (B signs) — closes the cycle
        let ops = [g, ab, ba];
        // Terminates; the genesis owner + its cross-signed successor are both owner principals.
        assert!(is_owner_principal(&ops, &a.pubkey, &a.fp), "genesis owner");
        assert!(
            is_owner_principal(&ops, &a.pubkey, &b.fp),
            "successor reached before the cycle closes"
        );
        // An unrelated stranger is still not an owner principal.
        assert!(
            !is_owner_principal(&ops, &a.pubkey, &id(9).fp),
            "a stranger is never in the owner chain"
        );
    }

    /// ADR-042 (#247) — the O(n log n) `causal_order` must emit IDENTICALLY to the prior O(n²)
    /// generation-scan (deterministic replay is a hard correctness invariant: every honest peer
    /// must replay the op-log in the same order). Property-tested against a reference copy of the
    /// old impl on random op-trees (linear chains, wide fan-out, orphans).
    #[test]
    fn causal_order_matches_the_reference_impl_on_random_trees() {
        use rand::{rngs::StdRng, RngExt, SeedableRng};

        // Reference = the pre-ADR-042 generation-scan, verbatim.
        fn reference(
            by_hash: &BTreeMap<String, &SignedMembershipOp>,
            genesis: &str,
        ) -> Vec<String> {
            let mut emitted: BTreeSet<String> = BTreeSet::new();
            let mut order: Vec<String> = Vec::new();
            loop {
                let mut ready: Vec<String> = by_hash
                    .keys()
                    .filter(|h| !emitted.contains(*h))
                    .filter(|h| {
                        h.as_str() == genesis || emitted.contains(&by_hash[*h].op.prev_hash)
                    })
                    .cloned()
                    .collect();
                if ready.is_empty() {
                    break;
                }
                ready.sort();
                for h in ready {
                    emitted.insert(h.clone());
                    order.push(h);
                }
            }
            order
        }

        let owner = id(1);
        for seed in 0..64u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            // Build a random tree: op 0 = genesis (prev ""); each later op's parent is a random
            // earlier op; distinct subject per op ⇒ distinct chain_hash (no map collision).
            let mut ops: Vec<SignedMembershipOp> = vec![genesis(&owner)];
            let n: usize = rng.random_range(0..30);
            for i in 0..n {
                let parent = ops[rng.random_range(0..ops.len())].chain_hash();
                let subj = id(((i % 240) + 2) as u8).fp;
                ops.push(make(
                    &owner,
                    MembershipAction::Admit,
                    &subj,
                    Some(Role::Viewer),
                    false,
                    None,
                    &parent,
                ));
            }
            // Occasionally add an ORPHAN (prev points nowhere) — both impls must drop it.
            if seed % 3 == 0 {
                let subj = id(255).fp;
                ops.push(make(
                    &owner,
                    MembershipAction::Admit,
                    &subj,
                    Some(Role::Viewer),
                    false,
                    None,
                    "dangling-parent-hash",
                ));
            }
            let by_hash: BTreeMap<String, &SignedMembershipOp> =
                ops.iter().map(|o| (o.chain_hash(), o)).collect();
            let genesis_h = ops[0].chain_hash();
            assert_eq!(
                causal_order(&by_hash, &genesis_h),
                reference(&by_hash, &genesis_h),
                "seed {seed}: causal_order must match the reference impl exactly"
            );
        }
    }
}
