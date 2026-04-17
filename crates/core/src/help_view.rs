//! Help-buffer view state: navigation history over the knowledge base.
//!
//! A help buffer is a live window onto a KB node. When the user follows
//! a link, the current node is pushed onto `back_stack` and the new node
//! becomes `current`. `C-o` / `C-i` walk the stack — the same pattern
//! Emacs `*Help*` and browsers use.
//!
//! Rendering pulls the node body from the KB on each frame; `HelpView`
//! stores only pointers, never body text. This keeps the view in sync
//! when KB content is regenerated (e.g. after loading new commands).

/// Cursor position within the help buffer, measured in "interactive link
/// index". `None` means no link is currently focused — `Enter` is a no-op.
pub type LinkIdx = usize;

/// A navigable link embedded in the rendered help text (byte range in the rope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpLinkSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub target: String,
}

/// Navigation state for a help buffer.
#[derive(Debug, Clone)]
pub struct HelpView {
    /// Id of the KB node currently displayed.
    pub current: String,
    /// Previously visited node ids (most recent last). `C-o` pops from here.
    pub back_stack: Vec<String>,
    /// Forward stack populated when `C-o` is used; cleared on any fresh navigation.
    pub forward_stack: Vec<String>,
    /// Scroll offset in lines from the top of the rendered body.
    pub scroll: usize,
    /// Which link is currently focused (0-indexed into the node's link list).
    /// `None` if the node has no links.
    pub focused_link: Option<LinkIdx>,
    /// Link spans in the rendered rope text. Populated by `help_populate_buffer`.
    pub rendered_links: Vec<HelpLinkSpan>,
}

impl HelpView {
    pub fn new(start: impl Into<String>) -> Self {
        HelpView {
            current: start.into(),
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            scroll: 0,
            focused_link: None,
            rendered_links: Vec::new(),
        }
    }

    /// Navigate to a new node, pushing the current one onto the back stack.
    /// Clears the forward stack — standard browser semantics.
    pub fn navigate_to(&mut self, id: impl Into<String>) {
        let id = id.into();
        if id == self.current {
            return;
        }
        let prev = std::mem::replace(&mut self.current, id);
        self.back_stack.push(prev);
        self.forward_stack.clear();
        self.scroll = 0;
        self.focused_link = None;
        self.rendered_links.clear();
    }

    /// Go back one step. Returns false if the back stack is empty.
    pub fn go_back(&mut self) -> bool {
        let Some(prev) = self.back_stack.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.current, prev);
        self.forward_stack.push(current);
        self.scroll = 0;
        self.focused_link = None;
        self.rendered_links.clear();
        true
    }

    /// Go forward one step. Returns false if the forward stack is empty.
    pub fn go_forward(&mut self) -> bool {
        let Some(next) = self.forward_stack.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.current, next);
        self.back_stack.push(current);
        self.scroll = 0;
        self.focused_link = None;
        self.rendered_links.clear();
        true
    }

    pub fn focus_next_link(&mut self, link_count: usize) {
        if link_count == 0 {
            self.focused_link = None;
            return;
        }
        self.focused_link = Some(match self.focused_link {
            None => 0,
            Some(i) => (i + 1) % link_count,
        });
    }

    pub fn focus_prev_link(&mut self, link_count: usize) {
        if link_count == 0 {
            self.focused_link = None;
            return;
        }
        self.focused_link = Some(match self.focused_link {
            None => link_count - 1,
            Some(i) => (i + link_count - 1) % link_count,
        });
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_view_has_empty_stacks() {
        let v = HelpView::new("index");
        assert_eq!(v.current, "index");
        assert!(v.back_stack.is_empty());
        assert!(v.forward_stack.is_empty());
        assert_eq!(v.scroll, 0);
        assert_eq!(v.focused_link, None);
    }

    #[test]
    fn navigate_pushes_back() {
        let mut v = HelpView::new("a");
        v.navigate_to("b");
        assert_eq!(v.current, "b");
        assert_eq!(v.back_stack, vec!["a"]);
    }

    #[test]
    fn navigate_to_same_is_noop() {
        let mut v = HelpView::new("a");
        v.navigate_to("a");
        assert!(v.back_stack.is_empty());
    }

    #[test]
    fn navigate_clears_forward_stack() {
        let mut v = HelpView::new("a");
        v.navigate_to("b");
        v.go_back();
        assert!(!v.forward_stack.is_empty());
        v.navigate_to("c");
        assert!(v.forward_stack.is_empty());
    }

    #[test]
    fn back_and_forward_round_trip() {
        let mut v = HelpView::new("a");
        v.navigate_to("b");
        v.navigate_to("c");
        assert!(v.go_back());
        assert_eq!(v.current, "b");
        assert!(v.go_back());
        assert_eq!(v.current, "a");
        assert!(!v.go_back());
        assert!(v.go_forward());
        assert_eq!(v.current, "b");
        assert!(v.go_forward());
        assert_eq!(v.current, "c");
        assert!(!v.go_forward());
    }

    #[test]
    fn focus_link_wraps() {
        let mut v = HelpView::new("a");
        v.focus_next_link(3);
        assert_eq!(v.focused_link, Some(0));
        v.focus_next_link(3);
        assert_eq!(v.focused_link, Some(1));
        v.focus_next_link(3);
        assert_eq!(v.focused_link, Some(2));
        v.focus_next_link(3);
        assert_eq!(v.focused_link, Some(0));
        v.focus_prev_link(3);
        assert_eq!(v.focused_link, Some(2));
    }

    #[test]
    fn focus_link_with_zero_count_is_none() {
        let mut v = HelpView::new("a");
        v.focus_next_link(0);
        assert_eq!(v.focused_link, None);
    }

    #[test]
    fn scroll_saturates() {
        let mut v = HelpView::new("a");
        v.scroll_up(5);
        assert_eq!(v.scroll, 0);
        v.scroll_down(10);
        assert_eq!(v.scroll, 10);
        v.scroll_up(3);
        assert_eq!(v.scroll, 7);
    }

    #[test]
    fn navigation_resets_scroll_and_focus() {
        let mut v = HelpView::new("a");
        v.scroll_down(5);
        v.focus_next_link(3);
        v.navigate_to("b");
        assert_eq!(v.scroll, 0);
        assert_eq!(v.focused_link, None);
    }
}
