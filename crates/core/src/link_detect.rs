//! Link detection for clickable URLs and file paths in buffer text.

/// A detected link span in buffer text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    /// Byte offset of the link start in the source text.
    pub byte_start: usize,
    /// Byte offset of the link end (exclusive) in the source text.
    pub byte_end: usize,
    /// The resolved target (URL or file path).
    pub target: String,
    /// Display label (for rendered markdown/org links, this differs from target).
    pub label: Option<String>,
}

/// Detect URLs and file paths in a line of text.
/// Returns link spans relative to the start of the line.
pub fn detect_links(text: &str) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    detect_urls(text, &mut links);
    detect_file_paths(text, &mut links);
    // Sort by start position and deduplicate overlaps (URLs take priority)
    links.sort_by_key(|s| s.byte_start);
    dedup_overlapping(&mut links);
    links
}

/// Detect `https?://` URLs in text.
fn detect_urls(text: &str, out: &mut Vec<LinkSpan>) {
    let mut search_start = 0;
    while let Some(offset) = text[search_start..].find("http") {
        let start = search_start + offset;
        // Check for https:// or http://
        let rest = &text[start..];
        if !rest.starts_with("https://") && !rest.starts_with("http://") {
            search_start = start + 4;
            continue;
        }
        // Find the end of the URL: stop at whitespace, >, ), ], or end of string
        let end = start
            + rest
                .find(|c: char| c.is_whitespace() || matches!(c, '>' | ')' | ']' | '"' | '\''))
                .unwrap_or(rest.len());
        // Trim trailing punctuation that's likely not part of the URL
        let mut actual_end = end;
        while actual_end > start {
            let last = text.as_bytes()[actual_end - 1];
            if matches!(last, b'.' | b',' | b';' | b':' | b'!' | b'?') {
                actual_end -= 1;
            } else {
                break;
            }
        }
        let url = &text[start..actual_end];
        out.push(LinkSpan {
            byte_start: start,
            byte_end: actual_end,
            target: url.to_string(),
            label: None,
        });
        search_start = actual_end;
    }
}

/// Detect file paths (absolute or `./`-relative) in text.
fn detect_file_paths(text: &str, out: &mut Vec<LinkSpan>) {
    let is_path_char =
        |c: char| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '~' | '@');

    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        // Look for path starts: `/` (absolute) or `./` (relative)
        let is_abs = bytes[i] == b'/' && (i == 0 || bytes[i - 1].is_ascii_whitespace());
        let is_rel = i + 1 < bytes.len()
            && bytes[i] == b'.'
            && bytes[i + 1] == b'/'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace());
        let is_home = bytes[i] == b'~'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'/'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace());

        if is_abs || is_rel || is_home {
            let start = i;
            while i < bytes.len() && is_path_char(bytes[i] as char) {
                i += 1;
            }
            // Check for :line:col suffix (e.g., /path/file.rs:42:5)
            if i < bytes.len() && bytes[i] == b':' {
                let colon_start = i;
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // Optional second :col
                if i < bytes.len() && bytes[i] == b':' {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                // Only include if there were digits after the colon
                if i == colon_start + 1 {
                    i = colon_start; // revert — no digits
                }
            }
            let path = &text[start..i];
            // Must have at least one `/` after the prefix to be a real path
            if path.contains('/') && path.len() > 2 {
                out.push(LinkSpan {
                    byte_start: start,
                    byte_end: i,
                    target: path.to_string(),
                    label: None,
                });
            }
        } else {
            i += 1;
        }
    }
}

/// Parse markdown `[label](url)` links, returning spans with label set.
pub fn detect_markdown_links(text: &str) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // Find matching ]
            if let Some(close_bracket) = text[i + 1..].find(']') {
                let label_end = i + 1 + close_bracket;
                let label = &text[i + 1..label_end];
                // Check for (url) immediately after
                if label_end + 1 < bytes.len() && bytes[label_end + 1] == b'(' {
                    if let Some(close_paren) = text[label_end + 2..].find(')') {
                        let url_end = label_end + 2 + close_paren;
                        let url = &text[label_end + 2..url_end];
                        links.push(LinkSpan {
                            byte_start: i,
                            byte_end: url_end + 1,
                            target: url.to_string(),
                            label: Some(label.to_string()),
                        });
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    links
}

/// Parse org `[[target][label]]` or `[[target]]` links.
pub fn detect_org_links(text: &str) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    let mut search_start = 0;
    while let Some(offset) = text[search_start..].find("[[") {
        let start = search_start + offset;
        let rest = &text[start + 2..];
        // Find the closing ]]
        if let Some(close) = rest.find("]]") {
            let inner = &rest[..close];
            let (target, label) = if let Some(sep) = inner.find("][") {
                (&inner[..sep], Some(inner[sep + 2..].to_string()))
            } else {
                (inner, None)
            };
            links.push(LinkSpan {
                byte_start: start,
                byte_end: start + 2 + close + 2,
                target: target.to_string(),
                label,
            });
            search_start = start + 2 + close + 2;
        } else {
            search_start = start + 2;
        }
    }
    links
}

/// A segment of text that may or may not be a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSegment {
    /// The display text (for links with labels, this is the label).
    pub text: String,
    /// If this segment is a link, the target URL/path.
    pub link_target: Option<String>,
}

