//! Display regions: buffer text ranges with display overrides.
//!
//! Emacs text-property `invisible` + `display` equivalent. A `DisplayRegion`
//! replaces a byte range of buffer text with alternative display text (or
//! hides it entirely). Used for link concealment (`link_descriptive`), and
//! extensible to folds, emphasis marker hiding, and virtual text.
//!
//! Regions are per-buffer, sorted by `byte_start`, and rebuilt when buffer
//! generation changes (same invalidation pattern as tree-sitter spans).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::link_detect::is_image_path;
use crate::link_detect::{detect_markdown_links, detect_org_links};

/// Parsed image attributes from `#+attr_*` directives or markdown equivalents.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageAttrs {
    /// Resolved absolute path to the image file.
    pub path: PathBuf,
    /// Explicit width from `#+attr_html/:width` or `{width=N}`. None = fit to text area.
    pub width: Option<u32>,
    /// Natural pixel dimensions, cached at region creation time.
    /// `(0, 0)` if the image could not be read.
    pub natural_width: u32,
    pub natural_height: u32,
}

/// A region of buffer text with a display override.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayRegion {
    /// Byte offset of the region start in the rope.
    pub byte_start: usize,
    /// Byte offset of the region end (exclusive) in the rope.
    pub byte_end: usize,
    /// Replacement text. `None` = hide entirely (Emacs `invisible`).
    pub replacement: Option<String>,
    /// Link target for clickable regions (gx navigation).
    pub link_target: Option<String>,
    /// Image attrs for inline display (GUI renders image, TUI renders placeholder).
    pub image: Option<ImageAttrs>,
}

/// Compute display regions for link concealment in a buffer.
///
/// Detects markdown `[label](url)` and org `[[target][label]]` links and
/// creates regions that replace the full syntax with just the label text.
///
/// `extension`: file extension (e.g. "org", "md") to select link types.
/// Pass `None` to detect both.
pub fn compute_link_regions(
    text: &str,
    link_descriptive: bool,
    extension: Option<&str>,
) -> Vec<DisplayRegion> {
    if !link_descriptive {
        return Vec::new();
    }

    let detect_md = !matches!(extension, Some("org"));
    let detect_org = !matches!(extension, Some("md"));

    let mut regions = Vec::new();

    if detect_md {
        for link in detect_markdown_links(text) {
            let label = link.label.as_deref().unwrap_or(&link.target);
            regions.push(DisplayRegion {
                byte_start: link.byte_start,
                byte_end: link.byte_end,
                replacement: Some(label.to_string()),
                link_target: Some(link.target.clone()),
                image: None,
            });
        }
    }

    if detect_org {
        for link in detect_org_links(text) {
            let label = link.label.as_deref().unwrap_or(&link.target);
            regions.push(DisplayRegion {
                byte_start: link.byte_start,
                byte_end: link.byte_end,
                replacement: Some(label.to_string()),
                link_target: Some(link.target.clone()),
                image: None,
            });
        }
    }

    // Sort by byte_start (links from different detectors may interleave).
    regions.sort_by_key(|r| r.byte_start);

    // Deduplicate overlapping regions (keep first).
    let mut i = 0;
    while i + 1 < regions.len() {
        if regions[i].byte_end > regions[i + 1].byte_start {
            regions.remove(i + 1);
        } else {
            i += 1;
        }
    }

    regions
}

