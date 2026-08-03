//! Inline tool-call confirm prompt — `mae-agent`'s implementation of
//! ADR-090's `Ask` state.
//!
//! This file used to own a `PermissionMode` enum and a `needs_confirmation`
//! function: a third parallel tier vocabulary, after `mae::config`'s
//! lowercase config spellings and `ai_event_handler`'s wire spellings. ADR-090
//! D4 collapsed all three. What lives here now is **presentation only** — the
//! y/n/always overlay and the key mapping. The decision comes from
//! `mae_ai::PermissionPolicy::decide`, the same PDP the MCP and embedded
//! surfaces ask.
//!
//! @ai-caution: [permission] Do not reintroduce a local ceiling/mode type
//! here. If `mae-agent` needs a new permission concept, it belongs in
//! `mae_ai::tools::decision` so every surface gets it at once.

use mae_ai::{PermissionPolicy, PermissionTier};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Build `mae-agent`'s session policy from a `--permission-mode` string.
///
/// Returns `None` for an unrecognised spelling so the caller can refuse to
/// start (ADR-084 D4) rather than defaulting to something permissive. The
/// spellings themselves come from [`PermissionTier::parse`] — this function
/// adds no aliases of its own.
pub fn policy_for_mode(mode: &str) -> Option<PermissionPolicy> {
    Some(PermissionPolicy {
        auto_approve_up_to: PermissionTier::parse(mode)?,
        // `--permission-mode` is an auto-approval ceiling, not a prohibition:
        // an interactive run prompts above it. A HARD ceiling would make
        // `--permission-mode readonly` mean "refuse everything else", which is
        // not what the flag has ever meant.
        ..PermissionPolicy::default()
    })
}

/// A tool call awaiting the user's y/n/always decision.
#[derive(Debug, Clone)]
pub struct PendingConfirm {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub tier: PermissionTier,
}

/// A decision on a [`PendingConfirm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmChoice {
    Approve,
    ApproveAlwaysThisSession,
    Deny,
}

/// Map a raw key char to a [`ConfirmChoice`], or `None` if it's not one of the
/// recognized keys (y/n/a case-insensitive).
pub fn parse_confirm_key(c: char) -> Option<ConfirmChoice> {
    match c.to_ascii_lowercase() {
        'y' => Some(ConfirmChoice::Approve),
        'a' => Some(ConfirmChoice::ApproveAlwaysThisSession),
        'n' => Some(ConfirmChoice::Deny),
        _ => None,
    }
}

pub fn render_overlay(frame: &mut Frame, area: Rect, pending: &PendingConfirm) {
    let width = area.width.saturating_sub(8).clamp(30, 70);
    let height = 7;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let tier_label = format!("{:?}", pending.tier);
    let args_preview = serde_json::to_string(&pending.arguments).unwrap_or_default();
    let args_preview = if args_preview.len() > width as usize {
        format!("{}…", &args_preview[..(width as usize).saturating_sub(1)])
    } else {
        args_preview
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("Tool call: {} ({tier_label})", pending.tool_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(args_preview),
        Line::from(""),
        Line::from(vec![
            Span::styled("[y]", Style::default().fg(Color::Green)),
            Span::raw(" approve  "),
            Span::styled("[a]", Style::default().fg(Color::Cyan)),
            Span::raw(" always this session  "),
            Span::styled("[n]", Style::default().fg(Color::Red)),
            Span::raw(" deny"),
        ]),
    ];

    let block = Block::default()
        .title(" Action Required ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);
    frame.render_widget(paragraph, popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_ai::Decision;

    /// Full 4-tier x 5-mode matrix, now asserted against the **shared** PDP
    /// rather than a local `needs_confirmation`. The oracle changed with
    /// ADR-090: what used to be "needs confirmation: true/false" is now
    /// "`Ask` vs `Allow`", and — the load-bearing part — *never* `Deny`. A
    /// `--permission-mode` ceiling is an auto-approval line, so exceeding it
    /// must always be askable; a `Deny` here would be the regression that
    /// makes users pass `--permission-mode yolo` and lose the prompt entirely.
    #[test]
    fn decide_matrix_covers_all_tier_mode_combinations_and_never_denies() {
        use PermissionTier::{Privileged, ReadOnly, Shell, Write};

        let modes = [
            ("readonly", ReadOnly),
            ("write", Write),
            ("shell", Shell),
            ("privileged", Privileged),
            ("yolo", Privileged),
        ];
        let tiers = [ReadOnly, Write, Shell, Privileged];

        let mut checked = 0;
        for (mode_str, ceiling) in modes {
            let policy = policy_for_mode(mode_str).expect("mode must parse");
            assert_eq!(policy.auto_approve_up_to, ceiling);
            for tier in tiers {
                let expected = if tier <= ceiling {
                    Decision::Allow
                } else {
                    Decision::Ask
                };
                let got = policy.decide("some_tool", tier);
                assert_eq!(
                    got, expected,
                    "decide({tier:?}) under --permission-mode {mode_str} should be {expected:?}"
                );
                assert!(
                    !got.is_deny(),
                    "a --permission-mode ceiling must never DENY ({tier:?} under {mode_str})"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 20, "must cover all 4 tiers x 5 modes");
    }

    /// The alias set is `PermissionTier::parse`'s, not a local one — including
    /// the `read-only` spelling `mae-agent` accepted but `mae::config` did
    /// not, which is exactly the drift D4 names.
    #[test]
    fn mode_strings_are_the_one_shared_vocabulary() {
        for (spelling, expected) in [
            ("readonly", PermissionTier::ReadOnly),
            ("read-only", PermissionTier::ReadOnly),
            ("write", PermissionTier::Write),
            ("standard", PermissionTier::Write),
            ("shell", PermissionTier::Shell),
            ("trusted", PermissionTier::Shell),
            ("privileged", PermissionTier::Privileged),
            ("full", PermissionTier::Privileged),
            ("yolo", PermissionTier::Privileged),
            ("full-auto", PermissionTier::Privileged),
            ("auto", PermissionTier::Privileged),
        ] {
            assert_eq!(
                policy_for_mode(spelling).map(|p| p.auto_approve_up_to),
                Some(expected),
                "{spelling}"
            );
        }
        assert!(policy_for_mode("nonsense").is_none());
        // ADR-084 D4: an unrecognised value resolves to nothing at all, so the
        // caller must refuse to start. It must NOT quietly become a tier.
        for typo in ["Shel", "read only", "privelaged", "", "  "] {
            assert!(
                policy_for_mode(typo).is_none(),
                "{typo:?} must not resolve to a tier"
            );
        }
    }

    #[test]
    fn every_advertised_spelling_parses() {
        for s in PermissionTier::VALID_SPELLINGS {
            assert!(
                PermissionTier::parse(s).is_some(),
                "{s} is advertised but does not parse"
            );
        }
    }

    #[test]
    fn parse_confirm_key_recognizes_yna_case_insensitive() {
        assert_eq!(parse_confirm_key('y'), Some(ConfirmChoice::Approve));
        assert_eq!(parse_confirm_key('Y'), Some(ConfirmChoice::Approve));
        assert_eq!(
            parse_confirm_key('a'),
            Some(ConfirmChoice::ApproveAlwaysThisSession)
        );
        assert_eq!(parse_confirm_key('n'), Some(ConfirmChoice::Deny));
        assert_eq!(parse_confirm_key('x'), None);
    }
}
