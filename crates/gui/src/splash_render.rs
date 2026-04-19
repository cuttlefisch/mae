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

const QUICK_ACTIONS: &[(&str, &str, &str)] = &[
    ("SPC f f", "Find file", "find-file"),
    ("SPC f d", "File browser", "file-browser"),
    ("SPC f c", "Edit config", "edit-config"),
    ("SPC SPC", "Commands", "command-palette"),
    ("SPC :", "Command line", "command-mode"),
    ("SPC a a", "AI agent", "open-ai-agent"),
    ("SPC a p", "AI prompt", "ai-prompt"),
    ("SPC h h", "Help", "help"),
    ("SPC h t", "Tutorial", "tutor"),
    ("SPC t s", "Set theme", "theme-picker"),
    ("SPC q q", "Quit", "quit"),
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

    // Collect all lines: (text, fg_color, is_selected).
    let mut lines: Vec<(String, skia_safe::Color4f, bool)> = Vec::new();

    // Art.
    let art_lines: Vec<&str> = splash.art.lines().collect();
    let art_width = art_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    for (i, line) in art_lines.iter().enumerate() {
        let fg = if splash.accent_lines.contains(&i) {
            art_accent
        } else {
            art_fg
        };
        lines.push((line.to_string(), fg, false));
    }

    // Logo.
    let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
    let logo_width = logo_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let logo_pad = art_width.saturating_sub(logo_width) / 2;
    for line in &logo_lines {
        let padded = format!("{:>pad$}{}", "", line, pad = logo_pad);
        lines.push((padded, logo_fg, false));
    }

    // Subtitle.
    let subtitle = "Modern AI Editor -- ai-native lisp machine";
    let sub_pad = art_width.saturating_sub(subtitle.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", subtitle, w = sub_pad),
        subtitle_fg,
        false,
    ));
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let ver_pad = art_width.saturating_sub(version.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", version, w = ver_pad),
        subtitle_fg,
        false,
    ));
    lines.push((String::new(), subtitle_fg, false));

    // Quick actions — with selection highlight.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d, _)| format!("{:<10}{}", k, d).len())
        .max()
        .unwrap_or(0);
    let qa_pad = art_width.saturating_sub(qa_width + 2) / 2;
    let sel_bg = theme::ts_bg(editor, "ui.selection");
    for (i, &(key, desc, _cmd)) in QUICK_ACTIONS.iter().enumerate() {
        let is_selected = i == editor.splash_selection;
        let prefix = if is_selected { "▸ " } else { "  " };
        let text = format!("{:>pad$}{}{:<10}{}", "", prefix, key, desc, pad = qa_pad);
        lines.push((text, key_fg, is_selected));
    }
    lines.push((String::new(), subtitle_fg, false));

    // Recent files (up to 5).
    let recent: Vec<&str> = editor
        .recent_files
        .list()
        .iter()
        .take(5)
        .map(|p| p.to_str().unwrap_or("?"))
        .collect();
    if !recent.is_empty() {
        let header = "Recent Files";
        let header_pad = art_width.saturating_sub(header.len()) / 2;
        lines.push((
            format!("{:>w$}{}", "", header, w = header_pad),
            subtitle_fg,
            false,
        ));
        for (i, path) in recent.iter().enumerate() {
            let label = format!("  {}  {}", i + 1, truncate_path(path, 50));
            let label_pad = art_width.saturating_sub(label.len()) / 2;
            lines.push((format!("{:>w$}{}", "", label, w = label_pad), key_fg, false));
        }
        lines.push((String::new(), subtitle_fg, false));
    }

    // Dismiss hint.
    let dismiss = "j/k to navigate, Enter to select, any other key to dismiss";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", dismiss, w = dismiss_pad),
        subtitle_fg,
        false,
    ));

    // Centering.
    let total_height = lines.len();
    let top_pad = area_height.saturating_sub(total_height) / 2;
    let max_width = lines.iter().map(|(l, _, _)| l.len()).max().unwrap_or(0);
    let left_pad = area_width.saturating_sub(max_width) / 2;

    for (i, (text, fg, selected)) in lines.iter().enumerate() {
        let row = area_row + top_pad + i;
        if row >= area_row + area_height {
            break;
        }
        if *selected {
            if let Some(bg) = sel_bg {
                canvas.draw_rect_fill(row, left_pad, text.len(), 1, bg);
            }
            canvas.draw_text_bold(row, left_pad, text, *fg);
        } else {
            canvas.draw_text_at(row, left_pad, text, *fg);
        }
    }
}

fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        path.to_string()
    } else {
        format!("...{}", &path[path.len() - max_len + 3..])
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
