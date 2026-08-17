//! KB identity: which KB a collaborative id or a user-typed name refers to
//! (ADR-105 D4).
//!
//! Split out of `federation.rs` — where `KbRegistry` and `KbInstance` are defined
//! — because this is a distinct concern and that file is at its structural
//! ceiling. The `impl KbRegistry` block below is the same type, continued here.
//!
//! The distinction this module exists to keep straight: a KB has a display NAME
//! that a human types and that is not unique (every editor's primary is called
//! "default"), and a collab ID that is minted once, is unique, and is signed into
//! every membership op. Conflating them made a shared daemon effectively
//! single-tenant — see `collab_id_for_share`.

use crate::federation::{generate_uuid, KbRegistry};

/// The names a user may type to mean the primary KB (ADR-105 D4).
///
/// Spelled here, in the lowest crate that needs them, because this crate already
/// compared against a bare `"primary"` (`set_ai_residency`) while `mae-core`
/// compared against `KB_DEFAULT_NAME || "primary"` — two places, two different
/// answers for the same question. `mae-core`'s constant is pinned against this
/// list by a test there, so the two cannot drift apart silently.
///
/// These are DISPLAY names. Nothing here is an identifier: see
/// [`KbRegistry::collab_id_for_share`] for why the distinction is load-bearing.
pub const PRIMARY_NAME_ALIASES: &[&str] = &["default", "primary"];

/// Which KB a collaborative id or a user-typed name refers to (ADR-105 D4).
///
/// Exists because the primary KB is not one of `KbRegistry::instances` — it has no
/// `KbInstance` row, and its durable markers (`primary_shared`, `primary_collab_id`)
/// live on the registry itself. Every "which KB is this?" answer therefore has two
/// shapes, and code that models it as one (a name compared against `"default"`, an
/// `Option<uuid>` where `None` means primary) has repeatedly gone wrong at exactly
/// the seam. Naming the two cases makes the compiler carry the distinction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbTarget {
    /// The machine-global primary KB.
    Primary,
    /// A registered federated instance, by its registry `uuid` (never its name —
    /// names are user-facing and mutable, uuids are the registry's own key).
    Instance(String),
}

impl KbRegistry {
    /// Which KB does a user-typed name refer to? (ADR-105 D4.)
    ///
    /// The counterpart to [`target_of_collab_id`](Self::target_of_collab_id), for
    /// the other direction ids arrive from: a human typing `:kb-share <name>`, or
    /// a keybinding defaulting to the primary. Comparing a *name* against
    /// [`PRIMARY_NAME_ALIASES`] is correct here and only here — the D4 defect was
    /// using a name as an *identifier*, not offering the user a name at all.
    ///
    /// Returns `None` for a name that matches no registered instance, so a typo
    /// surfaces as "no such KB" instead of silently resolving to the primary.
    pub fn target_of_name(&self, name: &str) -> Option<KbTarget> {
        if PRIMARY_NAME_ALIASES
            .iter()
            .any(|a| name.eq_ignore_ascii_case(a))
        {
            return Some(KbTarget::Primary);
        }
        self.find(name).map(|i| KbTarget::Instance(i.uuid.clone()))
    }

    /// Which KB does this collaborative id name? (ADR-105 D4/H3.)
    ///
    /// **The predicate that replaces `kb_id == KB_DEFAULT_NAME || kb_id == "primary"`.**
    /// That comparison was correct only for as long as every editor's primary
    /// synced under the literal `"default"` — which is precisely the defect D4
    /// removes, since it meant the second tenant to connect could never share
    /// their own primary. Once a primary syncs under a minted id, a name
    /// comparison stops recognising it, and the failure is silent: the caller
    /// falls through to the instance branch, finds nothing, and quietly treats a
    /// live shared KB as unknown.
    ///
    /// Answers for the primary too, which is why this exists rather than callers
    /// using [`find_by_collab_id`](Self::find_by_collab_id) directly: the primary
    /// KB has no `KbInstance` row at all (its durable markers live on `KbRegistry`
    /// itself), so it is invisible to every `instances`-based lookup.
    ///
    /// A KB shared before D4 stored `primary_collab_id = Some("default")`, so it
    /// resolves through the same path with no special case and no migration.
    pub fn target_of_collab_id(&self, collab_id: &str) -> Option<KbTarget> {
        if self.primary_collab_id.as_deref() == Some(collab_id) {
            return Some(KbTarget::Primary);
        }
        self.find_by_collab_id(collab_id)
            .map(|i| KbTarget::Instance(i.uuid.clone()))
    }

