//! Collaborative cursor color assignment and helpers.
//!
//! Provides a deterministic 8-color palette for remote user cursors/selections.
//! Colors are assigned via FNV-1a hash of client_id, ensuring stable assignment
//! across sessions. The palette is WCAG AA accessible and colorblind-safe.

use crate::theme::ThemeColor;

/// Number of colors in the collaborative palette.
pub const COLLAB_PALETTE_SIZE: usize = 8;

/// Dark theme collaborative palette (WCAG AA against dark backgrounds).
pub const DARK_PALETTE: [(u8, u8, u8); COLLAB_PALETTE_SIZE] = [
    (0xFF, 0x6B, 0x6B), // 0: Ruby
    (0x60, 0xA5, 0xFA), // 1: Sapphire
    (0x34, 0xD3, 0x99), // 2: Emerald
    (0xFB, 0xBF, 0x24), // 3: Amber
    (0xA7, 0x8B, 0xFA), // 4: Violet
    (0x22, 0xD3, 0xEE), // 5: Cyan
    (0xF4, 0x72, 0xB6), // 6: Rose
    (0x94, 0xA3, 0xB8), // 7: Slate
];

/// Light theme collaborative palette (WCAG AA against light backgrounds).
pub const LIGHT_PALETTE: [(u8, u8, u8); COLLAB_PALETTE_SIZE] = [
    (0xDC, 0x26, 0x26), // 0: Ruby (darker)
    (0x25, 0x63, 0xEB), // 1: Sapphire (darker)
    (0x05, 0x96, 0x69), // 2: Emerald (darker)
    (0xD9, 0x77, 0x06), // 3: Amber (darker)
    (0x7C, 0x3A, 0xED), // 4: Violet (darker)
    (0x06, 0x91, 0xB2), // 5: Cyan (darker)
    (0xDB, 0x27, 0x77), // 6: Rose (darker)
    (0x64, 0x74, 0x8B), // 7: Slate (darker)
];

/// Compute a deterministic color index for a client_id.
///
/// Uses FNV-1a hash to distribute clients across the 8-color palette.
/// The same client_id always maps to the same color index.
pub fn collab_color_index(client_id: u64) -> usize {
    let bytes = client_id.to_le_bytes();
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in &bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % COLLAB_PALETTE_SIZE as u64) as usize
}

/// Return the theme style key for a collab cursor at the given palette index.
pub fn collab_cursor_style_key(index: usize) -> String {
    format!("ui.collab.cursor.{}", index % COLLAB_PALETTE_SIZE)
}

/// Return the theme style key for a collab selection at the given palette index.
pub fn collab_selection_style_key(index: usize) -> String {
    format!("ui.collab.selection.{}", index % COLLAB_PALETTE_SIZE)
}

/// Compute a selection color by blending a base color with alpha towards a background.
///
/// Returns a new ThemeColor with the base color at `alpha` opacity over `bg`.
pub fn collab_selection_alpha(base: ThemeColor, bg: ThemeColor, alpha: f32) -> ThemeColor {
    let (br, bg_g, bb) = match bg {
        ThemeColor::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        ThemeColor::Named(_) => (30.0, 30.0, 30.0), // dark fallback
    };
    let (fr, fg, fb) = match base {
        ThemeColor::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        ThemeColor::Named(_) => (200.0, 200.0, 200.0),
    };
    let a = alpha.clamp(0.0, 1.0);
    ThemeColor::Rgb(
        (fr * a + br * (1.0 - a)) as u8,
        (fg * a + bg_g * (1.0 - a)) as u8,
        (fb * a + bb * (1.0 - a)) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_index_deterministic() {
        let idx1 = collab_color_index(42);
        let idx2 = collab_color_index(42);
        assert_eq!(idx1, idx2);
    }

    #[test]
    fn color_index_wraps() {
        for id in 0..100u64 {
            let idx = collab_color_index(id);
            assert!(
                idx < COLLAB_PALETTE_SIZE,
                "index {} out of range for id {}",
                idx,
                id
            );
        }
    }

    #[test]
    fn color_index_distributes() {
        // With 100 clients, all 8 bins should be hit
        let mut bins = [0u32; COLLAB_PALETTE_SIZE];
        for id in 0..100u64 {
            bins[collab_color_index(id)] += 1;
        }
        for (i, &count) in bins.iter().enumerate() {
            assert!(count > 0, "bin {} got zero clients", i);
        }
    }

    #[test]
    fn cursor_style_key_format() {
        assert_eq!(collab_cursor_style_key(0), "ui.collab.cursor.0");
        assert_eq!(collab_cursor_style_key(7), "ui.collab.cursor.7");
        assert_eq!(collab_cursor_style_key(8), "ui.collab.cursor.0"); // wraps
    }

    #[test]
    fn selection_alpha_blend() {
        let base = ThemeColor::Rgb(255, 0, 0);
        let bg = ThemeColor::Rgb(0, 0, 0);
        let result = collab_selection_alpha(base, bg, 0.2);
        // 255 * 0.2 = 51
        assert_eq!(result, ThemeColor::Rgb(51, 0, 0));
    }

    #[test]
    fn selection_alpha_full() {
        let base = ThemeColor::Rgb(100, 150, 200);
        let bg = ThemeColor::Rgb(0, 0, 0);
        let result = collab_selection_alpha(base, bg, 1.0);
        assert_eq!(result, ThemeColor::Rgb(100, 150, 200));
    }

    #[test]
    fn dark_palette_has_8_colors() {
        assert_eq!(DARK_PALETTE.len(), COLLAB_PALETTE_SIZE);
    }

    #[test]
    fn light_palette_has_8_colors() {
        assert_eq!(LIGHT_PALETTE.len(), COLLAB_PALETTE_SIZE);
    }
}
