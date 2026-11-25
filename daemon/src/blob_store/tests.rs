//! ADR-115 P1's store and processing, against real files and real images: what
//! is stored is processed, addressed by the server, stripped, bounded, charged
//! once, and never left half-written.

use std::io::Cursor;
use std::sync::Arc;

use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, Frame, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};
use sha2::Digest;

use super::process::{process, ProcessError, ProcessLimits};
use super::upload::{ReceiveError, Receiving};
use super::*;

const LIMITS: ProcessLimits = ProcessLimits {
    max_pixels: 40_000_000,
    max_frames: 1000,
};

/// The GPS latitude 51/1, 30/1, 2649/100 as TIFF rationals (little endian):
/// the bytes a leak of the location would carry.
const LATITUDE: [u8; 24] = [
    51, 0, 0, 0, 1, 0, 0, 0, 30, 0, 0, 0, 1, 0, 0, 0, 0x59, 0x0A, 0, 0, 100, 0, 0, 0,
];
const CAMERA: &[u8] = b"SecretCam-9000";
/// Not a real profile, and need not be: the decoders carry it as bytes.
const ICC: &[u8] = b"\0\0\x02\x0cTEST-ICC-PROFILE-BYTES-0123456789abcdef";

fn sha(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A TIFF/EXIF block: orientation, the camera's make, and a GPS IFD.
fn exif(orientation: u16) -> Vec<u8> {
    let mut t = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes());
    // IFD0 at 8: three entries.
    let ifd0_len = 2 + 3 * 12 + 4;
    let make_at = 8 + ifd0_len;
    let gps_at = make_at + CAMERA.len() + 1;
    t.extend_from_slice(&3u16.to_le_bytes());
    let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&kind.to_le_bytes());
        t.extend_from_slice(&count.to_le_bytes());
        t.extend_from_slice(&value.to_le_bytes());
    };
    entry(&mut t, 0x010F, 2, CAMERA.len() as u32 + 1, make_at as u32);
    entry(&mut t, 0x0112, 3, 1, u32::from(orientation));
    entry(&mut t, 0x8825, 4, 1, gps_at as u32);
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(CAMERA);
    t.push(0);
    // GPS IFD: latitude ref and latitude.
    let lat_at = gps_at + 2 + 2 * 12 + 4;
    t.extend_from_slice(&2u16.to_le_bytes());
    entry(&mut t, 0x0001, 2, 2, u32::from_le_bytes(*b"N\0\0\0"));
    entry(&mut t, 0x0002, 5, 3, lat_at as u32);
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&LATITUDE);
    t
}

/// A two-colour image, left half red and right half blue, so a rotation shows.
fn halves(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, _| {
        if x < w / 2 {
            Rgb([220, 20, 30])
        } else {
            Rgb([20, 40, 210])
        }
    })
}

fn jpeg_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![0xFF, marker];
    s.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
    s.extend_from_slice(payload);
    s
}

/// A JPEG carrying EXIF (with GPS), XMP, a comment, an ICC profile, and
/// HTML appended after its end.
fn hostile_jpeg(orientation: u16) -> Vec<u8> {
    let mut plain = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut plain, 90);
    enc.set_icc_profile(ICC.to_vec()).unwrap();
    DynamicImage::ImageRgb8(halves(32, 16))
        .write_with_encoder(enc)
        .unwrap();
    let mut out = plain[..2].to_vec();
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(exif(orientation));
    out.extend(jpeg_segment(0xE1, &app1));
    out.extend(jpeg_segment(
        0xE1,
        b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>SecretCam-9000</x:xmpmeta>",
    ));
    out.extend(jpeg_segment(0xFE, b"<script>alert(1)</script>"));
    out.extend_from_slice(&plain[2..]);
    out.extend_from_slice(b"<html><script>alert(2)</script></html>");
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(data);
    c.extend_from_slice(&crc32(&crc_in).to_be_bytes());
    c
}

fn png_of(img: &DynamicImage, icc: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = PngEncoder::new(&mut out);
    if icc {
        enc.set_icc_profile(ICC.to_vec()).unwrap();
    }
    img.write_with_encoder(enc).unwrap();
    out
}