    /// The collaborative id `target` is currently shared under, if it is shared.
    ///
    /// The read half of [`target_of_collab_id`](Self::target_of_collab_id).
    /// `None` means "not shared yet", which is the caller's cue to mint one —
    /// never a cue to fall back to a shared constant.
    pub fn collab_id_of_target(&self, target: &KbTarget) -> Option<String> {
        match target {
            KbTarget::Primary => self.primary_collab_id.clone(),
            KbTarget::Instance(uuid) => self.find_by_uuid(uuid).and_then(|i| i.collab_id.clone()),
        }
    }

    /// The collaborative id to share `target` under, minting one on first share
    /// (ADR-105 D4).
    ///
    /// **An existing id is returned unchanged, always.** That is not an
    /// optimisation, it is a correctness requirement: `kb_id` is the second field
    /// of `MembershipOp::canonical_bytes`, so it is covered by every membership
    /// signature. Change it and `derive_valid_members` matches nothing — the KB's
    /// membership evaporates — while `derive_encryption` returns `None`, which
    /// makes an end-to-end-encrypted KB silently read as plaintext (#573's exact
    /// failure). A KB's id is therefore write-once, for the life of the KB.
    ///
    /// A first share mints an opaque uuid instead of reusing the KB's display
    /// name. The name was never an identifier: every editor's primary is called
    /// `"default"`, so on a shared daemon the first tenant to connect claimed that
    /// id permanently and every later tenant's primary share was accepted and then
    /// denied on every subsequent operation. The human-facing name still travels,
    /// as the collection's `name` metadata, where a duplicate is merely confusing
    /// rather than load-bearing.
    ///
    /// Persisting at MINT time rather than on confirmation is deliberate: a retried
    /// share must present the same id, or a share that half-succeeded (the daemon
    /// created `kbc:{id}` before the response was lost) would strand that
    /// collection under an id the editor never uses again. The durable *shared*
    /// marker still waits for confirmation — this stamps identity, not state.
    pub fn collab_id_for_share(&mut self, target: &KbTarget) -> String {
        if let Some(existing) = self.collab_id_of_target(target) {
            return existing;
        }
        let minted = self.mint_unused_collab_id();
        self.set_collab_id(target, &minted);
        minted
    }

    /// Discard `target`'s collab id and mint a fresh one (ADR-105 D4).
    ///
    /// **The recovery path for an id that was minted but never confirmed** — the
    /// daemon answered `KB_ID_OWNED_BY_ANOTHER`, so this id belongs to someone
    /// else and every retry under it will be refused identically. Without this the
    /// KB is permanently unshareable: `collab_id_for_share` correctly returns an
    /// existing id unchanged, so a poisoned id is re-presented forever and the only
    /// fix is hand-editing `kb-registry.toml`.
    ///
    /// Returns `None` — refusing to re-mint — when the KB **has a confirmed share**
    /// under that id. That case is not a collision: it means a KB we genuinely own
    /// is reported as someone else's, i.e. we are talking to the wrong daemon or
    /// the id was taken over. Re-minting there would destroy our own signed
    /// membership and read an E2E KB as plaintext (finding A) — the very thing the
    /// write-once rule exists to prevent. The caller must surface it instead.
    ///
    /// The confirmed-share marker is the right discriminator because it is stamped
    /// only on a confirmed share, so "minted but never accepted" and "ours" are
    /// exactly the two states it separates.
    pub fn remint_unconfirmed_collab_id(&mut self, target: &KbTarget) -> Option<String> {
        let confirmed = match target {
            KbTarget::Primary => self.primary_shared,
            KbTarget::Instance(uuid) => self.find_by_uuid(uuid).is_some_and(|i| i.shared),
        };
        if confirmed {
            return None;
        }
        let minted = self.mint_unused_collab_id();
        self.set_collab_id(target, &minted);
        Some(minted)
    }

    /// Mint an id no KB in THIS registry already uses.
    ///
    /// Layer one of three against collisions, and the only deterministic one: a
    /// local check needs no network and cannot be probabilistic. `generate_uuid`
    /// now draws 122 bits of OS entropy, so the loop realistically never spins —
    /// but "realistically never" is a probability, and a duplicate id silently
    /// merges two KBs' collections and node namespaces, which is too sharp an edge
    /// to leave to one. The bound stops a broken RNG from hanging the editor;
    /// exhausting it returns the last candidate, which the daemon still refuses
    /// rather than accepting a duplicate.
    fn mint_unused_collab_id(&self) -> String {
        let mut candidate = generate_uuid();
        for _ in 0..8 {
            if self.target_of_collab_id(&candidate).is_none() {
                break;
            }
            candidate = generate_uuid();
        }
        candidate
    }

    fn set_collab_id(&mut self, target: &KbTarget, id: &str) {
        match target {
            KbTarget::Primary => self.primary_collab_id = Some(id.to_string()),
            KbTarget::Instance(uuid) => {
                if let Some(inst) = self.instances.iter_mut().find(|i| &i.uuid == uuid) {
                    inst.collab_id = Some(id.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "kb_identity_tests.rs"]
mod tests;
