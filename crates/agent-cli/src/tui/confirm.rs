//! Inline tool-call confirm prompt — the "reviewability/interviewing" UX.
//! Genuinely net-new: no human-in-the-loop permission-approval UI exists
//! anywhere in MAE today (confirmed gap in ADR-045/046).

use mae_ai::PermissionTier;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Mirrors MAE's own `readonly|write|shell|privileged` policy strings
/// (`crates/mae/src/config.rs::resolve_permission_policy`) for consistency,
/// plus a Claude-Code-style full-auto override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    ReadOnly,
    Write,
    #[default]
    Shell,
    Privileged,
    /// Auto-approve everything, never prompt. Opt-in, not the default.
    FullAuto,
}

impl PermissionMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "readonly" | "read-only" | "read_only" => Some(Self::ReadOnly),
            "write" | "standard" => Some(Self::Write),
            "shell" | "trusted" => Some(Self::Shell),
            "privileged" | "full" => Some(Self::Privileged),
            "yolo" | "full-auto" | "full_auto" | "auto" => Some(Self::FullAuto),
            _ => None,
        }
    }

    fn ceiling(self) -> Option<PermissionTier> {
        match self {
            Self::ReadOnly => Some(PermissionTier::ReadOnly),
            Self::Write => Some(PermissionTier::Write),
            Self::Shell => Some(PermissionTier::Shell),
            Self::Privileged => Some(PermissionTier::Privileged),
            Self::FullAuto => None, // no ceiling — everything auto-approved
        }
    }
}

/// Does a tool call at `tier` need an interactive confirm under `mode`?
/// `ReadOnly`/`Write` tools are auto-approved by default (matches MAE's own
/// container-first `PermissionPolicy` default); `Shell`/`Privileged` always
/// prompt unless the mode's ceiling already covers them.
pub fn needs_confirmation(tier: PermissionTier, mode: PermissionMode) -> bool {
    match mode.ceiling() {
        None => false, // FullAuto
        Some(ceiling) => tier > ceiling,
    }
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

    /// Full 4-tier x 5-mode `needs_confirmation` matrix, asserted explicitly
    /// so no combination — including the "higher tier vs. a lower-than-Shell
    /// ceiling" diagonal (`Write`x`ReadOnly`, `Privileged`x`ReadOnly`,
    /// `Privileged`x`Write`) that earlier per-tier tests never actually
    /// exercised — can silently regress. Expected value is simply "does
    /// `tier` exceed `mode`'s ceiling", with `FullAuto` having no ceiling.
    #[test]
    fn needs_confirmation_matrix_covers_all_tier_mode_combinations() {
        use PermissionMode::{FullAuto, Privileged, ReadOnly, Shell, Write};
        use PermissionTier::{
            Privileged as PPriv, ReadOnly as PRead, Shell as PShell, Write as PWrite,
        };

        let cases: &[(PermissionTier, PermissionMode, bool)] = &[
            // tier = ReadOnly: never needs confirmation, any mode.
            (PRead, ReadOnly, false),
            (PRead, Write, false),
            (PRead, Shell, false),
            (PRead, Privileged, false),
            (PRead, FullAuto, false),
            // tier = Write: needs confirmation only when the ceiling is
            // below Write (i.e. ReadOnly).
            (PWrite, ReadOnly, true),
            (PWrite, Write, false),
            (PWrite, Shell, false),
            (PWrite, Privileged, false),
            (PWrite, FullAuto, false),
            // tier = Shell: needs confirmation unless the ceiling is Shell
            // or above.
            (PShell, ReadOnly, true),
            (PShell, Write, true),
            (PShell, Shell, false),
            (PShell, Privileged, false),
            (PShell, FullAuto, false),
            // tier = Privileged: needs confirmation unless the ceiling is
            // Privileged (the top tier).
            (PPriv, ReadOnly, true),
            (PPriv, Write, true),
            (PPriv, Shell, true),
            (PPriv, Privileged, false),
            (PPriv, FullAuto, false),
        ];

        assert_eq!(cases.len(), 20, "must cover all 4 tiers x 5 modes");

        for &(tier, mode, expected) in cases {
            assert_eq!(
                needs_confirmation(tier, mode),
                expected,
                "needs_confirmation({tier:?}, {mode:?}) should be {expected}"
            );
        }
    }

    #[test]
    fn parse_mode_strings_match_maes_own_policy_vocabulary() {
        assert_eq!(
            PermissionMode::parse("readonly"),
            Some(PermissionMode::ReadOnly)
        );
        assert_eq!(PermissionMode::parse("write"), Some(PermissionMode::Write));
        assert_eq!(
            PermissionMode::parse("standard"),
            Some(PermissionMode::Write)
        );
        assert_eq!(PermissionMode::parse("shell"), Some(PermissionMode::Shell));
        assert_eq!(
            PermissionMode::parse("trusted"),
            Some(PermissionMode::Shell)
        );
        assert_eq!(
            PermissionMode::parse("privileged"),
            Some(PermissionMode::Privileged)
        );
        assert_eq!(
            PermissionMode::parse("full"),
            Some(PermissionMode::Privileged)
        );
        assert_eq!(
            PermissionMode::parse("yolo"),
            Some(PermissionMode::FullAuto)
        );
        assert_eq!(PermissionMode::parse("nonsense"), None);
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
