//! ADR-090: a permission check answers with one of **three** states, not a
//! bool.
//!
//! This module is the single decision point (the PDP of ADR-084 D1). Every
//! enforcement point in the tree — MCP dispatch, the embedded agent session,
//! `mae-agent`'s TUI and its `--prompt` mode, the Scheme VM's ambient tier —
//! asks [`super::PermissionPolicy::decide`] and presents the answer; none of
//! them re-derives it.
//!
//! The distinction that motivates the whole ADR:
//!
//! * Exceeding `auto_approve_up_to` is [`Decision::Ask`]. Nothing forbids the
//!   call; it simply has not been auto-approved, so a human gets to say.
//! * [`Decision::Deny`] is reserved for what policy **forbids outright** — a
//!   session-declared ceiling (ADR-051), a category restriction (ADR-085/056),
//!   or a configuration that would not parse (ADR-084 D4). No prompt can
//!   rescue a `Deny`, and no approval may convert one into an `Allow`.
//!
//! @ai-caution: [security] A surface may map `Ask` to a denial when it has no
//! human to ask (`--prompt`, headless, external MCP) — that is ADR-090 D3 and
//! it is correct. What a surface must never do is treat `Ask` as `Allow`. The
//! `Decision` enum has no `Default` and no bool conversion precisely so that
//! "just unwrap it" is not available.

use crate::types::PermissionTier;

/// Where a hard ceiling came from. Only affects the message shown; both
/// sources are equally binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardCeilingSource {
    /// ADR-051: the session declared this ceiling for itself at `initialize`.
    /// A session that asked to be restricted gets restricted — silently
    /// prompting a human to undo the session's own declaration would make the
    /// declaration meaningless.
    SessionDeclared,
    /// ADR-084 D4: a declared value could not be parsed, so the most
    /// restrictive tier applies and stays applied. Prompting past a typo is
    /// how a typo becomes an escalation.
    UnparseableDeclaration,
}

/// A ceiling that produces [`Decision::Deny`] rather than [`Decision::Ask`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardCeiling {
    pub tier: PermissionTier,
    pub source: HardCeilingSource,
}

/// Why a call was forbidden outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// The call exceeds a [`HardCeiling`].
    HardCeiling(HardCeiling),
    /// ADR-085/056: the tool's category is outside this session's allowlist
    /// (or the tool has no classified category and a restriction is active —
    /// fail-closed).
    Category,
}

/// The outcome of a permission check (ADR-090 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// At or below `auto_approve_up_to`, and every hard gate passed.
    Allow,
    /// Above `auto_approve_up_to`, but nothing forbids it. An interactive
    /// surface prompts; a non-interactive one denies *explicitly*
    /// ([`Decision::is_ask`] + [`ask_denied_message`]).
    Ask,
    /// Forbidden by policy. Never promotable, by any surface.
    Deny(DenyReason),
}

impl Decision {
    pub fn is_allow(self) -> bool {
        matches!(self, Decision::Allow)
    }

    pub fn is_ask(self) -> bool {
        matches!(self, Decision::Ask)
    }

    pub fn is_deny(self) -> bool {
        matches!(self, Decision::Deny(_))
    }
}

/// The user-facing message for a [`Decision::Deny`]. One formatting site so
/// the wording cannot drift between the MCP path, the Scheme bridge, and the
/// agent CLI.
pub fn deny_message(tool_name: &str, tier: PermissionTier, reason: DenyReason) -> String {
    match reason {
        DenyReason::Category => {
            format!("Category denied: {tool_name} is not in this session's allowed tool categories")
        }
        DenyReason::HardCeiling(hc) => match hc.source {
            HardCeilingSource::SessionDeclared => format!(
                "Permission denied: {tool_name} requires {tier:?} tier, above this session's \
                 declared ceiling ({:?}). A session's own declared ceiling is binding — it is \
                 not something a prompt can raise.",
                hc.tier
            ),
            HardCeilingSource::UnparseableDeclaration => format!(
                "Permission denied: {tool_name} requires {tier:?} tier, and this session's \
                 permission declaration could not be parsed, so the most restrictive tier \
                 ({:?}) applies. Fix the declared value rather than approving past it.",
                hc.tier
            ),
        },
    }
}

/// The user-facing message when a **non-interactive** surface maps
/// [`Decision::Ask`] to a denial (ADR-090 D3). Distinct from
/// [`deny_message`] on purpose: this one names the ceiling the operator can
/// raise, because unlike a real `Deny` it is a usability limit, not a
/// prohibition.
pub fn ask_denied_message(
    tool_name: &str,
    tier: PermissionTier,
    auto_approve_up_to: PermissionTier,
    surface: &str,
) -> String {
    format!(
        "Permission denied: {tool_name} requires {tier:?} tier, above the auto-approval \
         ceiling ({auto_approve_up_to:?}). There is no human to confirm this on {surface}, \
         so the call is denied rather than queued for approval — raise the ceiling \
         explicitly if this call is expected."
    )
}

/// The prompt line shown to a human resolving a [`Decision::Ask`].
pub fn ask_message(
    tool_name: &str,
    tier: PermissionTier,
    auto_approve_up_to: PermissionTier,
) -> String {
    format!(
        "AI wants to run {tool_name} ({tier:?} tier), above the auto-approval ceiling \
         ({auto_approve_up_to:?})."
    )
}
