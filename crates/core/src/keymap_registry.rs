//! # Module: keymap_registry — data-driven buffer-context → keymap routing
//!
//! Replaces the hardcoded `BufferMode::keymap_name()` match and the org/markdown
//! `if/else` in `Editor::current_keymap_names` with a registry that maps a
//! buffer's *context* (its `BufferKind`, or the focused `Language`) to the
//! **context keymap** that overlays the input-modality keymap in the resolution
//! chain.
//!
//! Why: MAE principle #7 (no hardcoding — Scheme-first, no kernel patch to
//! extend). With this table data-driven, a module can route a new buffer kind or
//! language to a keymap from Scheme (`bind-context-keymap`) — and a "navigation"
//! context for read-only buffers, or a future canvas/kb-graph artifact, is just
//! another registration, not an edit to the kernel `match`.
//!
//! The registry is kernel-seeded with [`KeymapRegistry::kernel_defaults`], which
//! reproduces today's routing EXACTLY, so a bare `mae-core` (no Scheme runtime,
//! as in unit tests) behaves identically. It lives on `Editor` and is re-seeded
//! on `reset_keymaps_to_kernel`, then module registrations re-apply on reload —
//! the same lifecycle as the keymaps themselves.

use std::collections::HashMap;

use crate::buffer::BufferKind;
use crate::syntax::languages::Language;

/// Every `BufferKind`, in declaration order. Single list reused for seeding and
/// selector parsing (keep in sync with the enum — a missing variant just means
/// that kind can't be targeted by `bind-context-keymap`, not a crash).
const ALL_KINDS: [BufferKind; 16] = [
    BufferKind::Text,
    BufferKind::Conversation,
    BufferKind::Preview,
    BufferKind::Messages,
    BufferKind::Kb,
    BufferKind::Shell,
    BufferKind::Debug,
    BufferKind::Dashboard,
    BufferKind::GitStatus,
    BufferKind::Visual,
    BufferKind::FileTree,
    BufferKind::Diff,
    BufferKind::Agenda,
    BufferKind::Demo,
    BufferKind::ShellSelect,
    BufferKind::Modules,
];

/// Maps buffer context (kind / language) to a context keymap name.
///
/// `*_leader` are the parallel **local-leader** routes: the mode-scoped keymap
/// the transient keypad consults FIRST (before the global `leader`) so `SPC m`
/// is a major-mode local leader (org's babel/export in an org buffer, etc.)
/// while `SPC b/f/w/…` still reach the global leader everywhere. A local-leader
/// keymap parents on `leader`, so the keypad chain is `[<local-leader>, leader]`.
#[derive(Debug, Clone, Default)]
pub struct KeymapRegistry {
    by_kind: HashMap<BufferKind, String>,
    by_language: HashMap<Language, String>,
    leader_by_kind: HashMap<BufferKind, String>,
    leader_by_language: HashMap<Language, String>,
}

impl KeymapRegistry {
    /// Kernel baseline — byte-for-byte the routing that used to be hardcoded.
    /// `by_kind` is derived from `BufferMode::keymap_name()` so the legacy table
    /// stays the single source of truth (no drift between the two); the
    /// org/markdown language overlays were inlined in `current_keymap_names` and
    /// move here.
    pub fn kernel_defaults() -> Self {
        use crate::buffer_mode::BufferMode;
        let mut by_kind = HashMap::new();
        for kind in ALL_KINDS {
            if let Some(km) = kind.keymap_name() {
                by_kind.insert(kind, km.to_string());
            }
        }

        let mut by_language = HashMap::new();
        by_language.insert(Language::Org, "org".to_string());
        by_language.insert(Language::Markdown, "markdown".to_string());

        // Local-leader routes — the org/markdown modules create these keymaps
        // (parent `leader`) and bind their `m …` submenu into them. A module
        // opts in by creating `<name>-leader`; `current_keymap_names` only uses
        // it when the keymap actually exists, so a dangling route is harmless.
        let mut leader_by_language = HashMap::new();
        leader_by_language.insert(Language::Org, "org-leader".to_string());
        leader_by_language.insert(Language::Markdown, "markdown-leader".to_string());

        Self {
            by_kind,
            by_language,
            leader_by_kind: HashMap::new(),
            leader_by_language,
        }
    }

