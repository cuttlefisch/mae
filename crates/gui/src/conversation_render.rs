//! Conversation (AI chat) buffer rendering for the GUI backend.
//!
//! Conversation buffers are now rendered through the standard FrameLayout pipeline
//! (compute_layout + render_buffer_content) using highlight spans from
//! `Conversation::highlight_spans()`. This eliminates the viewport/wrapping
//! coordinate mismatch that caused cursor desync.

#[cfg(test)]
mod tests {
    use mae_core::conversation::{char_boundary_at, screen_line_count, wrap_text_into_rows};

    #[test]
    fn char_boundary_at_basic() {
        assert_eq!(char_boundary_at("hello", 3), 3);
        assert_eq!(char_boundary_at("hello", 10), 5);
        assert_eq!(char_boundary_at("", 5), 0);
    }

    #[test]
    fn char_boundary_at_multibyte() {
        let s = "héllo"; // é is 2 bytes
        let boundary = char_boundary_at(s, 2);
        assert!(s.is_char_boundary(boundary));
    }

    #[test]
    fn char_boundary_at_cjk() {
        // Each CJK char is 3 bytes, 2 display columns
        let s = "日本語テスト"; // 6 chars, 12 display columns
        let boundary = char_boundary_at(s, 4); // 4 display cols = 2 CJK chars
        assert_eq!(boundary, 6); // 2 chars × 3 bytes
        assert!(s.is_char_boundary(boundary));
    }

    #[test]
    fn screen_line_count_basic() {
        assert_eq!(screen_line_count("hello", 80), 1);
        assert_eq!(screen_line_count("", 80), 1);
        assert_eq!(screen_line_count(&"a".repeat(20), 10), 2);
        assert_eq!(screen_line_count(&"a".repeat(30), 10), 3);
    }

    #[test]
    fn screen_line_count_cjk() {
        // 6 CJK chars = 12 display columns
        assert_eq!(screen_line_count("日本語テスト", 12), 1);
        assert_eq!(screen_line_count("日本語テスト", 6), 2);
        assert_eq!(screen_line_count("日本語テスト", 4), 3);
    }

    #[test]
    fn wrap_text_into_rows_basic() {
        let text = "a".repeat(20);
        let rows = wrap_text_into_rows(&text, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 10);
        assert_eq!(rows[1].len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_exact() {
        let text = "a".repeat(10);
        let rows = wrap_text_into_rows(&text, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_short() {
        let rows = wrap_text_into_rows("hello", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], "hello");
    }
}
