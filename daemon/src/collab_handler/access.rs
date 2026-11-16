//! Membership resolution and the RBAC gate — the authorization *decision*, split
//! out of `collab_handler/mod.rs` (pure code motion).
//!
//! Sibling of `method_authz`, and the split between them is deliberate:
//! `method_authz` decides **which methods may reach a document at all**, before
//! dispatch; this module decides **what a given principal may do to a given KB**,
//! at the handler. Neither substitutes for the other, and the first two rounds of
//! MAE's cross-tenant bugs came from assuming one of them covered both.

use super::*;
use crate::storage::AnchorSource;

/// here. Resolves the caller's role from its cryptographic **principal** (key
/// fingerprint — never a label), then decides by hierarchical RBAC role × the
/// KB's join policy × the operation. `principal == None` (the `none`/loopback
/// auth mode) is connection-level-trusted and blanket-allowed (dev only — real
/// per-identity policy requires `key` mode).
pub(crate) async fn kb_access(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    op: KbOp,
    transport: Transport,
) -> Result<AccessDecision, Refusal> {
    kb_access_with_coll(doc_store, kb_id, principal, op, transport, None).await
}

/// `principal`'s role in `kb_id` and its replication policy, from the same source
/// [`kb_access`] decides on: the SIGNED op-log under its declared governance for an
/// anchored KB, else the locally-authoritative `member_roles`. `None` = not a member.
///
/// Shared with `kb/query.my_kbs` so the role a caller is SHOWN can never come from
/// a different source than the role they are GATED on (principle #8): an anchored
/// KB's unsigned `member_roles` may disagree with its op-log.
pub(crate) async fn member_role(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
    principal: &str,
) -> (Option<SyncRole>, ReplicationPolicy) {
    match signed_membership_anchor(doc_store, kb_id, coll).await {
        Some(anchor) => {
            // ADR-026 §A4: read the owner-declared governance from the op-log, then
            // derive membership under it — so a `Quorum{m}` KB enforces m-of-n
            // co-signed removals (and an Owner removed by quorum loses access here)
            // exactly as every honest peer derives it. `SingleOwner` (the default)
            // reduces to the prior single-author rule.
            // ADR-042 (#247): membership derivation is memoized in `derived_membership` — an
            // unchanged op-log (state-vector) + anchor + timebox horizon returns the cached set
            // without re-decoding the whole op-log. This gate runs on every anchored/E2E access;
            // the cache is what keeps it O(1) at membership-churn scale.
            let dm_scope = doc_store
                .derived_membership(kb_id, coll, &anchor, now_unix())
                .await;
            match dm_scope.members.get(principal) {
                Some(m) => (Some(m.role), m.replication),
                None => (None, ReplicationPolicy::Full),
            }
        }
        None => (coll.role_of(principal), ReplicationPolicy::Full),
    }
}

/// Where a KB's membership is read from — the ONE decision every membership
/// reader shares (`kb_access`, the epoch fence, content-op admission, the
/// member set), so access and verification can never read different sources
/// (ADR-036 Amendment 1 F2). `Some(anchor)`: the signed op-log, derived under
/// that anchor — always for a JOINED KB (empty without a genesis: fail closed),
/// and for a hub-recorded anchor only when the log is rooted at a genesis that
/// anchor signed. `None`: the locally authoritative roster (`member_roles`), on
/// a KB without an anchor, or a hub-recorded one whose log has no such root.
///
/// @ai-caution: [security] "Verified genesis or fallback", for HUB-RECORDED
/// anchors only (the owner's ruling on the F1+F2 lockout; this hub is the
/// authority for those member lists): a log with no anchor-signed root derives NO members,
/// so selecting it would lock every member out — and a member can create such
/// a log on its own (a self-`Rebind` appended to a KB with no genesis). No
/// member op may move a KB off its roster; only the owner's genesis does, and
/// authoring one later is a separate step.
pub(crate) async fn signed_membership_anchor(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
) -> Option<[u8; 32]> {
    let (anchor, source) = doc_store.kb_anchor_record(kb_id).await?;
    if source == AnchorSource::Joined {
        // ADR-026: fail closed. The member list came from a relay; without
        // a genesis the anchor signed the log derives no members, and that is
        // the answer — a relay never substitutes its own list.
        return Some(anchor);
    }
    coll.oplog_head()?;
    let derived = doc_store
        .derived_membership(kb_id, coll, &anchor, now_unix())
        .await;
    derived.genesis_verified.then_some(anchor)
}