/// A PNG carrying text, XMP, EXIF (with GPS and `orientation`), a private
/// chunk, an ICC profile, and a ZIP-like tail after IEND.
fn hostile_png(orientation: u16) -> Vec<u8> {
    let plain = png_of(&DynamicImage::ImageRgb8(halves(32, 16)), true);
    let ihdr_end = 8 + 25;
    let mut out = plain[..ihdr_end].to_vec();
    out.extend(png_chunk(b"tEXt", b"Comment\0<script>alert(1)</script>"));
    out.extend(png_chunk(
        b"iTXt",
        b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta>SecretCam-9000</x:xmpmeta>",
    ));
    out.extend(png_chunk(b"eXIf", &exif(orientation)));
    out.extend(png_chunk(b"prVt", b"private data"));
    out.extend_from_slice(&plain[ihdr_end..]);
    out.extend_from_slice(b"PK\x03\x04appended archive");
    out
}

fn decode(bytes: &[u8]) -> DynamicImage {
    image::load_from_memory(bytes).expect("the stored image decodes")
}

fn icc_of(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::ImageDecoder;
    let mut d = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap()
        .into_decoder()
        .unwrap();
    d.icc_profile().unwrap()
}

fn assert_no_metadata(bytes: &[u8]) {
    for needle in [
        &LATITUDE[..],
        CAMERA,
        b"Exif\0\0",
        b"xmpmeta",
        b"<script>",
        b"<html>",
        b"PK\x03\x04",
    ] {
        assert!(
            !contains(bytes, needle),
            "the stored file still carries {:?}",
            String::from_utf8_lossy(needle)
        );
    }
}

#[test]
fn a_jpeg_is_stored_without_exif_xmp_location_comment_or_tail_and_keeps_its_profile() {
    let input = hostile_jpeg(1);
    assert!(contains(&input, &LATITUDE) && contains(&input, CAMERA));
    let out = process(input, &LIMITS).unwrap();
    assert_eq!(out.media_type, MediaType::Jpeg);
    assert_no_metadata(&out.bytes);
    assert!(out.bytes.ends_with(&[0xFF, 0xD9]), "nothing after EOI");
    assert_eq!(icc_of(&out.bytes).as_deref(), Some(ICC));
    assert_eq!((out.width, out.height), (Some(32), Some(16)));
}

#[test]
fn a_png_is_stored_without_text_exif_private_chunks_or_tail_and_keeps_its_profile() {
    let out = process(hostile_png(1), &LIMITS).unwrap();
    assert_eq!(out.media_type, MediaType::Png);
    assert_no_metadata(&out.bytes);
    assert!(!contains(&out.bytes, b"prVt") && !contains(&out.bytes, b"eXIf"));
    assert_eq!(icc_of(&out.bytes).as_deref(), Some(ICC));
    // Lossless: the pixels are exactly the original's.
    assert_eq!(decode(&out.bytes).to_rgb8(), halves(32, 16));
}

/// EXIF orientation 6 is "rotate 90 degrees clockwise to display". The stored
/// image is the rotated pixels, with no tag left to rotate them again.
#[test]
fn a_rotated_photo_is_stored_upright_and_untagged() {
    let upright = image::imageops::rotate90(&halves(32, 16));
    // PNG: lossless, so exact.
    let png = process(hostile_png(6), &LIMITS).unwrap();
    assert_eq!((png.width, png.height), (Some(16), Some(32)));
    assert_eq!(decode(&png.bytes).to_rgb8(), upright);
    assert_eq!(icc_of(&png.bytes).as_deref(), Some(ICC));
    // JPEG: re-encoded once, so compared by colour.
    let jpg = process(hostile_jpeg(6), &LIMITS).unwrap();
    assert_no_metadata(&jpg.bytes);
    assert_eq!((jpg.width, jpg.height), (Some(16), Some(32)));
    let got = decode(&jpg.bytes).to_rgb8();
    let near = |a: &Rgb<u8>, b: &Rgb<u8>| a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) < 24);
    assert!(
        near(got.get_pixel(8, 4), upright.get_pixel(8, 4)),
        "top: red"
    );
    assert!(
        near(got.get_pixel(8, 28), upright.get_pixel(8, 28)),
        "bottom: blue"
    );
    assert_eq!(icc_of(&jpg.bytes).as_deref(), Some(ICC));
}

fn gif_frames(n: usize, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        for i in 0..n {
            let img = RgbaImage::from_fn(w, h, |x, y| {
                Rgba([(x * 7 + i as u32 * 31) as u8, (y * 5) as u8, 90, 255])
            });
            enc.encode_frame(Frame::new(img)).unwrap();
        }
    }
    out
}

