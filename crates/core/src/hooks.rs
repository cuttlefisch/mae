//! Hook registry: named hook points with ordered lists of Scheme function names.
//!
//! Hooks are the primary extensibility mechanism — they let Scheme code react
//! to editor events (save, open, mode change, command dispatch) without the
//! core crate knowing anything about Scheme. The core fires hooks by pushing
//! entries into `Editor::pending_hook_evals`; the binary drains them and calls
//! the Scheme runtime.
//!
//! Emacs lesson: hooks are what make Emacs feel alive. `before-save-hook`,
//! `after-save-hook`, `find-file-hook`, `post-command-hook` — without these,
//! the editor is just a binary. With them, it's a platform.

use std::collections::HashMap;

/// Well-known hook names. These are documented and used by the kernel.
/// The hook namespace is OPEN — modules can register any hook name.
/// This list exists for documentation and `mae pkg doctor` validation only.
pub const WELL_KNOWN_HOOKS: &[&str] = &[
    "before-save",
    "after-save",
    "buffer-open",
    "buffer-close",
    "mode-change",
    "command-pre",
    "command-post",
    "file-changed-on-disk",
    "app-start",
    "app-exit",
    "focus-in",
    "focus-out",
    "option-change",
    "before-revert",
    "after-revert",
    "window-split",
    "window-close",
    "after-load",
    "module-loaded",
    "module-unloaded",
    "after-kb-change",
];

/// A registry of named hooks, each with an ordered list of Scheme function names.
#[derive(Debug, Clone)]
pub struct HookRegistry {
    hooks: HashMap<String, Vec<String>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    pub fn new() -> Self {
        HookRegistry {
            hooks: HashMap::new(),
        }
    }

    /// Register a function for a hook. Always succeeds — the hook namespace is open.
    /// Duplicate registrations are silently ignored (idempotent).
    pub fn add(&mut self, hook_name: &str, fn_name: &str) -> bool {
        let fns = self.hooks.entry(hook_name.to_string()).or_default();
        if !fns.iter().any(|f| f == fn_name) {
            fns.push(fn_name.to_string());
        }
        true
    }

    /// Remove a function from a hook. Returns true if it was found and removed.
    pub fn remove(&mut self, hook_name: &str, fn_name: &str) -> bool {
        if let Some(fns) = self.hooks.get_mut(hook_name) {
            if let Some(pos) = fns.iter().position(|f| f == fn_name) {
                fns.remove(pos);
                return true;
            }
        }
        false
    }

    /// Get the list of functions registered for a hook (empty slice if none).
    pub fn get(&self, hook_name: &str) -> &[String] {
        self.hooks
            .get(hook_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// List all hooks that have at least one registered function.
    pub fn list(&self) -> Vec<(&str, &[String])> {
        self.hooks
            .iter()
            .filter(|(_, fns)| !fns.is_empty())
            .map(|(name, fns)| (name.as_str(), fns.as_slice()))
            .collect()
    }

    /// Return names of all hooks that contain the given function name.
    pub fn hooks_containing(&self, fn_name: &str) -> Vec<&str> {
        self.hooks
            .iter()
            .filter(|(_, fns)| fns.iter().any(|f| f == fn_name))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Check if a hook name is well-known (documented kernel hook).
    /// The hook namespace is open — any name is valid for registration.
    /// This method is for documentation and diagnostics only.
    pub fn is_well_known(name: &str) -> bool {
        if WELL_KNOWN_HOOKS.contains(&name) {
            return true;
        }
        // Check for parameterized form: "base-hook:param"
        if let Some(base) = name.split(':').next() {
            WELL_KNOWN_HOOKS.contains(&base)
        } else {
            false
        }
    }

    /// Check if a hook name is valid. Always returns true — the hook namespace
    /// is open so modules can define custom hooks.
    pub fn is_valid(_name: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_get() {
        let mut reg = HookRegistry::new();
        assert!(reg.add("before-save", "my-fn"));
        assert_eq!(reg.get("before-save"), &["my-fn"]);
    }

    #[test]
    fn add_duplicate_is_idempotent() {
        let mut reg = HookRegistry::new();
        reg.add("before-save", "my-fn");
        reg.add("before-save", "my-fn");
        assert_eq!(reg.get("before-save").len(), 1);
    }

    #[test]
    fn add_any_hook_succeeds() {
        // Hook namespace is open — any hook name is accepted.
        let mut reg = HookRegistry::new();
        assert!(reg.add("custom-module-hook", "fn"));
        assert_eq!(reg.get("custom-module-hook"), &["fn"]);
    }

    #[test]
    fn remove_existing() {
        let mut reg = HookRegistry::new();
        reg.add("after-save", "fn-a");
        reg.add("after-save", "fn-b");
        assert!(reg.remove("after-save", "fn-a"));
        assert_eq!(reg.get("after-save"), &["fn-b"]);
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut reg = HookRegistry::new();
        assert!(!reg.remove("after-save", "fn-a"));
    }

    #[test]
    fn get_empty_hook() {
        let reg = HookRegistry::new();
        assert!(reg.get("before-save").is_empty());
    }

    #[test]
    fn ordering_preserved() {
        let mut reg = HookRegistry::new();
        reg.add("buffer-open", "first");
        reg.add("buffer-open", "second");
        reg.add("buffer-open", "third");
        assert_eq!(reg.get("buffer-open"), &["first", "second", "third"]);
    }

    #[test]
    fn list_only_populated() {
        let mut reg = HookRegistry::new();
        reg.add("before-save", "fn-a");
        reg.add("mode-change", "fn-b");
        let listed = reg.list();
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn hooks_containing_finds_matches() {
        let mut reg = HookRegistry::new();
        reg.add("before-save", "my-fn");
        reg.add("after-save", "my-fn");
        reg.add("buffer-open", "other-fn");
        let mut result = reg.hooks_containing("my-fn");
        result.sort();
        assert_eq!(result, vec!["after-save", "before-save"]);
        assert!(reg.hooks_containing("nonexistent").is_empty());
    }

    #[test]
    fn well_known_hooks_recognized() {
        for name in WELL_KNOWN_HOOKS {
            assert!(HookRegistry::is_well_known(name));
        }
        assert!(!HookRegistry::is_well_known("bogus"));
    }

    #[test]
    fn any_hook_is_valid() {
        // Hook namespace is open.
        assert!(HookRegistry::is_valid("bogus"));
        assert!(HookRegistry::is_valid("my-module-hook"));
    }

    #[test]
    fn parameterized_hook_valid() {
        assert!(HookRegistry::is_valid("buffer-open:rust"));
        assert!(HookRegistry::is_valid("buffer-open:python"));
        assert!(HookRegistry::is_valid("before-save:rust"));
    }

    #[test]
    fn parameterized_hook_any_base_valid() {
        // Hook namespace is open — even unknown base names are valid.
        assert!(HookRegistry::is_valid("nonexistent:rust"));
        // But well_known check still rejects unknown bases.
        assert!(!HookRegistry::is_well_known("nonexistent:rust"));
    }

    #[test]
    fn parameterized_hook_add_and_get() {
        let mut reg = HookRegistry::new();
        assert!(reg.add("buffer-open:rust", "my-rust-fn"));
        assert_eq!(reg.get("buffer-open:rust"), &["my-rust-fn"]);
        // Base hook is separate
        assert!(reg.get("buffer-open").is_empty());
    }

    #[test]
    fn after_load_hook_valid() {
        assert!(HookRegistry::is_valid("after-load"));
        assert!(HookRegistry::is_valid("after-load:init.scm"));
    }
}