/// Where a KB's membership is read from, and who that leaves out — the owner's
/// ruling on F2's two sources: "log wins, made visible".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MembershipSource {
    /// The signed op-log governs ([`signed_membership_anchor`] chose it);
    /// otherwise the member list (`member_roles`) does.
    pub log_governed: bool,
    /// On a log-governed KB, the member-list entries the log does not admit,
    /// sorted. They have no access here. Always empty when the list governs.
    pub roster_only: Vec<String>,
}

/// [`MembershipSource`] for `kb_id` from its collection `coll`.
pub(crate) async fn membership_source(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
) -> MembershipSource {
    let Some(anchor) = signed_membership_anchor(doc_store, kb_id, coll).await else {
        return MembershipSource::default();
    };
    let derived = doc_store
        .derived_membership(kb_id, coll, &anchor, now_unix())
        .await;
    let mut roster_only: Vec<String> = coll
        .member_roles()
        .into_iter()
        .map(|m| m.fingerprint)
        .filter(|fp| !derived.members.contains_key(fp))
        .collect();
    roster_only.sort();
    roster_only.dedup();
    MembershipSource {
        log_governed: true,
        roster_only,
    }
}

/// [`MembershipSource`] for a stored KB — `mae-daemon kb anchor` and
/// `mae-daemon doctor` (operator-only, offline).
pub async fn kb_membership_source(
    doc_store: &DocStore,
    kb_id: &str,
) -> Result<MembershipSource, String> {
    let coll = load_collection(doc_store, kb_id)
        .await
        .map_err(|e| format!("KB '{kb_id}': {}", McpError::from(e).message))?;
    Ok(membership_source(doc_store, kb_id, &coll).await)
}

/// Every anchored KB whose signed log leaves member-list entries out, with
/// those entries (`mae-daemon doctor`). A KB whose collection cannot be read
/// is reported with the reason instead.
pub async fn roster_only_members_report(
    doc_store: &DocStore,
) -> Vec<(String, Result<Vec<String>, String>)> {
    let mut out = Vec::new();
    for kb_id in doc_store.anchored_kb_ids().await {
        match kb_membership_source(doc_store, &kb_id).await {
            Ok(src) if src.roster_only.is_empty() => {}
            Ok(src) => out.push((kb_id, Ok(src.roster_only))),
            Err(e) => out.push((kb_id, Err(e))),
        }
    }
    out
}

/// Log, at warn level, that anchoring `kb_id` left `roster_only` members out —
/// the count only; the fingerprints at debug (the operator CLI prints them).
pub(crate) fn warn_roster_only(kb_id: &str, roster_only: &[String]) {
    if roster_only.is_empty() {
        return;
    }
    warn!(
        kb_id,
        count = roster_only.len(),
        "KB anchored: its signed op-log governs membership, and member-list entries it does \
         not admit have no access; the owner must admit them with a signed op (`mae-daemon kb \
         anchor --kb-id` lists them)"
    );
    debug!(kb_id, members = ?roster_only, "member-list entries the signed op-log does not admit");
}

/// Refuse a membership change that would land on the member list only (the
/// owner's "log wins, made visible": no member is added or removed silently).
/// On a log-governed KB a change takes effect only as a signed op in the log;
/// `apply_membership_change` appends one only when this daemon signs as the
/// owner, so it must also be the key the log is rooted at. `can_sign`: the
/// caller appends a signed op when it can (false for a write that never does).
///
/// @ai-caution: [security] Every member-list writer of membership calls this —
/// `kb/add_member`, `kb/remove_member`, `kb/approve_member`, a permissive
/// `kb/join`'s auto-admit, and the operator's `kb member`. A writer that skips
/// it reports success for a member the log never admits (or keeps admitting).
pub(crate) async fn refuse_roster_only_change(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
    can_sign: bool,
) -> Result<(), McpError> {
    let Some(anchor) = signed_membership_anchor(doc_store, kb_id, coll).await else {
        return Ok(());
    };
    let signs_for_log = can_sign
        && doc_store
            .signer()
            .is_some_and(|s| s.fingerprint() == coll.owner() && s.public().to_bytes() == anchor);
    if signs_for_log {
        return Ok(());
    }
    Err(McpError::signer_unavailable(format!(
        "KB '{kb_id}''s membership is governed by its signed op-log, and this daemon cannot sign \
         that change into it: the owner must add or remove the member with a signed membership \
         op (ADR-036 Amendment 1 F2)"
    )))
}