/// A GIF carrying a comment and an XMP application extension before the
/// trailer, and HTML after it.
fn hostile_gif() -> Vec<u8> {
    let mut g = gif_frames(3, 12, 10);
    assert_eq!(g.pop(), Some(0x3B));
    g.extend_from_slice(&[0x21, 0xFE, 8]);
    g.extend_from_slice(b"<script>");
    g.push(0);
    g.extend_from_slice(&[0x21, 0xFF, 11]);
    g.extend_from_slice(b"XMP DataXMP");
    g.push(14);
    g.extend_from_slice(b"SecretCam-9000");
    g.push(0);
    g.push(0x3B);
    g.extend_from_slice(b"<html>");
    g
}

fn webp_lossless(img: &DynamicImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_with_encoder(WebPEncoder::new_lossless(&mut out))
        .unwrap();
    out
}

/// A WebP in the extended format, declaring and carrying EXIF and XMP.
fn hostile_webp() -> Vec<u8> {
    let simple = webp_lossless(&DynamicImage::ImageRgba8(RgbaImage::from_pixel(
        6,
        4,
        Rgba([1, 2, 3, 255]),
    )));
    let bitstream = &simple[12..];
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut c = kind.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    // VP8X: EXIF (0x08) and XMP (0x04) flags, canvas 6x4 (stored minus one).
    let vp8x = [0x08 | 0x04, 0, 0, 0, 5, 0, 0, 3, 0, 0];
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend_from_slice(bitstream);
    body.extend(chunk(b"EXIF", &exif(1)));
    body.extend(chunk(b"XMP ", b"<x:xmpmeta>SecretCam-9000</x:xmpmeta>"));
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out.extend_from_slice(b"<html>");
    out
}

#[test]
fn a_gif_and_a_webp_are_stored_without_their_metadata() {
    let gif = process(hostile_gif(), &LIMITS).unwrap();
    assert_eq!(gif.media_type, MediaType::Gif);
    assert_no_metadata(&gif.bytes);
    assert!(contains(&gif.bytes, b"NETSCAPE2.0"), "the loop count stays");
    let webp = process(hostile_webp(), &LIMITS).unwrap();
    assert_eq!(webp.media_type, MediaType::Webp);
    assert_no_metadata(&webp.bytes);
    assert_eq!(webp.bytes[20] & (0x08 | 0x04), 0, "flags cleared");
    assert_eq!((webp.width, webp.height), (Some(6), Some(4)));
}

/// D3a: processing a processed file returns identical bytes, for every kept
/// type, including the re-encoded (rotated) ones.
#[test]
fn processing_a_processed_file_returns_identical_bytes_for_every_allowed_format() {
    let inputs = [
        ("jpeg", hostile_jpeg(1)),
        ("jpeg rotated", hostile_jpeg(6)),
        ("png", hostile_png(1)),
        ("png rotated", hostile_png(8)),
        ("gif", hostile_gif()),
        ("webp", hostile_webp()),
        ("svg", hostile_svg().into_bytes()),
        ("pdf", minimal_pdf()),
        ("other", b"PK\x03\x04 an archive".to_vec()),
    ];
    for (what, input) in inputs {
        let once = process(input, &LIMITS).unwrap();
        let twice = process(once.bytes.clone(), &LIMITS).unwrap();
        assert_eq!(once, twice, "{what} is not a fixed point");
    }
}

#[test]
fn an_image_whose_header_declares_more_than_the_pixel_limit_is_refused_without_decoding() {
    // IHDR declares 50000 x 50000; the IDAT is not even valid zlib, so any
    // decode would fail as undecodable instead.
    let mut ihdr = 50_000u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&50_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(png_chunk(b"IHDR", &ihdr));
    png.extend(png_chunk(b"IDAT", b"not zlib"));
    png.extend(png_chunk(b"IEND", b""));
    assert!(matches!(
        process(png, &LIMITS),
        Err(ProcessError::TooManyPixels(_))
    ));
    // And the bound is the configured one, not a constant.
    let small = ProcessLimits {
        max_pixels: 32 * 16 - 1,
        ..LIMITS
    };
    let input = png_of(&DynamicImage::ImageRgb8(halves(32, 16)), false);
    assert!(matches!(
        process(input.clone(), &small),
        Err(ProcessError::TooManyPixels(_))
    ));
    let exact = ProcessLimits {
        max_pixels: 32 * 16,
        ..LIMITS
    };
    assert!(process(input, &exact).is_ok());
}

