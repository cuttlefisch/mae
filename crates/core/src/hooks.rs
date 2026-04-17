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

/// Valid hook names. Requests for unknown hooks are rejected.
pub const HOOK_NAMES: &[&str] = &[
    "before-save",
    "after-save",
    "buffer-open",
    "buffer-close",
    "mode-change",
    "command-pre",
    "command-post",
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

    /// Register a function for a hook. Returns false if the hook name is invalid.
    /// Duplicate registrations are silently ignored (idempotent).
    pub fn add(&mut self, hook_name: &str, fn_name: &str) -> bool {
        if !Self::is_valid(hook_name) {
            return false;
        }
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

    /// Check if a hook name is valid.
    pub fn is_valid(name: &str) -> bool {
        HOOK_NAMES.contains(&name)
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
    fn add_invalid_hook_returns_false() {
        let mut reg = HookRegistry::new();
        assert!(!reg.add("nonexistent-hook", "fn"));
        assert!(reg.get("nonexistent-hook").is_empty());
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
    fn all_hook_names_valid() {
        for name in HOOK_NAMES {
            assert!(HookRegistry::is_valid(name));
        }
        assert!(!HookRegistry::is_valid("bogus"));
    }
}
