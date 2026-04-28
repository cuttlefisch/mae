//! Heading scale/level utilities shared between core scroll logic and GUI rendering.

/// Default heading scale factors by level.
/// Level 1 = largest (e.g., `# Title` or `* Title`), level 4+ = normal.
pub fn heading_scale_for_level(level: u8) -> f32 {
    match level {
        1 => 1.5,
        2 => 1.3,
        3 => 1.15,
        _ => 1.0,
    }
}

/// Detect heading level from a line's leading characters.
/// Org-mode uses `*`, Markdown uses `#`. Returns 0 if not a heading.
pub fn heading_level_from_chars(first_chars: &[char]) -> u8 {
    if first_chars.is_empty() {
        return 0;
    }
    let marker = match first_chars[0] {
        '*' | '#' => first_chars[0],
        _ => return 0,
    };
    let count = first_chars.iter().take_while(|&&c| c == marker).count();
    // Must be followed by a space to be a heading (not `***bold***` or `###`).
    if count < first_chars.len() && first_chars[count] == ' ' {
        count.min(255) as u8
    } else {
        0
    }
}

/// Return how many visual cell rows a line occupies, accounting for heading scale.
/// Returns `ceil(scale)` — always >= 1.
/// If `heading_scale` is false, always returns 1.
pub fn line_heading_visual_rows(line_chars: &[char], heading_scale: bool) -> usize {
    if !heading_scale {
        return 1;
    }
    let level = heading_level_from_chars(line_chars);
    if level == 0 {
        return 1;
    }
    let scale = heading_scale_for_level(level);
    scale.ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_level_org() {
        let chars: Vec<char> = "* Title".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 1);
        let chars: Vec<char> = "** Sub".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 2);
        let chars: Vec<char> = "*** Deep".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 3);
    }

    #[test]
    fn heading_level_markdown() {
        let chars: Vec<char> = "# Title".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 1);
        let chars: Vec<char> = "## Sub".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 2);
    }

    #[test]
    fn heading_level_not_a_heading() {
        let chars: Vec<char> = "normal text".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 0);
        // No space after markers → not a heading.
        let chars: Vec<char> = "###notheading".chars().collect();
        assert_eq!(heading_level_from_chars(&chars), 0);
        assert_eq!(heading_level_from_chars(&[]), 0);
    }

    #[test]
    fn visual_rows_with_heading_scale() {
        // Level 1 heading: scale 1.5 → ceil = 2 rows.
        let chars: Vec<char> = "# Big Title".chars().collect();
        assert_eq!(line_heading_visual_rows(&chars, true), 2);
        // Level 2: scale 1.3 → ceil = 2.
        let chars: Vec<char> = "## Medium".chars().collect();
        assert_eq!(line_heading_visual_rows(&chars, true), 2);
        // Level 3: scale 1.15 → ceil = 2.
        let chars: Vec<char> = "### Small".chars().collect();
        assert_eq!(line_heading_visual_rows(&chars, true), 2);
        // Level 4+: scale 1.0 → 1 row.
        let chars: Vec<char> = "#### Tiny".chars().collect();
        assert_eq!(line_heading_visual_rows(&chars, true), 1);
        // Normal text: 1 row.
        let chars: Vec<char> = "normal".chars().collect();
        assert_eq!(line_heading_visual_rows(&chars, true), 1);
    }

    #[test]
    fn visual_rows_heading_scale_disabled() {
        let chars: Vec<char> = "# Big Title".chars().collect();
        assert_eq!(line_heading_visual_rows(&chars, false), 1);
    }
}