/// As [`kb_access`], but accepts a pre-loaded collection snapshot so a handler
/// running several gates on one request loads `kbc:{kb_id}` once. `None` loads
/// itself — identical to [`kb_access`]. The `None`-principal (loopback) case
/// returns before any load either way.
pub(crate) async fn kb_access_with_coll(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    op: KbOp,
    transport: Transport,
    coll: Option<&KbCollectionDoc>,
) -> Result<AccessDecision, Refusal> {
    let principal = match principal {
        Some(p) => p,
        None => return Ok(AccessDecision::Allow),
    };
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => {
            loaded = load_collection(doc_store, kb_id).await?;
            &loaded
        }
    };
    // ADR-026: for a KB JOINED from a relay we don't trust, an external anchor (the
    // join-ticket node-id) is registered — derive membership from the SIGNED op-log
    // rather than the relay-supplied `member_roles`. Owned / un-anchored KBs keep
    // the locally-authoritative legacy `member_roles` (the daemon owns that state).
    // ADR-067: `replication` rides alongside `role` from the SAME lookup (not a
    // second `derived_membership` call) -- `Full` for the legacy/un-anchored path,
    // since that path has no signed op-log to carry the field in at all (an
    // explicit, named scope boundary, not a silent gap: retrofitting tamper-evidence
    // onto the unsigned `member_roles` map would be the exact spoofing risk ADR-067
    // Phase A's signed-field design avoids).
    let (role, replication) = member_role(doc_store, kb_id, coll, principal).await;
    // Per-KB transport policy (ADR-018/025): a KB is reachable over a transport
    // only if its policy exposes it there — EXCEPT the owner, who always reaches
    // their own KB (e.g. their local editor over the hub socket) and is the one who
    // manages exposure. Non-owner members + would-be joiners are transport-gated.
    if role != Some(SyncRole::Owner) && !coll.transport_policy().allows(transport) {
        let t = match transport {
            Transport::Hub => "the hub",
            Transport::P2p => "the P2P mesh",
        };
        return Ok(AccessDecision::Deny(Refusal::new(
            RefusalKind::TransportForbidden { transport },
            format!("KB '{kb_id}' is not shared over {t}"),
        )));
    }
    match role {
        Some(role) => {
            // ADR-067: a QueryOnly-restricted member has full Read access (checked
            // below, unconditional) but may NOT replicate the KB locally via
            // kb_join — checked BEFORE the general RBAC match so its denial message
            // is specific and distinguishable both from a plain role-insufficiency
            // denial (this member's role is perfectly adequate; only replication is
            // restricted) and from the "not a member" denial a non-member joiner
            // gets below (telling a genuine, restricted member they're "not a
            // member" would be actively misleading).
            if op == KbOp::Join && replication == ReplicationPolicy::QueryOnly {
                return Ok(AccessDecision::Deny(Refusal::new(
                    RefusalKind::ReplicationRestricted,
                    format!(
                        "member is restricted to live-query-only access for KB '{kb_id}' \
                         and may not replicate it locally (ADR-067)"
                    ),
                )));
            }
            // Hierarchical RBAC: owner ⊇ editor ⊇ viewer.
            let required = match op {
                KbOp::Join | KbOp::Read => SyncRole::Viewer,
                KbOp::Edit => SyncRole::Editor,
                KbOp::Manage => SyncRole::Owner,
            };
            if role.includes(required) {
                Ok(AccessDecision::Allow)
            } else {
                Ok(AccessDecision::Deny(Refusal::new(
                    RefusalKind::RoleForbidden { required },
                    format!("role '{}' may not {:?} KB '{kb_id}'", role.as_str(), op),
                )))
            }
        }
        None => match op {
            // Non-member join is governed by the KB's join policy.
            KbOp::Join => match coll.join_policy() {
                JoinPolicy::Permissive => Ok(AccessDecision::AllowAutoJoin),
                JoinPolicy::Invite => Ok(AccessDecision::Pending),
                JoinPolicy::Restrictive => Ok(AccessDecision::Deny(Refusal::not_member(format!(
                    "not a member of KB '{kb_id}'"
                )))),
            },
            _ => Ok(AccessDecision::Deny(Refusal::not_member(format!(
                "not a member of KB '{kb_id}'"
            )))),
        },
    }
}

