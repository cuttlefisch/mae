//! Hook firing: push registered Scheme functions into the eval queue.

use super::Editor;

impl Editor {
    /// Queue all registered functions for `hook_name` into `pending_hook_evals`.
    /// The binary drains this queue and calls the Scheme runtime.
    pub fn fire_hook(&mut self, hook_name: &str) {
        for fn_name in self.hooks.get(hook_name) {
            self.pending_hook_evals
                .push((hook_name.to_string(), fn_name.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Editor;

    #[test]
    fn fire_hook_populates_pending() {
        let mut editor = Editor::new();
        editor.hooks.add("before-save", "my-save-fn");
        editor.hooks.add("before-save", "another-fn");
        editor.fire_hook("before-save");
        assert_eq!(editor.pending_hook_evals.len(), 2);
        assert_eq!(editor.pending_hook_evals[0].0, "before-save");
        assert_eq!(editor.pending_hook_evals[0].1, "my-save-fn");
        assert_eq!(editor.pending_hook_evals[1].1, "another-fn");
    }

    #[test]
    fn fire_hook_no_registrations_is_noop() {
        let mut editor = Editor::new();
        editor.fire_hook("before-save");
        assert!(editor.pending_hook_evals.is_empty());
    }
}
