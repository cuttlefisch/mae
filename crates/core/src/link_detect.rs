//! Link detection for clickable URLs and file paths in buffer text.

/// The kind of link — drives navigation behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// External URL (http/https) — opened in browser via xdg-open.
    Url,
    /// Local file path (absolute, relative, or ~/...) — opened in editor.
    FilePath,
    /// Markdown `[label](url)` — stripped to show just label.
    Markdown,
    /// Org `[[target][label]]` — stripped to show just label.
    OrgLink,
}

/// A link embedded in rendered output (after markup stripping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedLink {
    /// Rendered line index in the containing buffer.
    pub line_idx: usize,
    /// Byte offset of the label start in the rendered (stripped) text.
    pub byte_start: usize,
    /// Byte offset of the label end in the rendered text.
    pub byte_end: usize,
    /// The resolved target (URL, file path, or internal reference).
    pub target: String,
    /// The kind of link — drives navigation behavior.
    pub kind: LinkKind,
}

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
/// and org `[[target][label]]` with their rendered labels, and highlighting
/// plain URLs and file paths.
pub fn render_segments(text: &str) -> Vec<TextSegment> {
    // Collect markdown links, org links, and plain URLs/paths.
    let mut md = detect_markdown_links(text);
    let mut org = detect_org_links(text);
    let mut plain = detect_links(text);
    let mut all_links: Vec<LinkSpan> = Vec::new();
    // Markdown/org links take priority over plain URL detection.
    all_links.append(&mut md);
    all_links.append(&mut org);
    all_links.sort_by_key(|s| s.byte_start);
    dedup_overlapping(&mut all_links);
    // Add plain links that don't overlap with markdown/org
    plain.retain(|p| {
        !all_links
            .iter()
            .any(|a| p.byte_start < a.byte_end && p.byte_end > a.byte_start)
    });
    all_links.append(&mut plain);
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

/// Strip markdown `[label](url)` from text, returning the cleaned text
/// and a list of (byte_start, byte_end, target) tuples relative to the cleaned text.
pub fn strip_markdown_links(text: &str) -> (String, Vec<(usize, usize, String)>) {
    let links = detect_markdown_links(text);
    if links.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut result = String::with_capacity(text.len());
    let mut link_positions = Vec::new();
    let mut pos = 0;
    for link in &links {
        // Copy text before this link
        result.push_str(&text[pos..link.byte_start]);
        let label = link.label.as_deref().unwrap_or(&link.target);
        let label_start = result.len();
        result.push_str(label);
        let label_end = result.len();
        link_positions.push((label_start, label_end, link.target.clone()));
        pos = link.byte_end;
    }
    // Copy remaining text
    result.push_str(&text[pos..]);
    (result, link_positions)
}

/// Check if a file path has an image extension.
pub fn is_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".svg")
        || lower.ends_with(".bmp")
        || lower.ends_with(".ico")
}

