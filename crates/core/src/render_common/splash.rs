//! Shared splash screen data: ASCII art, logo, quick actions, layout.
//!
//! Backends call [`should_show_splash`] and [`build_splash_lines`] to get
//! pre-laid-out lines, then just draw them with their native draw calls.

use crate::{BufferKind, Editor};

pub const ART_BAT: &str = r#"
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

pub struct SplashArt {
    pub name: &'static str,
    pub art: &'static str,
    pub accent_lines: &'static [usize],
}

pub const ALL_ARTS: &[SplashArt] = &[SplashArt {
    name: "bat",
    art: ART_BAT,
    accent_lines: &[],
}];

pub const MAE_LOGO: &str = r#"
     __  __    _     _____
    |  \/  |  / \   | ____|
    | |\/| | / _ \  |  _|
    | |  | |/ ___ \ | |___
    |_|  |_/_/   \_\|_____|
"#;

pub const QUICK_ACTIONS: &[(&str, &str, &str)] = &[
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

/// Returns the number of quick actions (for splash selection bounds).
pub fn splash_action_count() -> usize {
    QUICK_ACTIONS.len()
}

/// Returns true if the fullscreen splash should be displayed.
///
/// Only shows fullscreen splash when the dashboard is active AND there's a
/// single window. In a split layout, the dashboard renders within its pane
/// via the normal window pipeline instead of obscuring other windows.
pub fn should_show_splash(editor: &Editor) -> bool {
    editor.active_buffer().kind == BufferKind::Dashboard && editor.window_mgr.window_count() == 1
}

/// A pre-laid-out splash line ready for rendering.
pub struct SplashLine {
    pub text: String,
    /// Theme key for the foreground color.
    pub theme_key: &'static str,
    pub is_selected: bool,
}

/// Build all splash lines with centering padding pre-applied.
///
/// Returns `(lines, art_width)` — backends use art_width to compute left padding.
pub fn build_splash_lines(editor: &Editor) -> (Vec<SplashLine>, usize) {
    let selected = editor.splash_art.as_deref().unwrap_or("bat");
    let splash = ALL_ARTS
        .iter()
        .find(|a| a.name == selected)
        .unwrap_or(&ALL_ARTS[0]);

    let mut lines: Vec<SplashLine> = Vec::new();

    // Art.
    let art_lines: Vec<&str> = splash.art.lines().collect();
    let art_width = art_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    for (i, line) in art_lines.iter().enumerate() {
        let key = if splash.accent_lines.contains(&i) {
            "string"
        } else {
            "keyword"
        };
        lines.push(SplashLine {
            text: line.to_string(),
            theme_key: key,
            is_selected: false,
        });
    }

    // Logo.
    let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
    let logo_width = logo_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let logo_pad = art_width.saturating_sub(logo_width) / 2;
    for line in &logo_lines {
        lines.push(SplashLine {
            text: format!("{:>pad$}{}", "", line, pad = logo_pad),
            theme_key: "function",
            is_selected: false,
        });
    }

    // Subtitle.
    let subtitle = "Modern AI Editor -- ai-native lisp machine";
    let sub_pad = art_width.saturating_sub(subtitle.len()) / 2;
    lines.push(SplashLine {
        text: format!("{:>w$}{}", "", subtitle, w = sub_pad),
        theme_key: "comment",
        is_selected: false,
    });
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let ver_pad = art_width.saturating_sub(version.len()) / 2;
    lines.push(SplashLine {
        text: format!("{:>w$}{}", "", version, w = ver_pad),
        theme_key: "comment",
        is_selected: false,
    });
    lines.push(SplashLine {
        text: String::new(),
        theme_key: "comment",
        is_selected: false,
    });

    // Quick actions.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d, _)| format!("{:<10}{}", k, d).len())
        .max()
        .unwrap_or(0);
    let qa_pad = art_width.saturating_sub(qa_width + 2) / 2;
    for (i, &(key, desc, _cmd)) in QUICK_ACTIONS.iter().enumerate() {
        let is_selected = i == editor.splash_selection;
        let prefix = if is_selected { "▸ " } else { "  " };
        lines.push(SplashLine {
            text: format!("{:>pad$}{}{:<10}{}", "", prefix, key, desc, pad = qa_pad),
            theme_key: "type",
            is_selected,
        });
    }
    lines.push(SplashLine {
        text: String::new(),
        theme_key: "comment",
        is_selected: false,
    });

    // Recent files.
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
        lines.push(SplashLine {
            text: format!("{:>w$}{}", "", header, w = header_pad),
            theme_key: "comment",
            is_selected: false,
        });
        for (i, path) in recent.iter().enumerate() {
            let label = format!("  {}  {}", i + 1, truncate_path_simple(path, 50));
            let label_pad = art_width.saturating_sub(label.len()) / 2;
            lines.push(SplashLine {
                text: format!("{:>w$}{}", "", label, w = label_pad),
                theme_key: "type",
                is_selected: false,
            });
        }
        lines.push(SplashLine {
            text: String::new(),
            theme_key: "comment",
            is_selected: false,
        });
    }

    // Dismiss hint.
    let dismiss = "j/k to navigate, Enter to select, any other key to dismiss";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push(SplashLine {
        text: format!("{:>w$}{}", "", dismiss, w = dismiss_pad),
        theme_key: "comment",
        is_selected: false,
    });

    (lines, art_width)
}

fn truncate_path_simple(path: &str, max_len: usize) -> String {
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
    fn splash_shows_for_dashboard() {
        let mut editor = Editor::default();
        editor.install_dashboard();
        assert!(should_show_splash(&editor));
    }

    #[test]
    fn splash_hidden_on_scratch() {
        let mut editor = Editor::default();
        editor.install_dashboard();
        editor.window_mgr.focused_window_mut().buffer_idx = 1;
        assert!(!should_show_splash(&editor));
    }

    #[test]
    fn splash_hidden_in_split_layout() {
        let mut editor = Editor::default();
        editor.install_dashboard();
        // Split the window — dashboard is still focused but shouldn't go fullscreen
        let area = crate::window::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let _ = editor
            .window_mgr
            .split(crate::window::SplitDirection::Vertical, 1, area);
        assert!(
            !should_show_splash(&editor),
            "fullscreen splash should NOT show in a split layout"
        );
    }

    #[test]
    fn splash_action_count_matches() {
        assert_eq!(splash_action_count(), QUICK_ACTIONS.len());
    }
}