    /// The context keymap for a buffer kind, if any.
    pub fn context_for_kind(&self, kind: BufferKind) -> Option<&str> {
        self.by_kind.get(&kind).map(String::as_str)
    }

    /// The context keymap for a focused language, if any.
    pub fn context_for_language(&self, lang: Language) -> Option<&str> {
        self.by_language.get(&lang).map(String::as_str)
    }

    /// The local-leader keymap for a buffer kind, if any (keypad-only).
    pub fn local_leader_for_kind(&self, kind: BufferKind) -> Option<&str> {
        self.leader_by_kind.get(&kind).map(String::as_str)
    }

    /// The local-leader keymap for a focused language, if any (keypad-only).
    pub fn local_leader_for_language(&self, lang: Language) -> Option<&str> {
        self.leader_by_language.get(&lang).map(String::as_str)
    }

    /// Route a buffer kind to a context keymap (Scheme: `bind-context-keymap`).
    pub fn bind_kind(&mut self, kind: BufferKind, keymap: impl Into<String>) {
        self.by_kind.insert(kind, keymap.into());
    }

    /// Route a language to a context keymap.
    pub fn bind_language(&mut self, lang: Language, keymap: impl Into<String>) {
        self.by_language.insert(lang, keymap.into());
    }

    /// Route a buffer kind to a local-leader keymap.
    pub fn bind_kind_leader(&mut self, kind: BufferKind, keymap: impl Into<String>) {
        self.leader_by_kind.insert(kind, keymap.into());
    }

    /// Route a language to a local-leader keymap.
    pub fn bind_language_leader(&mut self, lang: Language, keymap: impl Into<String>) {
        self.leader_by_language.insert(lang, keymap.into());
    }

    /// Apply a `(selector_type, selector_value, keymap)` registration coming
    /// from Scheme. Returns `Err` with a reason if the selector is unknown, so
    /// the caller can warn rather than silently drop it.
    pub fn apply_binding(
        &mut self,
        selector_type: &str,
        selector_value: &str,
        keymap: &str,
    ) -> Result<(), String> {
        match selector_type {
            "kind" => {
                let kind = kind_from_selector(selector_value)
                    .ok_or_else(|| format!("unknown buffer kind '{selector_value}'"))?;
                self.bind_kind(kind, keymap);
                Ok(())
            }
            "language" => {
                let lang = language_from_selector(selector_value)
                    .ok_or_else(|| format!("unknown language '{selector_value}'"))?;
                self.bind_language(lang, keymap);
                Ok(())
            }
            "kind-leader" => {
                let kind = kind_from_selector(selector_value)
                    .ok_or_else(|| format!("unknown buffer kind '{selector_value}'"))?;
                self.bind_kind_leader(kind, keymap);
                Ok(())
            }
            "language-leader" => {
                let lang = language_from_selector(selector_value)
                    .ok_or_else(|| format!("unknown language '{selector_value}'"))?;
                self.bind_language_leader(lang, keymap);
                Ok(())
            }
            other => Err(format!(
                "unknown context selector type '{other}' (expected 'kind', 'language', 'kind-leader', or 'language-leader')"
            )),
        }
    }
}

/// Stable kebab-case selector id for a `BufferKind`, used by the Scheme API
/// (`(bind-context-keymap "kind" "<id>" "<keymap>")`). Distinct from the
/// human-facing `BufferMode::name()` so the API contract is stable.
pub fn kind_selector(kind: BufferKind) -> &'static str {
    match kind {
        BufferKind::Text => "text",
        BufferKind::Conversation => "conversation",
        BufferKind::Preview => "preview",
        BufferKind::Messages => "messages",
        BufferKind::Kb => "kb",
        BufferKind::Shell => "shell",
        BufferKind::Debug => "debug",
        BufferKind::Dashboard => "dashboard",
        BufferKind::GitStatus => "git-status",
        BufferKind::Visual => "visual",
        BufferKind::FileTree => "file-tree",
        BufferKind::Diff => "diff",
        BufferKind::Agenda => "agenda",
        BufferKind::Demo => "demo",
        BufferKind::ShellSelect => "shell-select",
        BufferKind::Modules => "modules",
        BufferKind::Notifications => "notifications",
        BufferKind::KbSharing => "kb-sharing",
    }
}

