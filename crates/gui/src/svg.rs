//! SVG parsing and rasterization for inline images — `resvg`, not Skia's SVG module.
//!
//! @ai-caution: [rendering] Skia's `svg` feature is deliberately NOT enabled.
//! rust-skia publishes prebuilt Skia binaries only for particular feature sets,
//! and on Linux none of them includes `svg` without also including `gl` + `x11`
//! (checked for 0.99 and 0.153). A set with no published binary makes
//! `skia-bindings` build Skia from source, and that is not a viable fallback:
//! in 2026-09 the 0.99 source build failed on a pinned third-party commit that
//! no longer exists upstream, and CI had only ever passed on a cached build.
//! Enabling `gl`/`x11` to get a binary would have made every Linux build link
//! libGL — including on machines with no GL development headers — for a
//! renderer that is CPU-raster by design.
//!
//! `resvg` is pure Rust, renders onto `tiny-skia` (already in the dependency
//! tree), and is the reference-grade SVG renderer in the Rust ecosystem. The
//! cost of the switch is that an SVG is rasterized at its display size rather
//! than drawn as vectors each frame; callers cache the raster per size.

use std::sync::{Arc, OnceLock};

use resvg::{tiny_skia, usvg};

/// A parsed SVG document, cheap to clone.
pub type SvgTree = Arc<usvg::Tree>;

/// Parse options with the system fonts loaded — once per process, since
/// scanning the font directories is the expensive part and `<text>` in an SVG
/// renders nothing without them.
fn options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut opt = usvg::Options::default();
        let db = opt.fontdb_mut();
        db.load_system_fonts();
        map_generic_families(db);
        opt
    })
}

/// Installed families to try for each CSS generic, most common first across
/// Linux distributions, macOS and Windows.
const SANS: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Liberation Sans",
    "Cantarell",
    "Helvetica",
    "Arial",
    "Segoe UI",
];
const SERIF: &[&str] = &[
    "DejaVu Serif",
    "Noto Serif",
    "Liberation Serif",
    "Times",
    "Times New Roman",
];
const MONO: &[&str] = &[
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "JetBrains Mono",
    "Menlo",
    "Consolas",
    "Courier New",
];

/// Point `sans-serif`/`serif`/`monospace` at fonts that are actually installed.
///
/// `fontdb` maps the generics to Windows families (Arial, Times New Roman,
/// Courier New) by default, which most Linux systems do not have — so an SVG's
/// `font-family="sans-serif"` text resolved to NOTHING and silently vanished.
/// Skia's SVG module used fontconfig and never had this gap; it was caught by
/// rendering a real SVG after the switch, not by a unit test.
fn map_generic_families(db: &mut usvg::fontdb::Database) {
    fn first_installed(db: &usvg::fontdb::Database, candidates: &[&str]) -> Option<String> {
        candidates
            .iter()
            .find(|want| {
                db.faces().any(|f| {
                    f.families
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(want))
                })
            })
            .map(|s| s.to_string())
    }
    if let Some(f) = first_installed(db, SANS) {
        db.set_sans_serif_family(f);
    }
    if let Some(f) = first_installed(db, SERIF) {
        db.set_serif_family(f);
    }
    if let Some(f) = first_installed(db, MONO) {
        db.set_monospace_family(f);
    }
}

/// Parse SVG bytes. `None` for anything `usvg` rejects.
pub fn parse(bytes: &[u8]) -> Option<SvgTree> {
    usvg::Tree::from_data(bytes, options()).ok().map(Arc::new)
}

/// The document's own size in CSS pixels (from `width`/`height`, else the
/// `viewBox`).
pub fn intrinsic_size(tree: &usvg::Tree) -> (f32, f32) {
    let size = tree.size();
    (size.width(), size.height())
}