/// Split a line of text into segments, replacing markdown `[label](url)`
/// and org `[[target][label]]` with their rendered labels.
/// Plain URLs and file paths are kept as-is (no label replacement).
pub fn render_segments(text: &str) -> Vec<TextSegment> {
    // Collect markdown and org links that have labels
    let mut md = detect_markdown_links(text);
    let mut org = detect_org_links(text);
    let mut all_links: Vec<LinkSpan> = Vec::new();
    all_links.append(&mut md);
    all_links.append(&mut org);
    all_links.sort_by_key(|s| s.byte_start);
    dedup_overlapping(&mut all_links);

    if all_links.is_empty() {
        return vec![TextSegment {
            text: text.to_string(),
            link_target: None,
        }];
    }

    let mut segments = Vec::new();
    let mut pos = 0;
    for link in &all_links {
        if link.byte_start > pos {
            segments.push(TextSegment {
                text: text[pos..link.byte_start].to_string(),
                link_target: None,
            });
        }
        let display = link.label.as_deref().unwrap_or(&link.target);
        segments.push(TextSegment {
            text: display.to_string(),
            link_target: Some(link.target.clone()),
        });
        pos = link.byte_end;
    }
    if pos < text.len() {
        segments.push(TextSegment {
            text: text[pos..].to_string(),
            link_target: None,
        });
    }
    segments
}

fn dedup_overlapping(spans: &mut Vec<LinkSpan>) {
    let mut i = 0;
    while i + 1 < spans.len() {
        if spans[i].byte_end > spans[i + 1].byte_start {
            spans.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_urls_in_text() {
        let links = detect_links("See https://example.com/foo for details.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com/foo");
        assert_eq!(links[0].byte_start, 4);
    }

    #[test]
    fn detect_file_paths_in_text() {
        let links = detect_links("Error at /home/user/src/main.rs:42:5");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "/home/user/src/main.rs:42:5");
    }

    #[test]
    fn detect_relative_paths() {
        let links = detect_links("See ./src/lib.rs for details");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "./src/lib.rs");
    }

    #[test]
    fn detect_home_paths() {
        let links = detect_links("Config at ~/config/init.scm");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "~/config/init.scm");
    }

    #[test]
    fn markdown_link_rendered_as_label() {
        let links = detect_markdown_links("Check [docs](https://docs.rs) now");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://docs.rs");
        assert_eq!(links[0].label.as_deref(), Some("docs"));
        assert_eq!(links[0].byte_start, 6);
        assert_eq!(links[0].byte_end, 29);
    }

    #[test]
    fn org_link_rendered_as_label() {
        let links = detect_org_links("See [[https://docs.rs][docs]] here");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://docs.rs");
        assert_eq!(links[0].label.as_deref(), Some("docs"));
    }

    #[test]
    fn org_link_without_label() {
        let links = detect_org_links("See [[https://docs.rs]] here");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://docs.rs");
        assert_eq!(links[0].label, None);
    }

    #[test]
    fn link_span_covers_label_text() {
        let text = "[Click here](https://example.com)";
        let links = detect_markdown_links(text);
        assert_eq!(links.len(), 1);
        assert_eq!(&text[links[0].byte_start..links[0].byte_end], text);
    }

    #[test]
    fn url_strips_trailing_punctuation() {
        let links = detect_links("Visit https://example.com. Done.");
        assert_eq!(links[0].target, "https://example.com");
    }

    #[test]
    fn multiple_links_in_one_line() {
        let links = detect_links("See https://a.com and /tmp/foo.txt");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "https://a.com");
        assert_eq!(links[1].target, "/tmp/foo.txt");
    }

    #[test]
    fn render_segments_plain_text() {
        let segs = render_segments("hello world");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "hello world");
        assert!(segs[0].link_target.is_none());
    }

    #[test]
    fn render_segments_markdown_link() {
        let segs = render_segments("See [docs](https://docs.rs) for info");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "See ");
        assert_eq!(segs[1].text, "docs");
        assert_eq!(segs[1].link_target.as_deref(), Some("https://docs.rs"));
        assert_eq!(segs[2].text, " for info");
    }

    #[test]
    fn render_segments_org_link() {
        let segs = render_segments("See [[https://docs.rs][docs]] here");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[1].text, "docs");
        assert_eq!(segs[1].link_target.as_deref(), Some("https://docs.rs"));
    }

    #[test]
    fn render_segments_org_link_no_label() {
        let segs = render_segments("See [[https://docs.rs]] here");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[1].text, "https://docs.rs");
        assert_eq!(segs[1].link_target.as_deref(), Some("https://docs.rs"));
    }
}
