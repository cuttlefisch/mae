//! Shared color utilities used by both GUI and TUI renderers.

/// Parse a 6-digit hex color string (e.g. "ff5733") into (R, G, B).
pub fn parse_hex6(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Parse a 3-digit hex color string (e.g. "f00") into (R, G, B).
pub fn parse_hex3(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 3 {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    let r = u8::from_str_radix(&format!("{0}{0}", chars[0]), 16).ok()?;
    let g = u8::from_str_radix(&format!("{0}{0}", chars[1]), 16).ok()?;
    let b = u8::from_str_radix(&format!("{0}{0}", chars[2]), 16).ok()?;
    Some((r, g, b))
}

/// Return a luminance value for choosing contrast foreground text.
/// Uses the standard perceived-brightness formula.
pub fn luminance(r: u8, g: u8, b: u8) -> f64 {
    0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64
}

/// `true` when black foreground text reads better than white on this RGB
/// background (relative luminance above the midpoint). Shared by both
/// backends' hex-color-preview contrast decision — each maps this to its own
/// color type (ratatui `Color` vs Skia `Color4f`).
pub fn prefers_dark_fg(r: u8, g: u8, b: u8) -> bool {
    luminance(r, g, b) > 128.0
}

/// Scan `chars` for `#rrggbb`/`#rgb` hex color literals, returning each
/// match's half-open char-index span and parsed RGB. Shared by both
/// backends' hex-color-preview rendering (`apply_hex_color_preview`) — each
/// applies the spans to its own style type.
pub fn find_hex_color_runs(chars: &[char]) -> Vec<(std::ops::Range<usize>, (u8, u8, u8))> {
    let len = chars.len();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < len {
        if chars[i] == '#' {
            // Try #rrggbb (7 chars total)
            if i + 7 <= len && chars[i + 1..i + 7].iter().all(|c| c.is_ascii_hexdigit()) {
                let hex: String = chars[i + 1..i + 7].iter().collect();
                if let Some(rgb) = parse_hex6(&hex) {
                    runs.push((i..i + 7, rgb));
                    i += 7;
                    continue;
                }
            }
            // Try #rgb (4 chars total)
            if i + 4 <= len && chars[i + 1..i + 4].iter().all(|c| c.is_ascii_hexdigit()) {
                let hex: String = chars[i + 1..i + 4].iter().collect();
                if let Some(rgb) = parse_hex3(&hex) {
                    runs.push((i..i + 4, rgb));
                    i += 4;
                    continue;
                }
            }
        }
        i += 1;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex6_valid() {
        assert_eq!(parse_hex6("ff5733"), Some((255, 87, 51)));
        assert_eq!(parse_hex6("000000"), Some((0, 0, 0)));
        assert_eq!(parse_hex6("ffffff"), Some((255, 255, 255)));
    }

    #[test]
    fn parse_hex3_valid() {
        assert_eq!(parse_hex3("f00"), Some((255, 0, 0)));
        assert_eq!(parse_hex3("fff"), Some((255, 255, 255)));
    }

    #[test]
    fn parse_hex_invalid() {
        assert_eq!(parse_hex6("zzzzzz"), None);
        assert_eq!(parse_hex6("fff"), None);
        assert_eq!(parse_hex3("ffffff"), None);
        assert_eq!(parse_hex3("zz"), None);
    }

    #[test]
    fn luminance_black_white() {
        assert!(luminance(0, 0, 0) < 1.0);
        assert!(luminance(255, 255, 255) > 250.0);
    }

    #[test]
    fn luminance_threshold() {
        // White bg → lum > 128 → dark fg
        assert!(luminance(255, 255, 255) > 128.0);
        // Black bg → lum < 128 → light fg
        assert!(luminance(0, 0, 0) < 128.0);
    }

    #[test]
    fn prefers_dark_fg_matches_luminance_threshold() {
        assert!(prefers_dark_fg(255, 255, 255), "white bg wants black text");
        assert!(!prefers_dark_fg(0, 0, 0), "black bg wants white text");
    }

    #[test]
    fn find_hex_color_runs_detects_6_and_3_digit() {
        let chars: Vec<char> = "a #ff5733 b #f00 c".chars().collect();
        let runs = find_hex_color_runs(&chars);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].1, (255, 87, 51));
        assert_eq!(
            &chars[runs[0].0.clone()].iter().collect::<String>(),
            "#ff5733"
        );
        assert_eq!(runs[1].1, (255, 0, 0));
        assert_eq!(&chars[runs[1].0.clone()].iter().collect::<String>(), "#f00");
    }

    #[test]
    fn find_hex_color_runs_ignores_invalid_hex() {
        let chars: Vec<char> = "#zzzzzz #12 plain text".chars().collect();
        assert!(find_hex_color_runs(&chars).is_empty());
    }

    #[test]
    fn find_hex_color_runs_empty_input() {
        assert!(find_hex_color_runs(&[]).is_empty());
    }
}