/// Rasterize `tree` scaled to fill `width` x `height` device pixels, as a Skia
/// image ready to draw. `None` for an empty or degenerate target.
pub fn rasterize(tree: &usvg::Tree, width: u32, height: u32) -> Option<skia_safe::Image> {
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let (w, h) = intrinsic_size(tree);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let transform = tiny_skia::Transform::from_scale(width as f32 / w, height as f32 / h);
    resvg::render(tree, transform, &mut pixmap.as_mut());

    // tiny-skia stores premultiplied RGBA, row-major, tightly packed.
    let info = skia_safe::ImageInfo::new(
        (width as i32, height as i32),
        skia_safe::ColorType::RGBA8888,
        skia_safe::AlphaType::Premul,
        None,
    );
    let row_bytes = width as usize * 4;
    skia_safe::images::raster_from_data(&info, skia_safe::Data::new_copy(pixmap.data()), row_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &[u8] =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
        <rect x="0" y="0" width="20" height="20" fill="#ff0000"/>
    </svg>"##;

    #[test]
    fn intrinsic_size_comes_from_the_document() {
        let tree = parse(SQUARE).expect("valid svg");
        assert_eq!(intrinsic_size(&tree), (40.0, 20.0));
    }

    /// Pixel oracle, not just "an image came back": the left half is the red
    /// rect, the right half is transparent — at a scaled size, so the transform
    /// is exercised too.
    #[test]
    fn rasterizes_the_shape_where_it_is_scaled_to() {
        let tree = parse(SQUARE).unwrap();
        let img = rasterize(&tree, 80, 40).expect("raster");
        assert_eq!((img.width(), img.height()), (80, 40));

        let info = skia_safe::ImageInfo::new(
            (80, 40),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Premul,
            None,
        );
        let mut px = vec![0u8; 80 * 40 * 4];
        assert!(img.read_pixels(
            &info,
            &mut px,
            80 * 4,
            (0, 0),
            skia_safe::image::CachingHint::Allow
        ));
        let at = |x: usize, y: usize| &px[(y * 80 + x) * 4..(y * 80 + x) * 4 + 4];
        assert_eq!(at(10, 20), &[255, 0, 0, 255], "inside the rect: opaque red");
        assert_eq!(at(70, 20), &[0, 0, 0, 0], "outside it: transparent");
    }

    /// `font-family="sans-serif"` text must produce ink. Conditional on the
    /// machine having ANY of the sans candidates — a CI image with no fonts at
    /// all cannot render text by any route, and must not fail for that.
    #[test]
    fn generic_sans_serif_text_renders_when_a_sans_font_is_installed() {
        let db = &options().fontdb;
        let has_sans = db.faces().any(|f| {
            f.families
                .iter()
                .any(|(n, _)| SANS.iter().any(|c| n.eq_ignore_ascii_case(c)))
        });
        if !has_sans {
            eprintln!("no sans candidate installed; skipping");
            return;
        }
        let tree = parse(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
                <text x="10" y="40" font-family="sans-serif" font-size="32" fill="#000000">Mae</text>
            </svg>"##,
        )
        .unwrap();
        let img = rasterize(&tree, 200, 60).unwrap();
        let info = skia_safe::ImageInfo::new(
            (200, 60),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Premul,
            None,
        );
        let mut px = vec![0u8; 200 * 60 * 4];
        assert!(img.read_pixels(
            &info,
            &mut px,
            200 * 4,
            (0, 0),
            skia_safe::image::CachingHint::Allow
        ));
        let inked = px.chunks(4).filter(|p| p[3] > 0).count();
        assert!(
            inked > 50,
            "generic sans-serif text drew {inked} pixels — it resolved to no font"
        );
    }

    #[test]
    fn garbage_is_rejected_not_rendered() {
        assert!(parse(b"not svg at all").is_none());
    }

    #[test]
    fn a_degenerate_target_yields_nothing() {
        let tree = parse(SQUARE).unwrap();
        assert!(rasterize(&tree, 0, 10).is_none());
    }
}