/// A decompression bomb as animation: every frame is small, the total is not.
#[test]
fn an_animation_over_its_frame_or_pixel_budget_is_refused() {
    let gif = gif_frames(9, 10, 10);
    let per_frame = ProcessLimits {
        max_pixels: 200,
        max_frames: 1000,
    };
    // Budget: 4 x 200 = 800 pixels = 8 frames of 100. Nine is one too many.
    assert!(matches!(
        process(gif.clone(), &per_frame),
        Err(ProcessError::TooManyPixels(_))
    ));
    assert!(process(gif_frames(8, 10, 10), &per_frame).is_ok());
    let few_frames = ProcessLimits {
        max_frames: 8,
        ..LIMITS
    };
    assert!(matches!(
        process(gif, &few_frames),
        Err(ProcessError::TooManyPixels(_))
    ));
}

#[test]
fn a_malformed_image_is_refused_as_undecodable() {
    let png = png_of(&DynamicImage::ImageRgb8(halves(64, 64)), false);
    let jpg = hostile_jpeg(1);
    let cases = [
        ("truncated png", png[..png.len() / 2].to_vec()),
        ("truncated jpeg", jpg[..jpg.len() / 2].to_vec()),
        (
            "gif header only",
            b"GIF89a\x0a\x00\x0a\x00\x00\x00\x00".to_vec(),
        ),
        ("webp header only", b"RIFF\x04\x00\x00\x00WEBP".to_vec()),
    ];
    for (what, bytes) in cases {
        assert!(
            matches!(process(bytes, &LIMITS), Err(ProcessError::Undecodable(_))),
            "{what}"
        );
    }
}

#[test]
fn the_type_comes_from_the_bytes_never_the_claim() {
    // HTML, whatever it is called, is a download.
    let html = process(b"<html><script>alert(1)</script></html>".to_vec(), &LIMITS).unwrap();
    assert_eq!(html.media_type, MediaType::Other);
    // An SVG with a DTD is not parsed (no entity expansion): a download too.
    let dtd = br#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaa">]><svg xmlns="http://www.w3.org/2000/svg">&a;</svg>"#;
    assert_eq!(
        process(dtd.to_vec(), &LIMITS).unwrap().media_type,
        MediaType::Other
    );
    // AVIF is recognised and refused: it cannot be processed here.
    let avif = b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf".to_vec();
    assert!(matches!(
        process(avif, &LIMITS),
        Err(ProcessError::Unsupported(_))
    ));
    // A PNG signature in front of HTML is a PNG to sniff, and refused as one.
    let fake = [&b"\x89PNG\r\n\x1a\n"[..], b"<html><script>"].concat();
    assert!(process(fake, &LIMITS).is_err());
}