/// Parse org `#+attr_html: :width XXXpx` or `#+attr_org: :width XXX` directives.
/// Returns the width in pixels if found.
pub fn parse_org_attr_width(line: &str) -> Option<u32> {
    let trimmed = line.trim();
    if !trimmed.starts_with("#+attr_html:")
        && !trimmed.starts_with("#+attr_org:")
        && !trimmed.starts_with("#+ATTR_HTML:")
        && !trimmed.starts_with("#+ATTR_ORG:")
    {
        return None;
    }
    // Look for :width followed by a number (optionally with "px" suffix).
    let lower = trimmed.to_ascii_lowercase();
    let idx = lower.find(":width")?;
    let rest = trimmed[idx + 6..].trim_start();
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

/// Parse markdown image width from `{width=XXX}` attribute or `<!-- width=XXX -->` comment.
/// The `text` should be the line containing or following a markdown image.
pub fn parse_md_image_width(text: &str) -> Option<u32> {
    // Check for {width=XXX} after ![...](...) on the same line.
    if let Some(idx) = text.find("{width=") {
        let rest = &text[idx + 7..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(w) = num_str.parse() {
            return Some(w);
        }
    }
    // Check for <!-- width=XXX --> comment.
    if let Some(idx) = text.find("<!-- width=") {
        let rest = &text[idx + 11..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(w) = num_str.parse() {
            return Some(w);
        }
    }
    None
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

    #[test]
    fn render_segments_relative_file_path() {
        let segs = render_segments("See ./src/file.rs for details");
        assert!(
            segs.iter()
                .any(|s| s.link_target.as_deref() == Some("./src/file.rs")),
            "Expected linked segment for ./src/file.rs, got: {:?}",
            segs
        );
    }

    #[test]
    fn render_segments_home_file_path() {
        let segs = render_segments("Config at ~/config/init.scm here");
        assert!(
            segs.iter()
                .any(|s| s.link_target.as_deref() == Some("~/config/init.scm")),
            "Expected linked segment for ~/config/init.scm, got: {:?}",
            segs
        );
    }

    #[test]
    fn render_segments_plain_url() {
        let segs = render_segments("Visit https://example.com for info");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "Visit ");
        assert_eq!(segs[1].text, "https://example.com");
        assert!(segs[1].link_target.is_some());
        assert_eq!(segs[2].text, " for info");
    }

    // --- strip_markdown_links tests ---

    #[test]
    fn strip_markdown_links_basic() {
        let (clean, links) = strip_markdown_links("[docs](https://docs.rs)");
        assert_eq!(clean, "docs");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], (0, 4, "https://docs.rs".to_string()));
    }

    #[test]
    fn strip_markdown_links_multiple() {
        let (clean, links) = strip_markdown_links("[a](https://a.com) and [b](https://b.com)");
        assert_eq!(clean, "a and b");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].2, "https://a.com");
        assert_eq!(links[1].2, "https://b.com");
    }

    // --- Image detection tests ---

    #[test]
    fn is_image_path_extensions() {
        assert!(is_image_path("photo.png"));
        assert!(is_image_path("photo.PNG"));
        assert!(is_image_path("img.jpg"));
        assert!(is_image_path("img.jpeg"));
        assert!(is_image_path("img.webp"));
        assert!(is_image_path("img.gif"));
        assert!(is_image_path("img.svg"));
        assert!(is_image_path("img.bmp"));
        assert!(!is_image_path("file.txt"));
        assert!(!is_image_path("file.rs"));
        assert!(!is_image_path("file.png.bak"));
    }

    #[test]
    fn parse_org_attr_width_basic() {
        assert_eq!(parse_org_attr_width("#+attr_html: :width 600px"), Some(600));
        assert_eq!(parse_org_attr_width("#+attr_org: :width 400"), Some(400));
        assert_eq!(parse_org_attr_width("#+ATTR_HTML: :width 800px"), Some(800));
        assert_eq!(parse_org_attr_width("no attr here"), None);
        assert_eq!(parse_org_attr_width("#+attr_html: :height 400"), None);
    }

    #[test]
    fn parse_md_image_width_basic() {
        assert_eq!(
            parse_md_image_width("![alt](img.png){width=500}"),
            Some(500)
        );
        assert_eq!(parse_md_image_width("<!-- width=300 -->"), Some(300));
        assert_eq!(parse_md_image_width("![alt](img.png)"), None);
    }

    #[test]
    fn strip_markdown_links_no_links() {
        let (clean, links) = strip_markdown_links("plain text here");
        assert_eq!(clean, "plain text here");
        assert!(links.is_empty());
    }

    #[test]
    fn strip_markdown_links_mixed() {
        let (clean, links) = strip_markdown_links("See [docs](https://docs.rs) for more info");
        assert_eq!(clean, "See docs for more info");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, 4); // "See " = 4 bytes, then "docs" starts
        assert_eq!(links[0].1, 8); // "docs" = 4 bytes
        assert_eq!(links[0].2, "https://docs.rs");
    }
}