/// Compute display regions for image links in a buffer.
///
/// Detects markdown `![alt](path)` and org `[[path]]` links where the target is
/// an image file. Each image link gets a `DisplayRegion` with `image: Some(...)`.
///
/// `base_dir`: parent directory of the buffer's file, used to resolve relative paths.
/// Pass `None` for unsaved buffers (image regions are skipped).
///
/// `collapsed_images`: set of line indices where images are individually collapsed.
pub fn compute_image_regions(
    text: &str,
    extension: Option<&str>,
    base_dir: Option<&Path>,
    collapsed_images: &std::collections::HashSet<usize>,
) -> Vec<DisplayRegion> {
    let base_dir = match base_dir {
        Some(d) => d,
        None => return Vec::new(),
    };

    let detect_md = !matches!(extension, Some("org"));
    let detect_org = !matches!(extension, Some("md"));

    let mut regions = Vec::new();

    // Helper to compute the line number for a byte offset.
    let line_of_byte = |byte_offset: usize| -> usize {
        text[..byte_offset].chars().filter(|&c| c == '\n').count()
    };

    // Helper to get the line text for the line before a byte offset.
    let prev_line_text = |byte_offset: usize| -> Option<&str> {
        let line_num = line_of_byte(byte_offset);
        if line_num == 0 {
            return None;
        }
        text.split('\n').nth(line_num - 1)
    };

    if detect_md {
        // Markdown images: ![alt](path) — note the `!` prefix distinguishes from links.
        let mut i = 0;
        let bytes = text.as_bytes();
        while i < bytes.len() {
            if bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                // Find matching ]
                if let Some(close_bracket) = text[i + 2..].find(']') {
                    let label_end = i + 2 + close_bracket;
                    if label_end + 1 < bytes.len() && bytes[label_end + 1] == b'(' {
                        if let Some(close_paren) = text[label_end + 2..].find(')') {
                            let url_end = label_end + 2 + close_paren;
                            let path_str = &text[label_end + 2..url_end];
                            if is_image_path(path_str) {
                                let line_num = line_of_byte(i);
                                if !collapsed_images.contains(&line_num) {
                                    let resolved = resolve_image_path(path_str, base_dir);
                                    let width = {
                                        // Check same line for {width=N} after the image.
                                        let rest = &text[url_end + 1..];
                                        let same_line = rest.split('\n').next().unwrap_or("");
                                        crate::link_detect::parse_md_image_width(same_line).or_else(
                                            || {
                                                prev_line_text(i).and_then(
                                                    crate::link_detect::parse_md_image_width,
                                                )
                                            },
                                        )
                                    };
                                    let filename = Path::new(path_str)
                                        .file_name()
                                        .map(|f| f.to_string_lossy().to_string())
                                        .unwrap_or_else(|| path_str.to_string());
                                    regions.push(DisplayRegion {
                                        byte_start: i,
                                        byte_end: url_end + 1,
                                        replacement: Some(format!("[Image: {}]", filename)),
                                        link_target: Some(path_str.to_string()),
                                        image: resolved.map(|p| {
                                            let (nw, nh) = crate::image_meta::read_image_meta(&p)
                                                .map(|m| (m.width, m.height))
                                                .unwrap_or((0, 0));
                                            ImageAttrs {
                                                path: p,
                                                width,
                                                natural_width: nw,
                                                natural_height: nh,
                                            }
                                        }),
                                    });
                                }
                            }
                            i = url_end + 1;
                            continue;
                        }
                    }
                }
            }
            i += 1;
        }
    }

    if detect_org {
        for link in detect_org_links(text) {
            if is_image_path(&link.target) {
                let line_num = line_of_byte(link.byte_start);
                if !collapsed_images.contains(&line_num) {
                    let resolved = resolve_image_path(&link.target, base_dir);
                    let width = prev_line_text(link.byte_start)
                        .and_then(crate::link_detect::parse_org_attr_width);
                    let filename = Path::new(&link.target)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| link.target.clone());
                    regions.push(DisplayRegion {
                        byte_start: link.byte_start,
                        byte_end: link.byte_end,
                        replacement: Some(format!("[Image: {}]", filename)),
                        link_target: Some(link.target.clone()),
                        image: resolved.map(|p| {
                            let (nw, nh) = crate::image_meta::read_image_meta(&p)
                                .map(|m| (m.width, m.height))
                                .unwrap_or((0, 0));
                            ImageAttrs {
                                path: p,
                                width,
                                natural_width: nw,
                                natural_height: nh,
                            }
                        }),
                    });
                }
            }
        }
    }

    regions.sort_by_key(|r| r.byte_start);
    regions
}

/// Resolve an image path relative to a base directory.
/// Returns `None` if the resolved path doesn't exist.
fn resolve_image_path(path_str: &str, base_dir: &Path) -> Option<PathBuf> {
    let path = Path::new(path_str);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else if path_str.starts_with("~/") {
        dirs_next_path(path_str)
    } else {
        // Strip file: or file:// prefix if present (org-mode convention).
        let clean = path_str
            .strip_prefix("file://")
            .or_else(|| path_str.strip_prefix("file:"))
            .unwrap_or(path_str);
        base_dir.join(clean)
    };
    if resolved.exists() {
        Some(resolved)
    } else {
        None
    }
}

/// Expand ~/... to home directory.
fn dirs_next_path(path_str: &str) -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(&path_str[2..])
    } else {
        PathBuf::from(path_str)
    }
}