fn hostile_svg() -> String {
    r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10" onload="alert(1)">
  <!-- a comment -->
  <script>alert(2)</script>
  <style>@import url(https://evil.example/x.css);</style>
  <defs><linearGradient id="g"><stop offset="0" stop-color="red"/></linearGradient></defs>
  <rect width="5" height="5" fill="url(#g)" onclick="alert(3)" style="fill:url(https://evil.example/p)"/>
  <circle r="2" fill="url(https://evil.example/pattern)"/>
  <use xlink:href="https://evil.example/sprite.svg#a"/>
  <use href="#g"/>
  <image href="https://evil.example/track.png" width="1" height="1"/>
  <foreignObject><div xmlns="http://www.w3.org/1999/xhtml">x</div></foreignObject>
  <a href="javascript:alert(4)"><text>link</text></a>
  <text x="1" y="9">A &amp; B &lt; C</text>
</svg>"##
        .to_string()
}

#[test]
fn an_svg_is_stored_without_script_handlers_or_external_references() {
    let out = process(hostile_svg().into_bytes(), &LIMITS).unwrap();
    assert_eq!(out.media_type, MediaType::Svg);
    let text = String::from_utf8(out.bytes).unwrap();
    for gone in [
        "script",
        "onload",
        "onclick",
        "style",
        "evil.example",
        "foreignObject",
        "<image",
        "javascript",
        "<a ",
        "@import",
    ] {
        assert!(!text.contains(gone), "{gone} survived: {text}");
    }
    for kept in [
        r#"fill="url(#g)""#,
        r##"<use href="#g"/>"##,
        "A &amp; B &lt; C",
        "<linearGradient",
    ] {
        assert!(text.contains(kept), "{kept} was lost: {text}");
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(&dir.path().join("blobs")).unwrap();
    (dir, s)
}

fn put(
    s: &BlobStore,
    kb: &str,
    bytes: &[u8],
    quota: Option<&StorageQuota>,
) -> Result<BlobMeta, BlobError> {
    let staged = s.stage(bytes)?;
    s.commit(kb, staged, (MediaType::Other, None, None), quota)
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Blob files (not the database, not `.tmp`).
fn blob_files(s: &BlobStore) -> Vec<PathBuf> {
    files_under(s.root())
        .into_iter()
        .filter(|p| {
            !p.starts_with(s.root().join(TMP_DIR))
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(DB_FILE)
        })
        .collect()
}

#[test]
fn the_address_is_the_hash_of_the_stored_bytes() {
    let (_d, s) = store();
    // What a client sends is not what is stored: the server's processing picks
    // the address.
    let sent = hostile_png(1);
    let processed = process(sent.clone(), &LIMITS).unwrap();
    let staged = s.stage(&processed.bytes).unwrap();
    let meta = s
        .commit(
            "kb-a",
            staged,
            (processed.media_type, processed.width, processed.height),
            None,
        )
        .unwrap();
    let stored = std::fs::read(s.blob_path("kb-a", &meta.sha256)).unwrap();
    assert_eq!(meta.sha256, sha(&stored));
    assert_ne!(meta.sha256, sha(&sent));
    assert_eq!(meta.size, stored.len() as u64);
    assert_eq!(s.get("kb-a", &meta.sha256).unwrap(), Some(meta));
}

#[test]
fn the_same_bytes_in_two_kbs_are_two_blobs_and_removing_one_keeps_the_other() {
    let (_d, s) = store();
    let a = put(&s, "kb-a", b"shared bytes", None).unwrap();
    let b = put(&s, "kb-b", b"shared bytes", None).unwrap();
    assert_eq!(a.sha256, b.sha256);
    assert_eq!(blob_files(&s).len(), 2, "one file per KB");
    let guard = s.binding_lock("kb-a");
    s.remove_unreferenced(&guard, "kb-a", &a.sha256).unwrap();
    assert_eq!(s.get("kb-a", &a.sha256).unwrap(), None);
    assert!(s.get("kb-b", &b.sha256).unwrap().is_some());
    assert_eq!(
        std::fs::read(s.blob_path("kb-b", &b.sha256)).unwrap(),
        b"shared bytes"
    );
}

#[test]
fn a_deduplicated_upload_adds_no_file_and_no_charge() {
    let (_d, s) = store();
    let q = StorageQuota {
        tenant: "team".into(),
        max_bytes: 100,
    };
    let first = put(&s, "kb-a", &[7u8; 40], Some(&q)).unwrap();
    let again = put(&s, "kb-a", &[7u8; 40], Some(&q)).unwrap();
    assert_eq!(first, again);
    assert_eq!(s.usage("team").unwrap(), 40);
    assert_eq!(blob_files(&s).len(), 1);
}

#[test]
fn the_quota_refuses_the_upload_that_would_exceed_it_and_not_the_one_before() {
    let (_d, s) = store();
    let q = StorageQuota {
        tenant: "team".into(),
        max_bytes: 80,
    };
    put(&s, "kb-a", &[1u8; 40], Some(&q)).unwrap();
    // Exactly at the quota: admitted.
    put(&s, "kb-b", &[2u8; 40], Some(&q)).unwrap();
    assert_eq!(s.usage("team").unwrap(), 80);
    let refused = put(&s, "kb-a", &[3u8; 1], Some(&q));
    assert!(
        matches!(
            refused,
            Err(BlobError::TenantFull {
                used: 80,
                size: 1,
                limit: 80
            })
        ),
        "{refused:?}"
    );
    assert_eq!(blob_files(&s).len(), 2, "nothing stored for the refusal");
    // Another tenant is not charged for this one's bytes.
    let other = StorageQuota {
        tenant: "other".into(),
        max_bytes: 10,
    };
    put(&s, "kb-a", &[4u8; 10], Some(&other)).unwrap();
}

/// N concurrent uploads of different bytes against room for two: exactly two
/// are stored, whatever the interleaving.
#[test]
fn concurrent_uploads_cannot_jointly_overshoot_the_quota() {
    for round in 0..8u8 {
        let (_d, s) = store();
        let s = Arc::new(s);
        let q = StorageQuota {
            tenant: "team".into(),
            max_bytes: 100,
        };
        let threads: Vec<_> = (0..5u8)
            .map(|i| {
                let (s, q) = (Arc::clone(&s), q.clone());
                std::thread::spawn(move || {
                    let kb = if i % 2 == 0 { "kb-a" } else { "kb-b" };
                    put(&s, kb, &[i ^ round; 45], Some(&q)).is_ok()
                })
            })
            .collect();
        let stored = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(stored, 2, "round {round}");
        assert_eq!(s.usage("team").unwrap(), 90);
        assert_eq!(blob_files(&s).len(), 2);
    }
}

#[test]
fn concurrent_identical_uploads_both_succeed_and_leave_one_blob() {
    let (_d, s) = store();
    let s = Arc::new(s);
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let s = Arc::clone(&s);
            std::thread::spawn(move || put(&s, "kb-a", b"same bytes", None).unwrap())
        })
        .collect();
    let metas: Vec<BlobMeta> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(metas.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(blob_files(&s).len(), 1);
    assert_eq!(s.list_all().unwrap().len(), 1);
}

/// A crash at each step of a commit, through the seam: after a restart no row
/// names a missing file, nothing unrowed is counted, nothing is left in
/// `.tmp`, and the same upload then succeeds.
#[test]
fn a_crash_at_each_commit_step_leaves_no_dangling_row_and_no_counted_orphan() {
    let q = StorageQuota {
        tenant: "team".into(),
        max_bytes: 1000,
    };
    for step in [
        CommitStep::BeforeFsync,
        CommitStep::BeforeRename,
        CommitStep::BeforeRow,
        CommitStep::AfterRow,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("blobs");
        {
            let s = BlobStore::open(&root)
                .unwrap()
                .with_fault_hook(Arc::new(move |at| at == step));
            let got = put(&s, "kb-a", b"the bytes", Some(&q));
            assert!(
                matches!(got, Err(BlobError::Interrupted(at)) if at == step),
                "{step:?}: {got:?}"
            );
        }
        let s = BlobStore::open(&root).unwrap();
        for (kb, sha) in s.list_all().unwrap() {
            assert!(
                s.blob_path(&kb, &sha).is_file(),
                "{step:?}: a row names a missing file"
            );
        }
        let rowed: u64 = s.list_all().unwrap().len() as u64 * 9;
        assert_eq!(
            s.usage("team").unwrap(),
            rowed,
            "{step:?}: usage is rows only"
        );
        assert!(
            files_under(&root.join(TMP_DIR)).is_empty(),
            "{step:?}: .tmp is cleared at open"
        );
        let meta = put(&s, "kb-a", b"the bytes", Some(&q)).unwrap();
        assert_eq!(
            std::fs::read(s.blob_path("kb-a", &meta.sha256)).unwrap(),
            b"the bytes"
        );
        assert_eq!(s.usage("team").unwrap(), 9, "{step:?}: charged once");
    }
}

/// Removing a blob drops the row before the file: a crash between them leaves
/// an unrowed file, never a row without its file.
#[test]
fn removal_drops_the_row_before_the_file() {
    let (_d, s) = store();
    let m = put(&s, "kb-a", b"to remove", None).unwrap();
    let path = s.blob_path("kb-a", &m.sha256);
    // Make the file undeletable by replacing it with a non-empty directory:
    // the row must already be gone when the file removal fails.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(path.join("x")).unwrap();
    let guard = s.binding_lock("kb-a");
    assert!(s.remove_unreferenced(&guard, "kb-a", &m.sha256).is_err());
    assert_eq!(s.get("kb-a", &m.sha256).unwrap(), None);
}

#[tokio::test]
async fn receiving_stops_at_the_cap_and_writes_nothing_past_it() {
    let (_d, s) = store();
    let mut r = Receiving::new(&s, 100).unwrap();
    r.write(&[1u8; 60]).await.unwrap();
    assert_eq!(
        r.write(&[1u8; 41]).await,
        Err(ReceiveError::OverCap { cap: 100 })
    );
    assert_eq!(r.len(), 60);
    r.write(&[1u8; 40]).await.unwrap();
    let got = r.finish().await.unwrap();
    assert_eq!(got.len, 100);
    assert_eq!(std::fs::metadata(got.path.path()).unwrap().len(), 100);
}

#[tokio::test]
async fn an_interrupted_receive_leaves_nothing_and_a_restart_clears_a_crashed_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("blobs");
    let s = BlobStore::open(&root).unwrap();
    let mut r = Receiving::new(&s, 1000).unwrap();
    r.write(b"partial").await.unwrap();
    drop(r);
    assert!(
        files_under(&root.join(TMP_DIR)).is_empty(),
        "dropped: removed"
    );
    let mut crashed = Receiving::new(&s, 1000).unwrap();
    crashed.write(b"partial").await.unwrap();
    std::mem::forget(crashed);
    assert_eq!(files_under(&root.join(TMP_DIR)).len(), 1);
    drop(s);
    let s = BlobStore::open(&root).unwrap();
    assert!(
        files_under(&root.join(TMP_DIR)).is_empty(),
        "restart: removed"
    );
    assert!(s.list_all().unwrap().is_empty());
}

#[test]
fn addresses_and_kb_ids_are_validated_before_any_path_is_built() {
    assert!(valid_sha256(&sha(b"x")));
    for bad in [
        "",
        "../../etc/passwd",
        &sha(b"x").to_uppercase(),
        &sha(b"x")[..63],
        &format!("{}0", sha(b"x")),
    ] {
        assert!(!valid_sha256(bad), "{bad}");
    }
    for bad in [".tmp", "blobs.db", "..", "KB-A", "kb/a", ""] {
        assert!(!valid_kb_id(bad), "{bad}");
    }
}

/// A real, minimal one-page PDF, with the binary marker line writers emit.
pub(crate) fn minimal_pdf() -> Vec<u8> {
    let mut pdf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] >>",
    ];
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").bytes());
    }
    pdf.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    pdf
}

