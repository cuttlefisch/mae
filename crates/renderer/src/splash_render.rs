//! Splash screen — shown when the scratch buffer is empty and focused.
//! Inspired by Doom Emacs's dashboard: ASCII art logo + quick-action hints.

use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::theme_convert::ts;

// ---------------------------------------------------------------------------
// ASCII art variants
//
// Design constraints:
//   - ~40-60 chars wide (centered in 80+ col terminals)
//   - ~12-18 lines tall above the MAE logo
//   - Only printable ASCII (no Unicode — must render in any terminal)
//
// Additional art can be added by defining a new const and adding an
// entry to ALL_ARTS. User selects via :set-splash-art or SPC SPC.
// ---------------------------------------------------------------------------

/// Bat — wings spread wide. Inspired by Vivian Aldridge's classic design.
const ART_BAT: &str = r#"
               _-.                       .-_
            _..-'(                       )`-.._
         ./'. '||\.       (\_/)       .//||` .'\.
      ./'.|'.'||||\\|..    )o o(    ..|//||||`.'|.'\.
   ./'..|'.|| |||||\'''''  `"'  ''''''/ ||||| ||.'|..'\.
 ./'.||'.|||| ||||||||||||.     .|||||||||||| |||||.'||.'\.
/'|||'.|||||| ||||||||||||{     }|||||||||||| ||||||.'|||\`\
 '.||| ||||||| |||||||||||{     }||||||||||| |||||||.'|||.'
'.||| |||||||| |/' `\`\||``     ``||/'' `\| ||||||||| |||.'
|/' \./'    `\./        \!|\   /|!/        \./' `   `\./ `\|
V    V        V          }' `V' `{          V        V    V
`    `        `              V              '        '    '
"#;

struct SplashArt {
    name: &'static str,
    art: &'static str,
    /// Line indices (within the art) that should use the accent color.
    accent_lines: &'static [usize],
}

const ALL_ARTS: &[SplashArt] = &[SplashArt {
    name: "bat",
    art: ART_BAT,
    accent_lines: &[],
}];

/// MAE logo appended to all art variants.
const MAE_LOGO: &str = r#"
     __  __    _     _____
    |  \/  |  / \   | ____|
    | |\/| | / _ \  |  _|
    | |  | |/ ___ \ | |___
    |_|  |_/_/   \_\|_____|
"#;

/// Quick-action hints shown below the ASCII art.
const QUICK_ACTIONS: &[(&str, &str)] = &[
    ("SPC f f", "Find file"),
    ("SPC f d", "File browser"),
    ("SPC f c", "Edit config"),
    ("SPC SPC", "Commands"),
    ("SPC :", "Command line"),
    ("SPC a p", "AI prompt"),
    ("SPC h h", "Help"),
    ("SPC t s", "Set theme"),
    ("SPC q q", "Quit"),
];

/// Returns true if the splash should be displayed: the active buffer is the
/// initial empty scratch buffer with no modifications.
pub(crate) fn should_show_splash(editor: &Editor) -> bool {
    let buf = editor.active_buffer();
    buf.kind == mae_core::BufferKind::Text
        && buf.name == "[scratch]"
        && buf.rope().len_chars() == 0
        && !buf.modified
        && editor.buffers.len() == 1
}

/// Render the splash screen centered in the given area.
pub(crate) fn render_splash(frame: &mut Frame, area: Rect, editor: &Editor) {
    let selected = editor.splash_art.as_deref().unwrap_or("bat");
    let splash = ALL_ARTS
        .iter()
        .find(|a| a.name == selected)
        .unwrap_or(&ALL_ARTS[0]);

    let art_primary = ts(editor, "keyword");
    let art_accent = ts(editor, "string");
    let logo_style = ts(editor, "function");
    let key_style = ts(editor, "type");
    let desc_style = ts(editor, "ui.text");
    let subtitle_style = ts(editor, "comment");

    let mut lines: Vec<Line> = Vec::new();

    // Art lines with two-tone coloring.
    let art_lines: Vec<&str> = splash.art.lines().collect();
    let art_width = art_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    for (i, line) in art_lines.iter().enumerate() {
        let style = if splash.accent_lines.contains(&i) {
            art_accent
        } else {
            art_primary
        };
        lines.push(Line::styled(line.to_string(), style));
    }

    // Helper: center a block of text (all lines padded to same width) within art_width.
    let center_block_pad =
        |block_width: usize| -> usize { art_width.saturating_sub(block_width) / 2 };

    // MAE logo — treat as a fixed-width block, then center the block.
    let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
    let logo_width = logo_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let logo_pad = center_block_pad(logo_width);
    for line in &logo_lines {
        let padded = format!(
            "{:>pad$}{:<width$}",
            "",
            line,
            pad = logo_pad,
            width = logo_width
        );
        lines.push(Line::styled(padded, logo_style));
    }

    // Subtitle — single line, center within art_width.
    let subtitle = "Modern AI Editor — ai-native lisp machine";
    let sub_pad = art_width.saturating_sub(subtitle.len()) / 2;
    lines.push(Line::styled(
        format!("{:>width$}{}", "", subtitle, width = sub_pad),
        subtitle_style,
    ));
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let ver_pad = center_block_pad(version.len());
    lines.push(Line::styled(
        format!("{:>w$}{}", "", version, w = ver_pad),
        subtitle_style,
    ));
    lines.push(Line::raw(""));

    // Quick actions — format all to the same fixed width, then center the block.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d)| format!("{:<10}{}", k, d).len())
        .max()
        .unwrap_or(0);
    let qa_pad = center_block_pad(qa_width);
    for &(key, desc) in QUICK_ACTIONS {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(qa_pad)),
            Span::styled(format!("{:<10}", key), key_style),
            Span::styled(
                format!("{:<width$}", desc, width = qa_width.saturating_sub(10)),
                desc_style,
            ),
        ]));
    }
    lines.push(Line::raw(""));

    // Recent files (up to 5).
    let recent: Vec<&std::path::Path> = editor
        .recent_files
        .list()
        .iter()
        .take(5)
        .map(|p| p.as_path())
        .collect();
    if !recent.is_empty() {
        let header = "Recent Files";
        let header_pad = center_block_pad(header.len());
        lines.push(Line::styled(
            format!("{:>w$}{}", "", header, w = header_pad),
            subtitle_style,
        ));
        for (i, path) in recent.iter().enumerate() {
            let display = path.display().to_string();
            let truncated = if display.len() > 50 {
                format!("...{}", &display[display.len() - 47..])
            } else {
                display
            };
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(qa_pad)),
                Span::styled(format!("  {}  ", i + 1), key_style),
                Span::styled(truncated, desc_style),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // Dismiss hint — single line, center within art_width.
    let dismiss = "Press any key to dismiss";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push(Line::styled(
        format!("{:>width$}{}", "", dismiss, width = dismiss_pad),
        subtitle_style,
    ));

    // Vertical + horizontal centering of the whole block.
    let total_height = lines.len() as u16;
    let top_pad = area.height.saturating_sub(total_height) / 2;

    let max_width = lines.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    let left_pad = area.width.saturating_sub(max_width) / 2;

    let centered_area = Rect {
        x: area.x + left_pad,
        y: area.y + top_pad,
        width: area.width.saturating_sub(left_pad),
        height: area.height.saturating_sub(top_pad),
    };

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, centered_area);
}
