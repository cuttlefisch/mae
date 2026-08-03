//! Per-primitive privilege classification for the Scheme surface (ADR-084 D3).
//!
//! Every Rust function exposed to Scheme through [`crate::vm::SchemeVm::register_fn`]
//! declares what privilege it requires. The declaration is a **required argument**,
//! so a newly-added primitive cannot ship unclassified: it fails to compile until
//! its author states a tier. This is Capsicum's `fget` technique — the compiler
//! enumerates the sites, rather than a human remembering to.
//!
//! The privilege lattice itself is [`PermissionTier`], reused verbatim from
//! `mae-ai`. There is deliberately no second lattice: the tier a session is
//! capped at and the tier a primitive requires must be comparable values of the
//! same type, or the comparison at the chokepoint would be a translation layer
//! nobody audits.
//!
//! @ai-caution: [permission] Never add a `_ =>` arm to a match on [`PrimitiveTier`].
//! A defaulting wildcard would silently classify a future variant instead of
//! loudly failing to compile — which is the entire mechanism this module exists
//! to provide (JEP 441: a match-all clause "risks sweeping exhaustiveness errors
//! under the rug").
//!
//! @stability: experimental
//! @since: 0.14.89

pub use mae_ai::types::PermissionTier;

/// What privilege a Scheme-callable Rust primitive requires.
///
/// Three cases, and the distinction between them is load-bearing (AOSP's AIDL
/// model): a primitive that needs no privilege must say so *as a choice*, so it
/// is distinguishable from one nobody has classified yet, and a primitive that
/// checks its own authorization in Rust must say *that*, so it shows up in an
/// audit rather than masquerading as harmless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTier {
    /// Deliberately requires no privilege: pure computation over its arguments,
    /// no ambient effect on the editor, the host, or the network.
    ///
    /// This is an assertion, not a default. Writing it says "I looked, and this
    /// primitive cannot reach an effect."
    Unrestricted,

    /// Callable only when the ambient tier is at least this tier.
    Requires(PermissionTier),

    /// The primitive performs its own authorization check in Rust, against
    /// context the ambient tier does not carry (a per-KB role, a residency
    /// policy, a peer's membership).
    ///
    /// Recorded distinctly so `SelfEnforced` primitives are enumerable — an
    /// audit can ask "which primitives claim to police themselves?" and get an
    /// answer. Classifying one of these as `Unrestricted` would hide it.
    SelfEnforced,
}

impl PrimitiveTier {
    /// The tier the ambient authority must reach for this primitive to run,
    /// or `None` when the primitive is not gated at the chokepoint.
    ///
    /// @ai-caution: [permission] Exhaustive by construction — no `_` arm. Adding
    /// a variant must break this function.
    pub fn required(self) -> Option<PermissionTier> {
        match self {
            PrimitiveTier::Unrestricted => None,
            PrimitiveTier::Requires(t) => Some(t),
            PrimitiveTier::SelfEnforced => None,
        }
    }

    /// Stable label for audits, diagnostics, and test oracles.
    ///
    /// @ai-caution: [permission] Exhaustive by construction — no `_` arm.
    pub fn label(self) -> &'static str {
        match self {
            PrimitiveTier::Unrestricted => "unrestricted",
            PrimitiveTier::Requires(PermissionTier::ReadOnly) => "readonly",
            PrimitiveTier::Requires(PermissionTier::Write) => "write",
            PrimitiveTier::Requires(PermissionTier::Shell) => "shell",
            PrimitiveTier::Requires(PermissionTier::Privileged) => "privileged",
            PrimitiveTier::SelfEnforced => "self-enforced",
        }
    }
}

/// Shorthands for the classification written at ~516 registration sites.
///
/// These are the vocabulary of the allow-list. `tier::PURE` is the explicit
/// "no privilege required" choice — it is not the absence of a classification,
/// because there is no way to register a primitive without writing one of these.
pub mod tier {
    use super::{PermissionTier, PrimitiveTier};

    /// Pure computation: no editor state, no host, no network. An explicit choice.
    pub const PURE: PrimitiveTier = PrimitiveTier::Unrestricted;
    /// Reads editor or host state without mutating anything.
    pub const READ: PrimitiveTier = PrimitiveTier::Requires(PermissionTier::ReadOnly);
    /// Ordinary editing: mutates buffers, cursors, KB nodes, on-disk files.
    pub const WRITE: PrimitiveTier = PrimitiveTier::Requires(PermissionTier::Write);
    /// Executes a process, or drives something that does.
    pub const SHELL: PrimitiveTier = PrimitiveTier::Requires(PermissionTier::Shell);
    /// Changes the editor's own configuration, code-loading, authorization,
    /// identity material, or reaches the host environment / the network.
    pub const PRIVILEGED: PrimitiveTier = PrimitiveTier::Requires(PermissionTier::Privileged);
    /// Does its own in-Rust authorization check against context the tier
    /// does not carry.
    pub const SELF_ENFORCED: PrimitiveTier = PrimitiveTier::SelfEnforced;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_and_self_enforced_are_distinguishable() {
        // The whole point of the three-way split: "needs nothing" and "polices
        // itself" must not collapse into the same value, or an audit cannot
        // tell a pure string function from a primitive that claims to check a
        // per-KB role internally.
        assert_ne!(PrimitiveTier::Unrestricted, PrimitiveTier::SelfEnforced);
        assert_eq!(PrimitiveTier::Unrestricted.required(), None);
        assert_eq!(PrimitiveTier::SelfEnforced.required(), None);
        assert_ne!(
            PrimitiveTier::Unrestricted.label(),
            PrimitiveTier::SelfEnforced.label()
        );
    }

    #[test]
    fn required_tier_round_trips_for_every_lattice_value() {
        for t in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            assert_eq!(PrimitiveTier::Requires(t).required(), Some(t));
        }
    }

    #[test]
    fn shorthand_constants_agree_with_the_lattice() {
        assert_eq!(tier::PURE, PrimitiveTier::Unrestricted);
        assert_eq!(tier::READ.required(), Some(PermissionTier::ReadOnly));
        assert_eq!(tier::WRITE.required(), Some(PermissionTier::Write));
        assert_eq!(tier::SHELL.required(), Some(PermissionTier::Shell));
        assert_eq!(
            tier::PRIVILEGED.required(),
            Some(PermissionTier::Privileged)
        );
        assert_eq!(tier::SELF_ENFORCED, PrimitiveTier::SelfEnforced);
    }

    #[test]
    fn labels_are_distinct() {
        let labels = [
            tier::PURE.label(),
            tier::READ.label(),
            tier::WRITE.label(),
            tier::SHELL.label(),
            tier::PRIVILEGED.label(),
            tier::SELF_ENFORCED.label(),
        ];
        let mut sorted = labels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels collided: {labels:?}");
    }
}
