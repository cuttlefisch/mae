//! The permission tier — the single vocabulary and the single ordering.
//!
//! # Why this lives in `mae-core` rather than `mae-ai`
//!
//! It was defined in `mae-ai` and re-exported from there, which was fine while
//! only the AI layer needed it. It no longer is: `Editor::set_option` validates
//! `ai_tier`, and `mae-core` cannot name `mae-ai` types — the dependency runs
//! the other way (`crates/ai/Cargo.toml` depends on `mae-core`). The option's
//! set arm therefore grew its OWN spelling list, and the two drifted:
//!
//! * `PermissionTier::parse` accepts `readonly`, `read-only`, `standard`,
//!   `trusted`, `yolo`, … case-insensitively, and its own doc calls itself
//!   *"the single tier vocabulary"*.
//! * `set_option("ai_tier", …)` accepted **only** `ReadOnly`/`Write`/`Shell`/
//!   `Privileged`, exactly. So `:set ai-tier shell` — the spelling config.toml
//!   uses, that `config_name()` emits, and that `mae-agent --permission-mode`
//!   takes — was rejected outright, while `:set ai-tier Shell` was accepted and
//!   then did nothing (issue #640).
//! * `crates/core/src/kb_seed/terminology.rs` meanwhile told users the values
//!   were case-insensitive, describing `parse`'s behaviour rather than the
//!   option's.
//!
//! Moving the type down to the crate both sides can see removes the ability to
//! have two vocabularies, rather than adding a third place to keep in sync
//! (principle #8). `mae-ai` re-exports it, so every existing
//! `mae_ai::PermissionTier` path is unchanged.
//!
//! @ai-caution: [permission] The variant ORDER is a security invariant, not a
//! style choice: `Ord` is derived from declaration order and
//! `PermissionPolicy::decide_tier` compares with `<=`. Reordering these
//! silently changes who may do what.

use serde::{Deserialize, Serialize};

/// Permission tier for AI operations.
///
/// Container-first: standard ops are pre-allowed within the container.
/// Only "escape hatch" operations (host filesystem, external network)
/// require explicit user approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PermissionTier {
    /// Read buffer contents, cursor state, file metadata.
    ReadOnly,
    /// Modify buffers, move cursors, standard editing.
    Write,
    /// Execute shell commands within the container.
    Shell,
    /// Host filesystem, external network, editor config changes.
    Privileged,
}

impl PermissionTier {
    /// The canonical config spelling of this tier — the one
    /// `mae::config::parse_permission_tier` round-trips and the one a denial
    /// message should name, so a user can paste it straight back into
    /// `ai_tier` without guessing.
    ///
    /// The legacy aliases (`standard`/`trusted`/`full`) are still accepted on
    /// input and still used by `ai_permissions`' human-readable help text;
    /// they are deliberately not produced here.
    ///
    /// @ai-caution: [permission] Exhaustive by construction — no `_` arm, so a
    /// new tier variant breaks the build rather than acquiring a name by
    /// default (ADR-084 D3).
    pub fn config_name(self) -> &'static str {
        match self {
            PermissionTier::ReadOnly => "readonly",
            PermissionTier::Write => "write",
            PermissionTier::Shell => "shell",
            PermissionTier::Privileged => "privileged",
        }
    }

    /// Parse a tier spelling, or `None` if it is not recognised.
    ///
    /// **This is the single tier vocabulary.** ADR-090 D4: before it existed,
    /// `mae::config::parse_permission_tier` (lowercase config spellings),
    /// `ai_event_handler`'s wire spellings, and `mae-agent`'s
    /// `PermissionMode::parse` were three separate parsers for the same four
    /// tiers, each with a slightly different alias set. Callers wrap this;
    /// none of them re-implements it.
    ///
    /// `full-auto`/`yolo`/`auto` are accepted as `Privileged` because
    /// `mae-agent`'s `--permission-mode` has always taken them, and under the
    /// three-state model a `Privileged` auto-approval ceiling *is*
    /// "auto-approve everything" — there is nothing above it left to ask
    /// about.
    ///
    /// @ai-caution: [security] Callers MUST treat `None` as an error and
    /// refuse to start (ADR-084 D4). Resolving an unrecognised tier to *any*
    /// real tier — especially via `unwrap_or_default()` — is CWE-636, and it
    /// means a typo silently widens access with nothing to notice it. The
    /// realistic source of an unknown value here is a typo in a local config
    /// written by the same person running the binary, not version skew, so
    /// leniency buys nothing.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "readonly" | "read-only" | "read_only" => Some(PermissionTier::ReadOnly),
            "write" | "standard" => Some(PermissionTier::Write),
            "shell" | "trusted" => Some(PermissionTier::Shell),
            "privileged" | "full" | "full-auto" | "full_auto" | "yolo" | "auto" => {
                Some(PermissionTier::Privileged)
            }
            _ => None,
        }
    }

    /// Position in the privilege ordering, for [`LiveTier`]'s atomic cell.
    ///
    /// @ai-caution: [permission] Must agree with the `Ord` derive — the variant
    /// order IS the privilege order. Exhaustive by construction so a new tier
    /// breaks the build rather than silently taking some ordinal.
    pub fn ordinal(self) -> u8 {
        match self {
            PermissionTier::ReadOnly => 0,
            PermissionTier::Write => 1,
            PermissionTier::Shell => 2,
            PermissionTier::Privileged => 3,
        }
    }

    /// Inverse of [`PermissionTier::ordinal`]. An unknown ordinal resolves to
    /// the most restrictive tier rather than panicking or guessing upward — the
    /// same fail-closed rule `parse` follows for unknown spellings.
    pub fn from_ordinal(n: u8) -> Self {
        match n {
            1 => PermissionTier::Write,
            2 => PermissionTier::Shell,
            3 => PermissionTier::Privileged,
            _ => PermissionTier::ReadOnly,
        }
    }

    /// Every spelling [`PermissionTier::parse`] accepts, for error messages
    /// and validation. Kept next to the parser so the two cannot drift —
    /// asserted by `every_advertised_spelling_parses`.
    pub const VALID_SPELLINGS: &'static [&'static str] = &[
        "readonly",
        "read-only",
        "read_only",
        "write",
        "standard",
        "shell",
        "trusted",
        "privileged",
        "full",
        "full-auto",
        "full_auto",
        "yolo",
        "auto",
    ];
}