/// ADR-053/Phase G (#382): the one narrow public entry point into this module's
/// access engine, for the OAuth HTTPS listener (`daemon/src/oauth.rs`, a sibling
/// module in the **binary** crate — `kb_access`/`kb_access_with_coll` are private to
/// this **library**-crate module and unreachable from there otherwise). Deliberately
/// hardcodes `KbOp::Read` — this wrapper structurally cannot be used for `Edit`/
/// `Manage`, so exposing it can never widen the collab access engine's surface beyond
/// read access, regardless of what a future caller passes in. `principal` is
/// expected to always be `Some` in practice for an OAuth caller (every validated
/// bearer token yields a real `ValidatedPrincipal.principal` string); `None` inherits
/// `kb_access`'s existing loopback-trusted semantics (N/A here, kept only so this
/// wrapper's behavior never silently diverges from the function it wraps).
pub async fn check_kb_read_access(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    transport: Transport,
) -> Result<AccessDecision, Refusal> {
    kb_access(doc_store, kb_id, principal, KbOp::Read, transport).await
}

/// The current member principals as the daemon derives them for `kb_id` — the same
/// anchored-vs-legacy split [`kb_access`] uses: an anchored, op-logged KB derives the
/// set from the SIGNED op-log under its declared governance; an owned / un-anchored KB
/// reads the locally-authoritative `member_roles`. Used by the member-`Rebind` gate to
/// confirm the rotating author is a current member and the successor is fresh.
pub(crate) async fn current_member_set(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
) -> std::collections::BTreeSet<String> {
    match signed_membership_anchor(doc_store, kb_id, coll).await {
        Some(anchor) => doc_store
            .derived_membership(kb_id, coll, &anchor, now_unix())
            .await
            .members
            .keys()
            .cloned()
            .collect(),
        None => coll
            .member_roles()
            .into_iter()
            .map(|m| m.fingerprint)
            .collect(),
    }
}

/// ADR-040 PR2c/PR3 — the member-authored **self-service** write gate. `kb/collection_op`
/// is otherwise owner-only (`KbOp::Manage`, ADR-018); this is the *single, narrow*
/// exception that lets a NON-owner member manage their **own** identity (rotation +
/// recovery-key registration + recovery rotation) without owner mediation. It accepts the
/// update **iff** every op it introduces is one of those three self-service shapes and the
/// update mutates **nothing else** in the collection — so a member cannot smuggle a
/// privilege change (an `Admit`, a `SetRole`, an owner flip) alongside it. Concretely,
/// applying `update` to the stored collection must (1) grow the op-log by ≥1 record and
/// change **only** the op-log (owner / member roster / policies / encryption byte-identical
/// before and after); and (2) introduce only NEW ops that are each exactly one of these three
/// self-service shapes:
///
/// - **member self-`Rebind`** — crypto-valid (`verify_signed`), `author` == the connection's
///   authenticated principal (you rotate yourself) AND a current member, with a well-formed
///   non-elevating successor.
/// - **member `RegisterRecoveryKey`** — crypto-valid (primary-signed), `author` == `subject`
///   == the principal AND a current member, carrying a `recovery_pubkey` (grants no roster
///   access — it just publishes the offline recovery key for a future recovery).
/// - **recovery-signed `Rebind`** (ADR-040 §Recovery-key) — signed NOT by the lost primary but
///   by the predecessor's *registered* recovery key (validated against the recovery registry
///   built from the **pre-existing** op-log), and submitted by the SUCCESSOR key's
///   authenticated connection (`subject` == principal). The lost-primary path: the holder of
///   the offline recovery key rotates a member that can no longer self-sign. The predecessor
///   must be a current member and the successor well-formed/fresh.
///
/// The self-rotation + recovery arms mirror [`membership::authorized`]'s `Rebind` arm + [`membership::crypto_valid`]'s
/// recovery filter; we re-check here because the daemon — not just the deriving peers — is
/// now an authorization point for these ops. `auth_principal` is the verified session
/// principal; `None` (un-authed local socket) never reaches here because the owner-`Manage`
/// check already allowed it. On success returns the accepted `(successor_fp, predecessor_fp)`
/// rebind pairs (rotation + recovery; registration contributes none) so the caller can mirror
/// each successor into the owned-KB roster (inheriting the predecessor's role), giving it
/// access on a roster-model daemon — the derive-based peers already alias it via the PR2a/PR3
/// post-pass.
/// Extract the `(successor, predecessor)` pairs of the authenticated principal's OWN
/// self-`Rebind`s from a collection `update`, for mirroring an **owner's** rotation into
/// the roster (#265).
///
/// Unlike [`verify_member_self_service_update`] — which requires the WHOLE update be a bare
/// self-service op and is only reached when `Manage` DENIES — an owner reaches the caller
/// via `Manage = Allow` and is authorized to make OTHER changes in the same update (e.g.
/// re-wrapping the E2E content key to the new key). So we cannot run the strict
/// append-only/every-op-is-self-service gate; instead we scan for JUST the caller's own
/// valid self-`Rebind`s and ignore the rest. A pair is emitted ONLY when the new `Rebind`
/// is authored by `principal`, self-signed by `principal`'s primary, and binds a fresh
/// successor key to its fingerprint — so this can never inject an arbitrary member (the
/// predecessor is always the authenticated caller, and the successor inherits the caller's
/// own role). Best-effort: any decode/apply failure yields no pairs (the op-log write still
/// happens; only the roster mirror is skipped).
pub(crate) async fn owner_self_rebind_pairs(
    doc_store: &DocStore,
    kb_id: &str,
    principal: &str,
    update: &[u8],
) -> Vec<(String, String)> {
    if principal.is_empty() {
        return Vec::new();
    }
    let collection_doc = format!("kbc:{kb_id}");
    let Ok((state, _sv)) = doc_store.encode_state_and_sv(&collection_doc).await else {
        return Vec::new();
    };
    let Ok(before) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    let Ok(mut after) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    if after.apply_update(update).is_err() {
        return Vec::new();
    }
    let before_hashes: HashSet<String> =
        before.oplog_ops().iter().map(|o| o.chain_hash()).collect();
    let mut pairs = Vec::new();
    for o in after.oplog_ops() {
        if before_hashes.contains(&o.chain_hash()) {
            continue; // only NEW ops
        }
        if o.op.action != MembershipAction::Rebind {
            continue;
        }
        let Some(npk) = o.op.new_pubkey else { continue };
        // The caller's OWN self-rotation: predecessor (author) is the authenticated
        // principal, self-signed by its primary, successor bound to a fresh key.
        if o.op.author != principal || o.op.subject == o.op.author {
            continue;
        }
        if fingerprint_of(&npk) != o.op.subject || !o.verify_signed() {
            continue;
        }
        pairs.push((o.op.subject.clone(), o.op.author.clone()));
    }
    pairs
}