/// Apply display regions to a line's characters.
///
/// Given the chars of a rope line, the line's byte offset in the rope, and
/// the buffer's display regions, returns:
/// - `display_chars`: the characters to render on screen
/// - `rope_col_map`: for each display char index, the corresponding rope
///   char index (used for cursor positioning and click mapping)
///
/// Characters inside a region are replaced with the region's replacement
/// text. The rope_col_map maps replacement chars back to the region's
/// start position in the rope.
pub fn apply_display_regions_to_line(
    line_chars: &[char],
    line_byte_start: usize,
    line_byte_end: usize,
    regions: &[DisplayRegion],
) -> (Vec<char>, Vec<usize>) {
    // Fast path: use binary search to find overlapping regions.
    // Regions are sorted by byte_start.
    let start_idx = regions.partition_point(|r| r.byte_end <= line_byte_start);
    let overlapping: Vec<&DisplayRegion> = regions[start_idx..]
        .iter()
        .take_while(|r| r.byte_start < line_byte_end)
        .collect();

    if overlapping.is_empty() {
        let map: Vec<usize> = (0..line_chars.len()).collect();
        return (line_chars.to_vec(), map);
    }

    // Build a byte-to-char map for the line so we can convert region byte
    // offsets to char offsets within the line.
    let line_str: String = line_chars.iter().collect();
    let byte_positions: Vec<usize> = line_str
        .char_indices()
        .map(|(byte_idx, _)| byte_idx)
        .collect();
    let line_byte_len = line_str.len();

    // Convert byte offset (relative to rope) to char index (relative to line).
    let byte_to_char_idx = |rope_byte: usize| -> usize {
        let line_relative = rope_byte.saturating_sub(line_byte_start);
        let clamped = line_relative.min(line_byte_len);
        byte_positions
            .iter()
            .position(|&b| b >= clamped)
            .unwrap_or(line_chars.len())
    };

    let mut display_chars = Vec::new();
    let mut rope_col_map = Vec::new();
    let mut char_pos = 0; // current position in line_chars

    for region in &overlapping {
        let region_char_start = byte_to_char_idx(region.byte_start);
        let region_char_end = byte_to_char_idx(region.byte_end);

        // Emit chars before this region.
        while char_pos < region_char_start && char_pos < line_chars.len() {
            display_chars.push(line_chars[char_pos]);
            rope_col_map.push(char_pos);
            char_pos += 1;
        }

        // Emit replacement text (if any), mapping back to region start.
        if let Some(ref replacement) = region.replacement {
            for ch in replacement.chars() {
                display_chars.push(ch);
                rope_col_map.push(region_char_start);
            }
        }
        // else: invisible (hide entirely) — emit nothing.

        // Skip over the original chars covered by the region.
        char_pos = region_char_end.min(line_chars.len());
    }

    // Emit remaining chars after the last region.
    while char_pos < line_chars.len() {
        display_chars.push(line_chars[char_pos]);
        rope_col_map.push(char_pos);
        char_pos += 1;
    }

    (display_chars, rope_col_map)
}

/// Map a rope char column to a display char column.
///
/// Given the `rope_col_map` from `apply_display_regions_to_line`, find
/// the display column that corresponds to a rope column. If the rope
/// column falls inside a hidden region, snaps to the nearest visible edge.
pub fn rope_col_to_display_col(rope_col: usize, rope_col_map: &[usize]) -> usize {
    // Find the first display col that maps to rope_col or later.
    for (display_col, &mapped_rope_col) in rope_col_map.iter().enumerate() {
        if mapped_rope_col >= rope_col {
            return display_col;
        }
    }
    rope_col_map.len()
}

/// Map a display char column to a rope char column.
pub fn display_col_to_rope_col(display_col: usize, rope_col_map: &[usize]) -> usize {
    rope_col_map
        .get(display_col)
        .copied()
        .unwrap_or_else(|| rope_col_map.last().map(|&c| c + 1).unwrap_or(0))
}

/// Find the display region containing `cursor_byte`, if any.
/// Returns the index into the regions vec.
pub fn region_at_byte(regions: &[DisplayRegion], cursor_byte: usize) -> Option<usize> {
    regions
        .iter()
        .position(|r| cursor_byte >= r.byte_start && cursor_byte < r.byte_end)
}

