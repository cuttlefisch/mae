//! Splash screen — shown when the scratch buffer is empty and focused.
//! Inspired by Doom Emacs's dashboard: ASCII art logo + quick-action hints.

use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::theme_convert::ts;

// ---------------------------------------------------------------------------
// ASCII art variants
// ---------------------------------------------------------------------------

const ART_CHERRY_BLOSSOM: &str = r#"
           .
        .:;:.
      .:;;;;;:.            .::.
      `;:::::;'          .;:::;.
        `:::;'          .;:::::;.
     .--.`:;'   .---.  .;:::::::;.
    /    \  :  /     \.;:::::::::;.
   ;      :  ;       ;`::::::::;'
   ;      ;  ;       ;  `::::::;
    \    /  : \     /     `:::;'
     `--'   :  `---'        `:'
     .--.   :   .---.
    / _  \  :  /     \
   ; / \  ; : ;       ;
   ;| | |;  : ;       ;
    \\_/ /  :  \     /
     `--'   :   `---'
            :
            :
     __  __    _     _____
    |  \/  |  / \   | ____|
    | |\/| | / _ \  |  _|
    | |  | |/ ___ \ | |___
    |_|  |_/_/   \_\|_____|
"#;

const ART_HAIRBOW: &str = r#"
        *    .  *       .
     .    *         *
   *    .    *   .     *
      .         .
    .  *  .  *    .  *
      *       *
     _\|/_ _\|/_
    (__  __X__  __)
      /|  |  |\
     / |  |  | \
    *  |  |  |  *
       |__|__|
       (    )
        \  /
    .    \/    .
     *       *

     __  __    _     _____
    |  \/  |  / \   | ____|
    | |\/| | / _ \  |  _|
    | |  | |/ ___ \ | |___
    |_|  |_/_/   \_\|_____|
"#;

const ART_BAT: &str = r#"
                   /\                 /\
                  / \'._   (\_/)   _.'/ \
                 /_.''._'--('.')--'_.''._\
                 | \_ / `;=/ " \=;` \ _/ |
                  \/ `\__|`\___/`|__/`  \/
                         \(_)_(_)/
                          " ` " `


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
    ("SPC SPC", "Commands"),
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
    let art = match editor.splash_art.as_deref() {
        Some("hairbow") => ART_HAIRBOW,
        Some("bat") => ART_BAT,
        _ => ART_CHERRY_BLOSSOM,
    };

    let art_style = ts(editor, "keyword");
    let logo_style = ts(editor, "function");
    let key_style = ts(editor, "string");
    let desc_style = ts(editor, "ui.text");
    let subtitle_style = ts(editor, "comment");

    // Build all lines: art + subtitle + blank + quick actions
    let art_lines: Vec<&str> = art.lines().collect();
    let mut lines: Vec<Line> = Vec::new();

    // Separate the MAE logo (last 5 non-empty lines) from the art above it.
    let logo_start = art_lines.len().saturating_sub(6);

    for (i, line) in art_lines.iter().enumerate() {
        let style = if i >= logo_start && line.contains("__") {
            logo_style
        } else {
            art_style
        };
        lines.push(Line::styled(line.to_string(), style));
    }

    lines.push(Line::styled(
        "Modern AI Editor — ai-native lisp machine",
        subtitle_style,
    ));
    lines.push(Line::raw(""));

    for &(key, desc) in QUICK_ACTIONS {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<10}", key), key_style),
            Span::styled(desc, desc_style),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Press any key to dismiss", subtitle_style));

    // Vertical centering
    let total_height = lines.len() as u16;
    let top_pad = area.height.saturating_sub(total_height) / 2;

    // Horizontal centering: find the widest line
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
