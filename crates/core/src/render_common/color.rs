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
}