/// Return effective display regions with the revealed region removed.
/// `reveal_cursor_byte`: cursor byte offset for org-appear reveal.
/// If the cursor is inside a region, that region is suppressed (raw text visible).
/// Returns `Cow::Borrowed` on the fast path (no reveal or cursor outside regions),
/// avoiding a full clone every frame.
pub fn regions_with_cursor_reveal<'a>(
    regions: &'a [DisplayRegion],
    reveal_cursor_byte: Option<usize>,
) -> Cow<'a, [DisplayRegion]> {
    let Some(cursor_byte) = reveal_cursor_byte else {
        return Cow::Borrowed(regions);
    };
    match region_at_byte(regions, cursor_byte) {
        Some(idx) => {
            let mut out = Vec::with_capacity(regions.len() - 1);
            out.extend_from_slice(&regions[..idx]);
            out.extend_from_slice(&regions[idx + 1..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(regions),
    }
}

/// Find the next display region with a `link_target` at or after `cursor_byte`.
/// Wraps to the first link if none found after cursor.
/// Returns `(byte_start, byte_end)` of the link region.
pub fn next_link_region(regions: &[DisplayRegion], cursor_byte: usize) -> Option<(usize, usize)> {
    let links: Vec<&DisplayRegion> = regions.iter().filter(|r| r.link_target.is_some()).collect();
    if links.is_empty() {
        return None;
    }
    // Find first link whose byte_start > cursor_byte (strictly after).
    let next = links.iter().find(|r| r.byte_start > cursor_byte);
    let r = next.unwrap_or(&links[0]);
    Some((r.byte_start, r.byte_end))
}

/// Find the previous display region with a `link_target` before `cursor_byte`.
/// Wraps to the last link if none found before cursor.
/// Returns `(byte_start, byte_end)` of the link region.
pub fn prev_link_region(regions: &[DisplayRegion], cursor_byte: usize) -> Option<(usize, usize)> {
    let links: Vec<&DisplayRegion> = regions.iter().filter(|r| r.link_target.is_some()).collect();
    if links.is_empty() {
        return None;
    }
    // Find last link whose byte_start < cursor_byte.
    let prev = links.iter().rposition(|r| r.byte_start < cursor_byte);
    let r = prev.map(|i| links[i]).unwrap_or(links[links.len() - 1]);
    Some((r.byte_start, r.byte_end))
}

/// Snap cursor past display regions when moving.
///
/// If `rope_col` is inside a hidden region, snaps to the appropriate edge:
/// - `forward=true`: snap to the end of the region (for move-right)
/// - `forward=false`: snap to the start of the region (for move-left)
///
/// Returns the adjusted rope char column.
pub fn snap_past_regions(
    rope_col: usize,
    line_byte_start: usize,
    line_chars: &[char],
    regions: &[DisplayRegion],
    forward: bool,
) -> usize {
    let line_str: String = line_chars.iter().collect();
    let byte_positions: Vec<usize> = line_str
        .char_indices()
        .map(|(byte_idx, _)| byte_idx)
        .collect();
    let line_byte_len = line_str.len();

    let char_to_byte = |char_idx: usize| -> usize {
        if char_idx >= byte_positions.len() {
            line_byte_start + line_byte_len
        } else {
            line_byte_start + byte_positions[char_idx]
        }
    };

    let cursor_byte = char_to_byte(rope_col);

    for region in regions {
        if cursor_byte >= region.byte_start && cursor_byte < region.byte_end {
            // Cursor is inside this region — snap to an edge.
            if forward {
                // Snap to end: find char index at region.byte_end
                let end_relative = region
                    .byte_end
                    .saturating_sub(line_byte_start)
                    .min(line_byte_len);
                let end_col = byte_positions
                    .iter()
                    .position(|&b| b >= end_relative)
                    .unwrap_or(line_chars.len());
                return end_col;
            } else {
                // Snap to start: find char index at region.byte_start
                let start_relative = region.byte_start.saturating_sub(line_byte_start);
                let start_col = byte_positions
                    .iter()
                    .position(|&b| b >= start_relative)
                    .unwrap_or(0);
                return if start_col > 0 { start_col - 1 } else { 0 };
            }
        }
    }

    rope_col
}

/// Find a display region at a given display column on a line.
/// Returns the region if the display column falls within its replacement text.
pub fn region_at_display_col<'a>(
    display_col: usize,
    line_byte_start: usize,
    line_byte_end: usize,
    line_chars: &[char],
    regions: &'a [DisplayRegion],
) -> Option<&'a DisplayRegion> {
    let (_, rope_col_map) =
        apply_display_regions_to_line(line_chars, line_byte_start, line_byte_end, regions);

    if display_col >= rope_col_map.len() {
        return None;
    }

    let rope_col = rope_col_map[display_col];

    // Build byte offset for the rope_col.
    let line_str: String = line_chars.iter().collect();
    let char_byte: usize = line_str
        .char_indices()
        .nth(rope_col)
        .map(|(b, _)| b)
        .unwrap_or(line_str.len());
    let rope_byte = line_byte_start + char_byte;

    regions
        .iter()
        .find(|r| rope_byte >= r.byte_start && rope_byte < r.byte_end)
}