pub(crate) async fn verify_member_self_service_update(
    doc_store: &DocStore,
    kb_id: &str,
    auth_principal: Option<&str>,
    update: &[u8],
) -> Result<Vec<(String, String)>, String> {
    let principal = auth_principal
        .ok_or_else(|| "member self-rotation requires an authenticated principal".to_string())?;
    let collection_doc = format!("kbc:{kb_id}");
    let (state, _sv) = doc_store
        .encode_state_and_sv(&collection_doc)
        .await
        .map_err(|e| format!("KB '{kb_id}' not found: {e}"))?;
    let before = KbCollectionDoc::from_bytes(&state).map_err(|e| format!("bad collection: {e}"))?;
    let mut after =
        KbCollectionDoc::from_bytes(&state).map_err(|e| format!("bad collection: {e}"))?;
    after
        .apply_update(update)
        .map_err(|e| format!("collection update did not apply: {e}"))?;

    // (1) The update must touch ONLY the op-log. Authority on the daemon derives from
    // the op-log (anchored) or the `member_roles` roster (owned/legacy) plus the owner
    // and the policy/encryption fields — pin every one of them so a rebind cannot ride
    // a roster or policy mutation. The roster is compared as a SET (keyed by
    // fingerprint): `member_roles()` is a yrs-map projection whose Vec order is not
    // stable across decodes, so an order-sensitive `!=` would false-positive.
    let roster_of =
        |c: &KbCollectionDoc| -> std::collections::BTreeMap<String, (SyncRole, String)> {
            c.member_roles()
                .into_iter()
                .map(|m| (m.fingerprint, (m.role, m.label)))
                .collect()
        };
    if after.owner() != before.owner()
        || roster_of(&after) != roster_of(&before)
        || after.join_policy() != before.join_policy()
        || after.transport_policy_raw() != before.transport_policy_raw()
        || after.encryption() != before.encryption()
        || after.creator() != before.creator()
    {
        return Err(
            "a member self-rotation may not modify the owner, member roster, policy, \
             or encryption state of the collection"
                .to_string(),
        );
    }

    // (2) Compute the op-log delta and require every NEW op be one of the three
    // self-service shapes. The recovery registry is built from the *pre-existing*
    // op-log (`before`) so a recovery key must already be registered to authorize a
    // recovery rotation — a registration cannot be smuggled into the same update to
    // self-authorize (the registration itself requires a primary signature, which the
    // recovering principal lacks).
    let before_ops = before.oplog_ops();
    let before_hashes: HashSet<String> = before_ops.iter().map(|o| o.chain_hash()).collect();
    let registry = recovery_registry(&before_ops);
    let after_ops = after.oplog_ops();
    let after_hashes: HashSet<String> = after_ops.iter().map(|o| o.chain_hash()).collect();
    // The membership op-log is APPEND-ONLY: for an anchored/E2e KB the authoritative
    // membership + governance + encryption are DERIVED from it (not the manifest roster this
    // gate pins), so a self-service update must never DELETE a pre-existing op. Without this,
    // a member could ride a valid self-`Rebind` while dropping a co-member's `Admit`, the
    // owner's `SetEncryption("e2e")` (an ADR-039 anti-downgrade attack), or the genesis (DoS)
    // — none of which touch the pinned manifest fields. Reject any update that loses a prior op.
    if !before_hashes.is_subset(&after_hashes) {
        return Err(
            "a member self-service update may not remove or rewrite any existing membership \
             op — the op-log is append-only"
                .to_string(),
        );
    }
    let new_ops: Vec<_> = after_ops
        .into_iter()
        .filter(|o| !before_hashes.contains(&o.chain_hash()))
        .collect();
    if new_ops.is_empty() {
        return Err("update introduces no new membership op (not a member rotation)".to_string());
    }
    let members = current_member_set(doc_store, kb_id, &before).await;
    let mut pairs = Vec::with_capacity(new_ops.len());
    for o in &new_ops {
        match o.op.action {
            MembershipAction::Rebind => {
                // Common successor validity (shapes a + c): well-formed, fingerprint-bound,
                // non-self, fresh, with the predecessor a current member.
                let npk = match o.op.new_pubkey {
                    Some(k) => k,
                    None => {
                        return Err("rotation op is missing the successor public key".to_string())
                    }
                };
                if o.op.new_wrap_pubkey.is_none() {
                    return Err("rotation op is missing the successor wrap key".to_string());
                }
                if fingerprint_of(&npk) != o.op.subject {
                    return Err("rotation successor is not bound to its public key".to_string());
                }
                if o.op.subject == o.op.author {
                    return Err("rotation successor equals the author (no-op)".to_string());
                }
                if !members.contains(&o.op.author) {
                    return Err("rotation predecessor is not a current member".to_string());
                }
                if members.contains(&o.op.subject) {
                    return Err(
                        "rotation successor is already a member (must rotate into a fresh key)"
                            .to_string(),
                    );
                }
                if o.verify_signed() {
                    // (a) self-rotation: signed by the rotating principal's own primary, so
                    // the op author must be the authenticated connection principal.
                    if o.op.author != principal {
                        return Err("a member may only rotate their own identity".to_string());
                    }
                } else if is_recovery_rebind(o, &registry) {
                    // (c) recovery rotation: signed by the predecessor's *registered* recovery
                    // key (the lost primary cannot self-sign), submitted by the SUCCESSOR key's
                    // authenticated connection — so the recovering user proves control of the new
                    // key it is rotating into, and the recovery key proves the authority to do so.
                    if o.op.subject != principal {
                        return Err(
                            "a recovery rotation must be submitted by the successor key it rotates into"
                                .to_string(),
                        );
                    }
                } else {
                    return Err(
                        "rotation op is neither self-signed nor signed by a registered recovery key"
                            .to_string(),
                    );
                }
                pairs.push((o.op.subject.clone(), o.op.author.clone()));
            }
            MembershipAction::RegisterRecoveryKey => {
                // (b) recovery-key registration: primary-signed self-registration. Grants no
                // roster access — it only publishes the offline recovery key for a future (c).
                if !o.verify_signed() {
                    return Err("recovery-key registration signature is invalid".to_string());
                }
                if o.op.author != principal || o.op.subject != principal {
                    return Err("a member may only register their OWN recovery key".to_string());
                }
                if o.op.recovery_pubkey.is_none() {
                    return Err(
                        "recovery-key registration is missing the recovery public key".to_string(),
                    );
                }
                if !members.contains(&o.op.author) {
                    return Err("recovery-key registrant is not a current member".to_string());
                }
            }
            other => {
                return Err(format!(
                    "a member may only author a Rebind or RegisterRecoveryKey on this path, \
                     not a {} op",
                    other.as_str()
                ));
            }
        }
    }
    Ok(pairs)
}
