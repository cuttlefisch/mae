//! Splash screen rendering for the GUI backend.

use mae_core::Editor;

use crate::canvas::SkiaCanvas;
use crate::theme;

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
    accent_lines: &'static [usize],
}

const ALL_ARTS: &[SplashArt] = &[SplashArt {
    name: "bat",
    art: ART_BAT,
    accent_lines: &[],
}];

const MAE_LOGO: &str = r#"
     __  __    _     _____
    |  \/  |  / \   | ____|
    | |\/| | / _ \  |  _|
    | |  | |/ ___ \ | |___
    |_|  |_/_/   \_\|_____|
"#;

const QUICK_ACTIONS: &[(&str, &str)] = &[
    ("SPC f f", "Find file"),
    ("SPC f d", "File browser"),
    ("SPC SPC", "Commands"),
    ("SPC :", "Command line"),
    ("SPC a p", "AI prompt"),
    ("SPC h h", "Help"),
    ("SPC t s", "Set theme"),
    ("SPC q q", "Quit"),
];

/// Returns true if the splash should be displayed.
pub fn should_show_splash(editor: &Editor) -> bool {
    let buf = editor.active_buffer();
    buf.kind == mae_core::BufferKind::Text
        && buf.name == "[scratch]"
        && buf.rope().len_chars() == 0
        && !buf.modified
        && editor.buffers.len() == 1
}

/// Render the splash screen centered in the available area.
pub fn render_splash(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_row: usize,
    _area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let selected = editor.splash_art.as_deref().unwrap_or("bat");
    let splash = ALL_ARTS
        .iter()
        .find(|a| a.name == selected)
        .unwrap_or(&ALL_ARTS[0]);

    let art_fg = theme::ts_fg(editor, "keyword");
    let art_accent = theme::ts_fg(editor, "string");
    let logo_fg = theme::ts_fg(editor, "function");
    let key_fg = theme::ts_fg(editor, "type");
    let _desc_fg = theme::ts_fg(editor, "ui.text");
    let subtitle_fg = theme::ts_fg(editor, "comment");

    // Collect all lines.
    let mut lines: Vec<(String, skia_safe::Color4f)> = Vec::new();

    // Art.
    let art_lines: Vec<&str> = splash.art.lines().collect();
    let art_width = art_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    for (i, line) in art_lines.iter().enumerate() {
        let fg = if splash.accent_lines.contains(&i) {
            art_accent
        } else {
            art_fg
        };
        lines.push((line.to_string(), fg));
    }

    // Logo.
    let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
    let logo_width = logo_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let logo_pad = art_width.saturating_sub(logo_width) / 2;
    for line in &logo_lines {
        let padded = format!("{:>pad$}{}", "", line, pad = logo_pad);
        lines.push((padded, logo_fg));
    }

    // Subtitle.
    let subtitle = "Modern AI Editor -- ai-native lisp machine";
    let sub_pad = art_width.saturating_sub(subtitle.len()) / 2;
    lines.push((format!("{:>w$}{}", "", subtitle, w = sub_pad), subtitle_fg));
    lines.push((String::new(), subtitle_fg));

    // Quick actions.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d)| format!("{:<10}{}", k, d).len())
        .max()
        .unwrap_or(0);
    let qa_pad = art_width.saturating_sub(qa_width) / 2;
    for &(key, desc) in QUICK_ACTIONS {
        let text = format!("{:>pad$}{:<10}{}", "", key, desc, pad = qa_pad);
        // We'll render key and desc with different colors cell-by-cell.
        // For simplicity, use key_fg for the whole line (good enough for M3).
        lines.push((text, key_fg));
    }
    lines.push((String::new(), subtitle_fg));

    // Dismiss hint.
    let dismiss = "Press any key to dismiss";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", dismiss, w = dismiss_pad),
        subtitle_fg,
    ));

    // Centering.
    let total_height = lines.len();
    let top_pad = area_height.saturating_sub(total_height) / 2;
    let max_width = lines.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let left_pad = area_width.saturating_sub(max_width) / 2;

    for (i, (text, fg)) in lines.iter().enumerate() {
        let row = area_row + top_pad + i;
        if row >= area_row + area_height {
            break;
        }
        canvas.draw_text_at(row, left_pad, text, *fg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_shows_for_empty_scratch() {
        let editor = Editor::default();
        assert!(should_show_splash(&editor));
    }

    #[test]
    fn splash_hidden_when_modified() {
        let mut editor = Editor::default();
        editor.buffers[0].modified = true;
        assert!(!should_show_splash(&editor));
    }

    #[test]
    fn splash_hidden_when_multiple_buffers() {
        let mut editor = Editor::default();
        // Create a second buffer to test multi-buffer condition.
        let buf = mae_core::Buffer::new();
        editor.buffers.push(buf);
        assert!(!should_show_splash(&editor));
    }
}
