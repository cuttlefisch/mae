//! Shared splash screen data: ASCII art, logo, quick actions, layout.
//!
//! Backends call [`should_show_splash`] and [`build_splash_lines`] to get
//! pre-laid-out lines, then just draw them with their native draw calls.

use std::path::PathBuf;

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

/// A custom splash art registered at runtime via `(register-splash-art! ...)`.
#[derive(Debug, Clone)]
pub struct CustomSplashArt {
    pub name: String,
    pub art: String,
    pub accent_lines: Vec<usize>,
    /// Optional image path for GUI rendering (PNG/JPG/SVG).
    /// TUI backends fall back to the ASCII `art` field.
    pub image_path: Option<PathBuf>,
}

/// Return all available splash art names (built-in + custom).
pub fn available_splash_names(editor: &Editor) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = ALL_ARTS
        .iter()
        .map(|a| (a.name.to_string(), "built-in".to_string()))
        .collect();
    for art in &editor.custom_splash_arts {
        let kind = if art.image_path.is_some() {
            "image"
        } else {
            "custom"
        };
        names.push((art.name.clone(), kind.to_string()));
    }
    names
}

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
    ("SPC a a", "AI Agent (terminal)", "open-ai-agent"),
    ("SPC a p", "AI Chat (built-in)", "ai-prompt"),
    ("SPC h h", "Help", "help"),
    ("SPC h t", "Tutorial", "tutor"),
    ("SPC t s", "Set theme", "theme-picker"),
    ("SPC x", "Scratch buffer", "toggle-scratch-buffer"),
    ("SPC C c", "Connect to server", "collab-connect"),
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

    // @ai-caution: [rendering] Splash art image paths resolve relative to module
    // dir. Relative-to-CWD paths will silently fail. Always use absolute paths.
    // Custom arts come from modules; built-in arts are compiled-in constants.
    // Look up art: first check custom, then built-in.
    let custom = editor
        .custom_splash_arts
        .iter()
        .find(|a| a.name == selected);
    let (art_str, accent_lines): (&str, &[usize]) = if let Some(c) = custom {
        (c.art.as_str(), &c.accent_lines)
    } else {
        let splash = ALL_ARTS
            .iter()
            .find(|a| a.name == selected)
            .unwrap_or(&ALL_ARTS[0]);
        (splash.art, splash.accent_lines)
    };

    let mut lines: Vec<SplashLine> = Vec::new();

    // Art (ASCII lines — for image-only arts these may be empty).
    let art_lines: Vec<&str> = art_str.lines().collect();
    let art_text_width = art_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    for (i, line) in art_lines.iter().enumerate() {
        let key = if accent_lines.contains(&i) {
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

    // When art_text_width is 0 (image-only art, TUI can't render images),
    // use a fallback width so text elements center properly.
    let art_width = if art_text_width > 0 {
        art_text_width
    } else {
        58 // fallback for image-only art (approx QA block width)
    };

    // Logo: auto-hide when image art is selected (image IS the logo).
    let has_image = custom.is_some_and(|c| c.image_path.is_some());
    if editor.splash_show_logo && !has_image {
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

    // Dismiss hint.
    let dismiss = "j/k navigate · Enter select";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push(SplashLine {
        text: format!("{:>w$}{}", "", dismiss, w = dismiss_pad),
        theme_key: "comment",
        is_selected: false,
    });

    (lines, art_width)
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

    #[test]
    fn custom_splash_art_used() {
        let mut editor = Editor::default();
        editor.custom_splash_arts.push(CustomSplashArt {
            name: "test-art".to_string(),
            art: "HELLO\nWORLD".to_string(),
            accent_lines: vec![],
            image_path: None,
        });
        editor.splash_art = Some("test-art".to_string());
        editor.install_dashboard();
        let (lines, _width) = build_splash_lines(&editor);
        // First lines should be from our custom art
        assert!(lines.iter().any(|l| l.text.contains("HELLO")));
        assert!(lines.iter().any(|l| l.text.contains("WORLD")));
    }

    #[test]
    fn available_names_includes_custom() {
        let mut editor = Editor::default();
        editor.custom_splash_arts.push(CustomSplashArt {
            name: "my-art".to_string(),
            art: String::new(),
            accent_lines: vec![],
            image_path: None,
        });
        editor.custom_splash_arts.push(CustomSplashArt {
            name: "img-art".to_string(),
            art: String::new(),
            accent_lines: vec![],
            image_path: Some(PathBuf::from("logo.svg")),
        });
        let names = available_splash_names(&editor);
        assert!(names.iter().any(|(n, k)| n == "bat" && k == "built-in"));
        assert!(names.iter().any(|(n, k)| n == "my-art" && k == "custom"));
        assert!(names.iter().any(|(n, k)| n == "img-art" && k == "image"));
    }
}
