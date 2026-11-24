//! What a file is, read from its bytes (ADR-115 D3a). The client's
//! `Content-Type` and the file's name are never consulted: they are claims, and
//! a claim is what a polyglot or a mislabelled upload lies with.

/// The types a blob is stored as. Everything not recognised is
/// [`MediaType::Other`], stored and served as `application/octet-stream`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Png,
    Jpeg,
    Gif,
    Webp,
    Avif,
    Svg,
    /// A PDF (a revision's rendering, ADR-113 D6, or any attached document):
    /// stored exactly as it arrived, served only as a download.
    Pdf,
    Other,
}

impl MediaType {
    /// The `Content-Type` this type is stored and served under.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Png => "image/png",
            MediaType::Jpeg => "image/jpeg",
            MediaType::Gif => "image/gif",
            MediaType::Webp => "image/webp",
            MediaType::Avif => "image/avif",
            MediaType::Svg => "image/svg+xml",
            MediaType::Pdf => "application/pdf",
            MediaType::Other => "application/octet-stream",
        }
    }

    /// The type a stored `media_type` names; anything unknown is `Other`.
    pub fn parse(s: &str) -> MediaType {
        [
            MediaType::Png,
            MediaType::Jpeg,
            MediaType::Gif,
            MediaType::Webp,
            MediaType::Avif,
            MediaType::Svg,
            MediaType::Pdf,
        ]
        .into_iter()
        .find(|t| t.as_str() == s)
        .unwrap_or(MediaType::Other)
    }

    /// Whether a browser may display it inline from the API origin (D3): the
    /// raster images. SVG never is, and neither is anything else.
    pub fn inline(self) -> bool {
        matches!(
            self,
            MediaType::Png | MediaType::Jpeg | MediaType::Gif | MediaType::Webp | MediaType::Avif
        )
    }
}

/// The type of `bytes`, from their content alone.
pub fn sniff(bytes: &[u8]) -> MediaType {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        MediaType::Png
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        MediaType::Jpeg
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        MediaType::Gif
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        MediaType::Webp
    } else if bytes.starts_with(b"%PDF-") {
        // The header at offset 0. PDF readers accept it anywhere in the first
        // 1024 bytes; a file that puts it later is not typed a PDF here, so
        // nothing else can be prefixed to one and still be served as one.
        MediaType::Pdf
    } else if is_avif(bytes) {
        MediaType::Avif
    } else if super::svg::is_svg(bytes) {
        MediaType::Svg
    } else {
        MediaType::Other
    }
}

/// An ISO-BMFF `ftyp` box whose major brand is AVIF's.
fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis")
}
