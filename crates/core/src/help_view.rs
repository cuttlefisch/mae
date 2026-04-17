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

    /// Focus the next link at or after `cursor_byte`. If already focused
    /// on a link, advance to the one after it. Wraps around.
    pub fn focus_next_link(&mut self, cursor_byte: usize) {
        if self.rendered_links.is_empty() {
            self.focused_link = None;
            return;
        }
        // If we already have a focused link, just advance from it.
        if let Some(cur) = self.focused_link {
            self.focused_link = Some((cur + 1) % self.rendered_links.len());
            return;
        }
        // Find the first link whose start is >= cursor_byte.
        let next = self
            .rendered_links
            .iter()
            .position(|l| l.byte_start >= cursor_byte);
        self.focused_link = Some(next.unwrap_or(0));
    }

    /// Focus the previous link before `cursor_byte`. If already focused
    /// on a link, move to the one before it. Wraps around.
    pub fn focus_prev_link(&mut self, cursor_byte: usize) {
        let count = self.rendered_links.len();
        if count == 0 {
            self.focused_link = None;
            return;
        }
        // If we already have a focused link, just go back from it.
        if let Some(cur) = self.focused_link {
            self.focused_link = Some((cur + count - 1) % count);
            return;
        }
        // Find the last link whose start is < cursor_byte.
        let prev = self
            .rendered_links
            .iter()
            .rposition(|l| l.byte_start < cursor_byte);
        self.focused_link = Some(prev.unwrap_or(count - 1));
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
        v.rendered_links = vec![
            HelpLinkSpan {
                byte_start: 10,
                byte_end: 20,
                target: "a".into(),
            },
            HelpLinkSpan {
                byte_start: 30,
                byte_end: 40,
                target: "b".into(),
            },
            HelpLinkSpan {
                byte_start: 50,
                byte_end: 60,
                target: "c".into(),
            },
        ];
        // First Tab from cursor at byte 0 → finds link at byte 10 (index 0)
        v.focus_next_link(0);
        assert_eq!(v.focused_link, Some(0));
        // Subsequent Tabs advance sequentially
        v.focus_next_link(0);
        assert_eq!(v.focused_link, Some(1));
        v.focus_next_link(0);
        assert_eq!(v.focused_link, Some(2));
        // Wraps around
        v.focus_next_link(0);
        assert_eq!(v.focused_link, Some(0));
        // S-Tab goes back
        v.focus_prev_link(0);
        assert_eq!(v.focused_link, Some(2));
    }

    #[test]
    fn focus_link_cursor_aware() {
        let mut v = HelpView::new("a");
        v.rendered_links = vec![
            HelpLinkSpan {
                byte_start: 10,
                byte_end: 20,
                target: "a".into(),
            },
            HelpLinkSpan {
                byte_start: 100,
                byte_end: 110,
                target: "b".into(),
            },
            HelpLinkSpan {
                byte_start: 200,
                byte_end: 210,
                target: "c".into(),
            },
        ];
        // Tab from cursor at byte 50 → should find link at byte 100 (index 1)
        v.focus_next_link(50);
        assert_eq!(v.focused_link, Some(1));
        // S-Tab from cursor at byte 150 → should find link before byte 150 (index 1)
        v.focused_link = None;
        v.focus_prev_link(150);
        assert_eq!(v.focused_link, Some(1));
    }

    #[test]
    fn focus_link_with_no_links_is_none() {
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
        v.rendered_links = vec![HelpLinkSpan {
            byte_start: 10,
            byte_end: 20,
            target: "x".into(),
        }];
        v.scroll_down(5);
        v.focus_next_link(0);
        assert!(v.focused_link.is_some());
        v.navigate_to("b");
        assert_eq!(v.scroll, 0);
        assert_eq!(v.focused_link, None);
    }
}
