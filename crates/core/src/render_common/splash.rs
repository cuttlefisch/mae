//! Shared splash screen data: ASCII art, logo, quick actions.
//!
//! Backends call [`should_show_splash`] to decide whether to render the
//! fullscreen dashboard at all, and [`resolve_active_splash_art`] to look up
//! which art (custom or built-in) is selected. Each backend then lays out
//! and draws it with its own native primitives (`Line`/`Span` for the TUI,
//! direct Skia calls for the GUI) — the two layout models have genuinely
//! diverged (the GUI supports per-section centering and inline images; the
//! TUI does not), so layout itself is not shared, only the art lookup.

use std::path::{Path, PathBuf};

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
    (
        "SPC a p",
        "AI Agent (or built-in chat, ai_chat_enabled)",
        "ai-prompt",
    ),
    ("SPC h h", "Help", "help"),
    ("SPC h t", "Tutorial", "tutor"),
    ("", "Choose keybindings", "choose-keymap-flavor"),
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

/// Resolve the active splash art by name (a custom registered art wins over
/// a built-in of the same name, falling back to `ALL_ARTS[0]` if the
/// selected name matches neither), returning `(art_text, accent_lines,
/// custom_image_path)`.
///
/// This was previously duplicated identically in both backends'
/// `render_splash` (`crates/renderer/src/splash_render.rs` and
/// `crates/gui/src/splash_render.rs`) — now the one implementation.
///
/// `custom_image_path` is `None` for every built-in art and for any custom
/// art with no image registered; it's a GUI-only feature (inline image
/// rendering) and TUI callers can ignore it.
///
/// @ai-caution: [rendering] Splash art image paths resolve relative to module
/// dir. Relative-to-CWD paths will silently fail. Always use absolute paths.
pub fn resolve_active_splash_art(editor: &Editor) -> (&str, &[usize], Option<&Path>) {
    let selected = editor.splash_art.as_deref().unwrap_or("bat");

    // Custom arts come from modules; built-in arts are compiled-in constants.
    // Look up art: first check custom, then built-in.
    let custom = editor
        .custom_splash_arts
        .iter()
        .find(|a| a.name == selected);
    if let Some(c) = custom {
        (c.art.as_str(), &c.accent_lines, c.image_path.as_deref())
    } else {
        let splash = ALL_ARTS
            .iter()
            .find(|a| a.name == selected)
            .unwrap_or(&ALL_ARTS[0]);
        (splash.art, splash.accent_lines, None)
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

    #[test]
    fn custom_splash_art_used() {
        let mut editor = Editor::default();
        editor.custom_splash_arts.push(CustomSplashArt {
            name: "test-art".to_string(),
            art: "HELLO\nWORLD".to_string(),
            accent_lines: vec![1],
            image_path: None,
        });
        editor.splash_art = Some("test-art".to_string());
        editor.install_dashboard();
        let (art, accent_lines, image_path) = resolve_active_splash_art(&editor);
        assert!(art.contains("HELLO"));
        assert!(art.contains("WORLD"));
        assert_eq!(accent_lines, &[1]);
        assert!(image_path.is_none());
    }

    #[test]
    fn custom_art_wins_over_built_in_of_the_same_name() {
        // A custom-registered art named "bat" must shadow the built-in
        // "bat" art — the "additive-only, contributor override always wins"
        // precedent used elsewhere in MAE (bundled KBs, guidance KB).
        let mut editor = Editor::default();
        editor.custom_splash_arts.push(CustomSplashArt {
            name: "bat".to_string(),
            art: "CUSTOM BAT".to_string(),
            accent_lines: vec![],
            image_path: None,
        });
        editor.splash_art = Some("bat".to_string());
        let (art, _, _) = resolve_active_splash_art(&editor);
        assert_eq!(art, "CUSTOM BAT");
    }

    #[test]
    fn unknown_selection_falls_back_to_first_built_in_art() {
        let editor = Editor {
            splash_art: Some("does-not-exist".to_string()),
            ..Editor::default()
        };
        let (art, _, _) = resolve_active_splash_art(&editor);
        assert_eq!(art, ALL_ARTS[0].art);
    }

    #[test]
    fn custom_art_image_path_is_surfaced() {
        let mut editor = Editor::default();
        editor.custom_splash_arts.push(CustomSplashArt {
            name: "img-art".to_string(),
            art: String::new(),
            accent_lines: vec![],
            image_path: Some(PathBuf::from("logo.svg")),
        });
        editor.splash_art = Some("img-art".to_string());
        let (_, _, image_path) = resolve_active_splash_art(&editor);
        assert_eq!(image_path, Some(Path::new("logo.svg")));
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