// ---------------------------------------------------------------------------
// Link syntax parsing for edit-link dialog
// ---------------------------------------------------------------------------

/// Parse a markdown link `[label](url)` into `(url, label)`.
pub fn parse_md_link(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if !text.starts_with('[') {
        return None;
    }
    let close_bracket = text.find(']')?;
    let label = &text[1..close_bracket];
    let rest = &text[close_bracket + 1..];
    if !rest.starts_with('(') {
        return None;
    }
    let close_paren = rest.find(')')?;
    let url = &rest[1..close_paren];
    Some((url.to_string(), label.to_string()))
}

/// Parse an org link `[[url][label]]` or `[[url]]` into `(url, Option<label>)`.
pub fn parse_org_link(text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim();
    if !text.starts_with("[[") {
        return None;
    }
    let inner = text.strip_prefix("[[")?.strip_suffix("]]")?;
    if let Some(sep) = inner.find("][") {
        let url = &inner[..sep];
        let label = &inner[sep + 2..];
        Some((url.to_string(), Some(label.to_string())))
    } else {
        Some((inner.to_string(), None))
    }
}

/// Reconstruct markdown link syntax.
pub fn build_md_link(url: &str, label: &str) -> String {
    format!("[{}]({})", label, url)
}

/// Reconstruct org link syntax.
pub fn build_org_link(url: &str, label: Option<&str>) -> String {
    match label {
        Some(l) if !l.is_empty() => format!("[[{}][{}]]", url, l),
        _ => format!("[[{}]]", url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_link_regions ---

    #[test]
    fn compute_link_regions_markdown() {
        let text = "See [docs](https://docs.rs) for info";
        let regions = compute_link_regions(text, true, Some("md"));
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].replacement.as_deref(), Some("docs"));
        assert_eq!(regions[0].link_target.as_deref(), Some("https://docs.rs"));
        assert_eq!(regions[0].byte_start, 4);
        assert_eq!(regions[0].byte_end, 27);
    }

    #[test]
    fn compute_link_regions_org() {
        let text = "See [[https://docs.rs][docs]] here";
        let regions = compute_link_regions(text, true, Some("org"));
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].replacement.as_deref(), Some("docs"));
        assert_eq!(regions[0].link_target.as_deref(), Some("https://docs.rs"));
    }

    #[test]
    fn compute_link_regions_org_no_label() {
        let text = "See [[https://docs.rs]] here";
        let regions = compute_link_regions(text, true, Some("org"));
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].replacement.as_deref(), Some("https://docs.rs"));
        assert_eq!(regions[0].link_target.as_deref(), Some("https://docs.rs"));
    }

    #[test]
    fn compute_link_regions_mixed() {
        let text = "[md](https://md.rs) and [[https://org.rs][org]]";
        let regions = compute_link_regions(text, true, None);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].replacement.as_deref(), Some("md"));
        assert_eq!(regions[1].replacement.as_deref(), Some("org"));
    }

    #[test]
    fn compute_link_regions_disabled() {
        let text = "See [docs](https://docs.rs) for info";
        let regions = compute_link_regions(text, false, None);
        assert!(regions.is_empty());
    }

    // --- apply_display_regions_to_line ---

    #[test]
    fn apply_display_regions_basic() {
        // "[docs](https://docs.rs)" → "docs"
        let text = "[docs](https://docs.rs)";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        let (display, map) = apply_display_regions_to_line(&chars, 0, text.len(), &regions);
        let display_str: String = display.iter().collect();
        assert_eq!(display_str, "docs");
        assert_eq!(map.len(), 4); // 4 display chars
                                  // All map to char 0 (region start)
        assert!(map.iter().all(|&c| c == 0));
    }

    #[test]
    fn apply_display_regions_multiple() {
        let text = "[a](https://a.com) and [b](https://b.com)";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        assert_eq!(regions.len(), 2);
        let (display, map) = apply_display_regions_to_line(&chars, 0, text.len(), &regions);
        let display_str: String = display.iter().collect();
        assert_eq!(display_str, "a and b");
        assert_eq!(map.len(), 7);
    }

    #[test]
    fn apply_display_regions_no_regions() {
        let text = "plain text here";
        let chars: Vec<char> = text.chars().collect();
        let (display, map) = apply_display_regions_to_line(&chars, 0, text.len(), &[]);
        let display_str: String = display.iter().collect();
        assert_eq!(display_str, "plain text here");
        assert_eq!(map, (0..15).collect::<Vec<_>>());
    }

    // --- rope_col_to_display_col ---

    #[test]
    fn rope_col_to_display_col_basic() {
        // "See [docs](url) here" with region replacing [docs](url)
        let text = "See [docs](https://docs.rs) here";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        let (_, map) = apply_display_regions_to_line(&chars, 0, text.len(), &regions);
        // "See docs here" — display cols
        // Rope col 0 → display col 0 ("S")
        assert_eq!(rope_col_to_display_col(0, &map), 0);
        // Rope col 3 → display col 3 (" ")
        assert_eq!(rope_col_to_display_col(3, &map), 3);
        // Rope col 4 → display col 4 (start of replacement "d")
        assert_eq!(rope_col_to_display_col(4, &map), 4);
        // Rope col 27 → display col 8 (" " after "docs")
        assert_eq!(rope_col_to_display_col(27, &map), 8);
    }

    // --- display_col_to_rope_col ---

    #[test]
    fn display_col_to_rope_col_basic() {
        let text = "See [docs](https://docs.rs) here";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        let (_, map) = apply_display_regions_to_line(&chars, 0, text.len(), &regions);
        // Display col 0 → rope col 0 (S)
        assert_eq!(display_col_to_rope_col(0, &map), 0);
        // Display col 4 → rope col 4 (start of region, maps to 'd' in replacement)
        assert_eq!(display_col_to_rope_col(4, &map), 4);
    }

    // --- snap_past_regions ---

    #[test]
    fn snap_past_regions_forward() {
        let text = "See [docs](https://docs.rs) here";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor at rope col 5 (inside "[docs](url)") → snap forward to end
        let snapped = snap_past_regions(5, 0, &chars, &regions, true);
        assert_eq!(snapped, 27); // byte_end of region = 27, char at that position
    }

    #[test]
    fn snap_past_regions_backward() {
        let text = "See [docs](https://docs.rs) here";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor at rope col 10 (inside hidden part) → snap backward
        let snapped = snap_past_regions(10, 0, &chars, &regions, false);
        assert_eq!(snapped, 3); // just before the region start
    }

    // --- Integration: region_at_display_col ---

    #[test]
    fn region_at_display_col_finds_link() {
        let text = "See [docs](https://docs.rs) here";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        // Display col 4 is "d" in "docs" replacement
        let r = region_at_display_col(4, 0, text.len(), &chars, &regions);
        assert!(r.is_some());
        assert_eq!(r.unwrap().link_target.as_deref(), Some("https://docs.rs"));
    }

    #[test]
    fn region_at_display_col_outside_link() {
        let text = "See [docs](https://docs.rs) here";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        // Display col 0 is "S" — not in a region
        let r = region_at_display_col(0, 0, text.len(), &chars, &regions);
        assert!(r.is_none());
    }

    // --- region_at_byte ---

    #[test]
    fn region_at_byte_inside() {
        let text = "See [docs](https://docs.rs) here";
        let regions = compute_link_regions(text, true, Some("md"));
        // byte 5 is inside the region (byte_start=4, byte_end=27)
        assert_eq!(region_at_byte(&regions, 5), Some(0));
    }

    #[test]
    fn region_at_byte_outside() {
        let text = "See [docs](https://docs.rs) here";
        let regions = compute_link_regions(text, true, Some("md"));
        // byte 0 is before the region
        assert_eq!(region_at_byte(&regions, 0), None);
        // byte 28 is after the region
        assert_eq!(region_at_byte(&regions, 28), None);
    }

    // --- regions_with_cursor_reveal ---

    #[test]
    fn regions_with_cursor_reveal_inside() {
        let text = "[a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        assert_eq!(regions.len(), 2);
        // Cursor inside first region → only first removed
        let cursor_byte = 2; // inside [a](...)
        let revealed = regions_with_cursor_reveal(&regions, Some(cursor_byte));
        assert_eq!(revealed.len(), 1);
        assert_eq!(revealed[0].replacement.as_deref(), Some("b"));
    }

    #[test]
    fn regions_with_cursor_reveal_outside() {
        let text = "[a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor outside all regions → all kept
        let revealed = regions_with_cursor_reveal(&regions, Some(20));
        assert_eq!(revealed.len(), 2);
    }

    #[test]
    fn regions_with_cursor_reveal_none() {
        let text = "[a](https://a.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        // No reveal cursor → all kept
        let revealed = regions_with_cursor_reveal(&regions, None);
        assert_eq!(revealed.len(), 1);
    }

    // --- next_link_region / prev_link_region ---

    #[test]
    fn next_link_region_basic() {
        let text = "text [a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        assert_eq!(regions.len(), 2);
        // Cursor before first link → next is first link
        let (start, _) = next_link_region(&regions, 0).unwrap();
        assert_eq!(start, regions[0].byte_start);
    }

    #[test]
    fn next_link_region_skips_current() {
        let text = "text [a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor at region[0].byte_start → should find region[1] (strictly after)
        let (start, _) = next_link_region(&regions, regions[0].byte_start).unwrap();
        assert_eq!(start, regions[1].byte_start);
    }

    #[test]
    fn next_link_region_wraps() {
        let text = "text [a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor past last link → wraps to first
        let (start, _) = next_link_region(&regions, 100).unwrap();
        assert_eq!(start, regions[0].byte_start);
    }

    #[test]
    fn prev_link_region_basic() {
        let text = "text [a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor at end → prev is second link
        let (start, _) = prev_link_region(&regions, 100).unwrap();
        assert_eq!(start, regions[1].byte_start);
    }

    #[test]
    fn prev_link_region_wraps() {
        let text = "text [a](https://a.com) and [b](https://b.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        // Cursor before first link → wraps to last
        let (start, _) = prev_link_region(&regions, 0).unwrap();
        assert_eq!(start, regions[1].byte_start);
    }

    #[test]
    fn link_region_empty() {
        let regions: Vec<DisplayRegion> = vec![];
        assert!(next_link_region(&regions, 0).is_none());
        assert!(prev_link_region(&regions, 0).is_none());
    }

    // --- compute_image_regions ---

    #[test]
    fn compute_image_regions_markdown() {
        let text = "![diagram](./img/arch.png)";
        let empty = std::collections::HashSet::new();
        // Use /tmp as base_dir; the file won't exist, so image will be None.
        let regions =
            compute_image_regions(text, Some("md"), Some(std::path::Path::new("/tmp")), &empty);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].replacement.as_deref(), Some("[Image: arch.png]"));
        assert_eq!(regions[0].link_target.as_deref(), Some("./img/arch.png"));
        assert!(regions[0].image.is_none()); // file doesn't exist
    }

    #[test]
    fn compute_image_regions_org() {
        let text = "[[./media/photo.jpg][My Photo]]";
        let empty = std::collections::HashSet::new();
        let regions = compute_image_regions(
            text,
            Some("org"),
            Some(std::path::Path::new("/tmp")),
            &empty,
        );
        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions[0].replacement.as_deref(),
            Some("[Image: photo.jpg]")
        );
    }

    #[test]
    fn compute_image_regions_non_image_link() {
        let text = "![readme](./README.md)";
        let empty = std::collections::HashSet::new();
        let regions =
            compute_image_regions(text, Some("md"), Some(std::path::Path::new("/tmp")), &empty);
        assert!(regions.is_empty()); // .md is not an image
    }

    #[test]
    fn compute_image_regions_collapsed() {
        let text = "![diagram](./img/arch.png)";
        let mut collapsed = std::collections::HashSet::new();
        collapsed.insert(0); // collapse line 0
        let regions = compute_image_regions(
            text,
            Some("md"),
            Some(std::path::Path::new("/tmp")),
            &collapsed,
        );
        assert!(regions.is_empty()); // collapsed = not shown
    }

    #[test]
    fn compute_image_regions_no_base_dir() {
        let text = "![img](photo.png)";
        let empty = std::collections::HashSet::new();
        let regions = compute_image_regions(text, Some("md"), None, &empty);
        assert!(regions.is_empty()); // unsaved buffer = no image resolution
    }

    #[test]
    fn compute_image_regions_org_with_attr_width() {
        let text = "#+attr_html: :width 600px\n[[./media/diagram.png]]";
        let empty = std::collections::HashSet::new();
        let regions = compute_image_regions(
            text,
            Some("org"),
            Some(std::path::Path::new("/tmp")),
            &empty,
        );
        assert_eq!(regions.len(), 1);
        // The image should have parsed the width from the preceding line.
        // image is None because file doesn't exist, but we can verify the region was created.
        assert_eq!(
            regions[0].replacement.as_deref(),
            Some("[Image: diagram.png]")
        );
    }

    #[test]
    fn compute_image_regions_real_fixture_md() {
        // Use the actual test-image.png fixture to verify ImageAttrs population.
        let assets_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("assets");
        if !assets_dir.join("test-image.png").exists() {
            return;
        }
        let text = "![Test](test-image.png)";
        let empty = std::collections::HashSet::new();
        let regions = compute_image_regions(text, Some("md"), Some(&assets_dir), &empty);
        assert_eq!(regions.len(), 1);
        let img = regions[0]
            .image
            .as_ref()
            .expect("should resolve existing image");
        assert_eq!(img.path, assets_dir.join("test-image.png"));
        assert_eq!(img.width, None); // no explicit width attribute
    }

    #[test]
    fn compute_image_regions_real_fixture_org() {
        let assets_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("assets");
        if !assets_dir.join("test-image.png").exists() {
            return;
        }
        let text = "#+attr_html: :width 200px\n[[file:test-image.png]]";
        let empty = std::collections::HashSet::new();
        let regions = compute_image_regions(text, Some("org"), Some(&assets_dir), &empty);
        assert_eq!(regions.len(), 1);
        let img = regions[0]
            .image
            .as_ref()
            .expect("should resolve existing image with width");
        assert_eq!(img.path, assets_dir.join("test-image.png"));
        assert_eq!(img.width, Some(200));
    }

    // --- ImageAttrs cached dimensions ---

    #[test]
    fn image_attrs_has_cached_dimensions() {
        let assets_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("assets");
        if !assets_dir.join("test-image.png").exists() {
            return;
        }
        let text = "![Test](test-image.png)";
        let empty = std::collections::HashSet::new();
        let regions = compute_image_regions(text, Some("md"), Some(&assets_dir), &empty);
        assert_eq!(regions.len(), 1);
        let img = regions[0].image.as_ref().unwrap();
        assert!(
            img.natural_width > 0 && img.natural_height > 0,
            "Expected cached dimensions, got {}x{}",
            img.natural_width,
            img.natural_height
        );
    }

    // --- Link parsing ---

    #[test]
    fn parse_md_link_basic() {
        let (url, label) = parse_md_link("[Click](http://example.com)").unwrap();
        assert_eq!(url, "http://example.com");
        assert_eq!(label, "Click");
    }

    #[test]
    fn parse_md_link_empty_label() {
        let (url, label) = parse_md_link("[](http://example.com)").unwrap();
        assert_eq!(url, "http://example.com");
        assert_eq!(label, "");
    }

    #[test]
    fn parse_md_link_invalid() {
        assert!(parse_md_link("not a link").is_none());
        assert!(parse_md_link("[no url]").is_none());
    }

    #[test]
    fn parse_org_link_with_label() {
        let (url, label) = parse_org_link("[[http://example.com][Click]]").unwrap();
        assert_eq!(url, "http://example.com");
        assert_eq!(label, Some("Click".to_string()));
    }

    #[test]
    fn parse_org_link_no_label() {
        let (url, label) = parse_org_link("[[http://example.com]]").unwrap();
        assert_eq!(url, "http://example.com");
        assert_eq!(label, None);
    }

    #[test]
    fn parse_org_link_invalid() {
        assert!(parse_org_link("not a link").is_none());
    }

    #[test]
    fn build_md_link_roundtrip() {
        assert_eq!(build_md_link("http://x.com", "X"), "[X](http://x.com)");
    }

    #[test]
    fn build_org_link_with_label() {
        assert_eq!(
            build_org_link("http://x.com", Some("X")),
            "[[http://x.com][X]]"
        );
    }

    #[test]
    fn build_org_link_no_label() {
        assert_eq!(build_org_link("http://x.com", None), "[[http://x.com]]");
    }

    // --- Cow regions_with_cursor_reveal ---

    #[test]
    fn regions_with_cursor_reveal_borrows_when_no_reveal() {
        let text = "[a](https://a.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        let result = regions_with_cursor_reveal(&regions, None);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn regions_with_cursor_reveal_borrows_when_outside() {
        let text = "[a](https://a.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        let result = regions_with_cursor_reveal(&regions, Some(100));
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn regions_with_cursor_reveal_owns_when_inside() {
        let text = "[a](https://a.com)";
        let regions = compute_link_regions(text, true, Some("md"));
        let result = regions_with_cursor_reveal(&regions, Some(2));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.is_empty());
    }

    // --- Binary search in apply_display_regions_to_line ---

    #[test]
    fn apply_display_regions_binary_search_correctness() {
        // Multiple regions: verify binary search produces same results as linear scan.
        let text = "A [x](u1) B [y](u2) C [z](u3) D";
        let chars: Vec<char> = text.chars().collect();
        let regions = compute_link_regions(text, true, Some("md"));
        assert_eq!(regions.len(), 3);
        let (display, map) = apply_display_regions_to_line(&chars, 0, text.len(), &regions);
        let display_str: String = display.iter().collect();
        assert_eq!(display_str, "A x B y C z D");
        assert_eq!(map.len(), display_str.len());
    }
}