fn kind_from_selector(s: &str) -> Option<BufferKind> {
    // Reverse of `kind_selector`, over the shared kinds list.
    ALL_KINDS.into_iter().find(|k| kind_selector(*k) == s)
}

fn language_from_selector(s: &str) -> Option<Language> {
    // Language::id() is the canonical id ("org", "markdown", "rust", …).
    [
        Language::Rust,
        Language::Toml,
        Language::Markdown,
        Language::Python,
        Language::JavaScript,
        Language::TypeScript,
        Language::Tsx,
        Language::Go,
        Language::Json,
        Language::Bash,
        Language::Scheme,
        Language::Yaml,
        Language::Org,
    ]
    .into_iter()
    .find(|l| l.id() == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_defaults_reproduce_legacy_routing() {
        let r = KeymapRegistry::kernel_defaults();
        assert_eq!(r.context_for_kind(BufferKind::FileTree), Some("file-tree"));
        assert_eq!(r.context_for_kind(BufferKind::Kb), Some("help"));
        assert_eq!(r.context_for_kind(BufferKind::Modules), Some("modules"));
        assert_eq!(
            r.context_for_kind(BufferKind::GitStatus),
            Some("git-status")
        );
        // Kinds with no overlay fall through (chain uses the modality keymap).
        assert_eq!(r.context_for_kind(BufferKind::Text), None);
        assert_eq!(r.context_for_kind(BufferKind::Dashboard), None);
        assert_eq!(r.context_for_language(Language::Org), Some("org"));
        assert_eq!(r.context_for_language(Language::Markdown), Some("markdown"));
        assert_eq!(r.context_for_language(Language::Rust), None);
    }

    #[test]
    fn kernel_defaults_route_org_markdown_local_leaders() {
        let r = KeymapRegistry::kernel_defaults();
        assert_eq!(
            r.local_leader_for_language(Language::Org),
            Some("org-leader")
        );
        assert_eq!(
            r.local_leader_for_language(Language::Markdown),
            Some("markdown-leader")
        );
        // A language with no local leader falls through (keypad uses plain `leader`).
        assert_eq!(r.local_leader_for_language(Language::Rust), None);
    }

    #[test]
    fn apply_binding_routes_local_leaders() {
        let mut r = KeymapRegistry::kernel_defaults();
        r.apply_binding("language-leader", "rust", "rust-leader")
            .unwrap();
        assert_eq!(
            r.local_leader_for_language(Language::Rust),
            Some("rust-leader")
        );
        r.apply_binding("kind-leader", "git-status", "git-leader")
            .unwrap();
        assert_eq!(
            r.local_leader_for_kind(BufferKind::GitStatus),
            Some("git-leader")
        );
    }

    #[test]
    fn apply_binding_routes_and_validates() {
        let mut r = KeymapRegistry::kernel_defaults();
        // A module routes the dashboard to a "navigation" context.
        r.apply_binding("kind", "dashboard", "navigation").unwrap();
        assert_eq!(
            r.context_for_kind(BufferKind::Dashboard),
            Some("navigation")
        );
        // Language rebinding works too.
        r.apply_binding("language", "org", "my-org").unwrap();
        assert_eq!(r.context_for_language(Language::Org), Some("my-org"));
        // Unknown selectors are rejected, not silently dropped.
        assert!(r.apply_binding("kind", "no-such-kind", "x").is_err());
        assert!(r.apply_binding("bogus", "x", "y").is_err());
    }

    #[test]
    fn kind_selector_roundtrips() {
        for k in [
            BufferKind::Dashboard,
            BufferKind::FileTree,
            BufferKind::GitStatus,
            BufferKind::Modules,
            BufferKind::Kb,
            BufferKind::Visual,
        ] {
            assert_eq!(kind_from_selector(kind_selector(k)), Some(k));
        }
    }
}