/// A PDF is stored byte for byte: none of the image rules touch it, and its
/// address is the hash of exactly what was sent.
#[test]
fn a_pdf_is_stored_byte_identical_and_addressed_by_its_own_bytes() {
    let pdf = minimal_pdf();
    let out = process(pdf.clone(), &LIMITS).unwrap();
    assert_eq!(out.media_type, MediaType::Pdf);
    assert_eq!(out.media_type.as_str(), "application/pdf");
    assert!(!out.media_type.inline(), "a PDF is only ever a download");
    assert_eq!(out.bytes, pdf);
    assert_eq!((out.width, out.height), (None, None));
    let (_d, s) = store();
    let staged = s.stage(&out.bytes).unwrap();
    let meta = s
        .commit("kb-a", staged, (out.media_type, None, None), None)
        .unwrap();
    assert_eq!(meta.sha256, sha(&pdf));
    assert_eq!(
        std::fs::read(s.blob_path("kb-a", &meta.sha256)).unwrap(),
        pdf
    );
    assert_eq!(
        s.get("kb-a", &meta.sha256).unwrap().unwrap().media_type,
        MediaType::Pdf
    );
}

/// Only the bytes say what a file is: an image is an image whatever follows
/// it, a PDF header after anything else does not make a PDF, and an image
/// header inside a PDF does not make an image.
#[test]
fn pdf_and_image_are_told_apart_by_their_bytes_alone() {
    // An image with a PDF appended: an image, stripped of the tail.
    let png = png_of(&DynamicImage::ImageRgb8(halves(8, 8)), false);
    let disguised = [&png[..], &minimal_pdf()[..]].concat();
    let out = process(disguised, &LIMITS).unwrap();
    assert_eq!(out.media_type, MediaType::Png);
    assert!(!contains(&out.bytes, b"%PDF-"));
    // A PDF carrying a PNG inside: a PDF, untouched.
    let pdf_with_image = [&minimal_pdf()[..], &png[..]].concat();
    let out = process(pdf_with_image.clone(), &LIMITS).unwrap();
    assert_eq!(
        (out.media_type, out.bytes),
        (MediaType::Pdf, pdf_with_image)
    );
    // Not a PDF, whatever it claims to be: HTML with a late PDF header is a
    // download of unknown type, never `application/pdf`.
    let fake = b"<html><script>alert(1)</script></html>\n%PDF-1.4\n".to_vec();
    assert_eq!(process(fake, &LIMITS).unwrap().media_type, MediaType::Other);
    assert_eq!(
        super::sniff::sniff(b"%PDF"),
        MediaType::Other,
        "no version dash"
    );
}