/// A permission tier that can be changed while MAE is running, shared by every
/// holder of a clone.
///
/// # Why this exists
///
/// ADR-090 deferred ADR-084 D7 because making `:set ai-tier` take effect needed
/// "a live-mutable policy shared between the main thread and the spawned
/// `AgentSession` task". The session receives the policy **by value**
/// (`AgentSession::with_permission_policy`) and then runs on its own task, so a
/// later change on the main thread could never reach it.
///
/// Putting the mutable part behind an `Arc` inside the policy dissolves that:
/// every clone — including the one the spawned session already owns, and the
/// per-session policies `effective_permission_policy` derives — observes the
/// same cell. No channel, no re-plumbing, no changes at any call site.
///
/// @ai-caution: [permission] This is a security control, so the failure modes
/// are chosen deliberately:
/// * `Relaxed` ordering is sufficient — this is a single value with no other
///   state depending on it, and a tool call racing an in-flight `:set` may
///   legitimately observe either side of it. What must never happen is
///   observing a value that was never set, which `AtomicU8` guarantees.
/// * An unrecognised ordinal resolves to the MOST RESTRICTIVE tier, never a
///   permissive one — the same rule `PermissionTier::parse` follows for
///   unrecognised spellings (CWE-636).
#[derive(Clone, Debug)]
pub struct LiveTier(std::sync::Arc<std::sync::atomic::AtomicU8>);

impl LiveTier {
    pub fn new(tier: PermissionTier) -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            tier.ordinal(),
        )))
    }

    pub fn get(&self) -> PermissionTier {
        PermissionTier::from_ordinal(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn set(&self, tier: PermissionTier) {
        self.0
            .store(tier.ordinal(), std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for LiveTier {
    fn default() -> Self {
        Self::new(PermissionTier::ReadOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drift guard, now covering the crate that owns the type. A sibling
    /// test in `mae-agent` asserts the same property for its own flag; this one
    /// exists so the invariant is enforced where the vocabulary is DEFINED,
    /// not only where one consumer happens to use it.
    #[test]
    fn every_advertised_spelling_parses() {
        for s in PermissionTier::VALID_SPELLINGS {
            assert!(
                PermissionTier::parse(s).is_some(),
                "advertised spelling {s:?} does not parse"
            );
        }
    }

    /// `config_name()` must round-trip: whatever a denial message or a config
    /// generator prints, a user must be able to paste straight back in.
    #[test]
    fn config_name_round_trips_through_parse() {
        for tier in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            assert_eq!(
                PermissionTier::parse(tier.config_name()),
                Some(tier),
                "config_name() for {tier:?} does not parse back to itself"
            );
        }
    }

    /// The ordering is a security invariant (`decide_tier` compares with `<=`),
    /// so it is asserted rather than left to declaration order surviving a
    /// future edit.
    #[test]
    fn tiers_are_ordered_least_to_most_privileged() {
        assert!(PermissionTier::ReadOnly < PermissionTier::Write);
        assert!(PermissionTier::Write < PermissionTier::Shell);
        assert!(PermissionTier::Shell < PermissionTier::Privileged);
    }
}
