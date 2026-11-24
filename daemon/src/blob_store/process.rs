//! ADR-115 D3a: what the daemon stores is what it has checked. An upload is
//! typed by sniffing, then:
//!
//! - a raster image (PNG, JPEG, GIF, WebP) has its declared size checked
//!   against the pixel bound BEFORE anything is decoded; an orientation tag is
//!   applied to the pixels (the one re-encode, after which no tag is left);
//!   its metadata is stripped at the container level ([`super::strip`]); and
//!   the stripped result is decoded in full under a memory bound and, for an
//!   animation, a frame and pixel budget. The stored bytes are the ones decoded.
//! - an SVG is sanitised ([`super::svg`]).
//! - AVIF is refused: its only decoders are C, and metadata cannot be stripped
//!   from what is not read (see the module docs of [`super`]).
//! - a PDF (`%PDF-` at offset 0) is stored exactly as it arrived, as
//!   `application/pdf`: the image rules do not apply, and its hash is of the
//!   bytes the client sent.
//! - anything else is stored as it arrived, as `application/octet-stream`.
//!
//! Processing is idempotent: `process(process(x)) == process(x)`, for every
//! kept type. The tests hold every format to it.
//!
//! Pure and synchronous: the caller runs it on a bounded worker, never on the
//! request task.

use std::io::Cursor;

use image::codecs::gif::GifDecoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{PngDecoder, PngEncoder};
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::metadata::Orientation;
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
};

use super::sniff::{sniff, MediaType};
use super::{strip, svg};

/// The widest buffer these decoders produce per pixel: RGBA at 16 bits a
/// channel. The decode's memory bound is the pixel bound times this, so an
/// image under the pixel bound always fits and nothing larger can be reserved.
const BYTES_PER_PIXEL_BOUND: u64 = 8;

/// An animation's frames may decode, together, this many times the
/// single-image pixel bound. Each frame is checked against the budget before
/// it is decoded, so a decompression bomb stops at the bound.
pub const ANIMATION_PIXEL_FACTOR: u64 = 4;

/// The quality a rotated JPEG is re-encoded at: the only lossy step, applied
/// once, since the result carries no orientation tag to apply again.
const ROTATED_JPEG_QUALITY: u8 = 90;

/// The bounds a decode runs under.
#[derive(Debug, Clone, Copy)]
pub struct ProcessLimits {
    /// `max_attachment_pixels`: width times height of one image, read from the
    /// header and checked before decoding.
    pub max_pixels: u64,
    /// Frames an animation may have.
    pub max_frames: u32,
}

impl ProcessLimits {
    fn decode_limits(&self) -> Limits {
        let mut l = Limits::default();
        l.max_alloc = Some(self.max_pixels.saturating_mul(BYTES_PER_PIXEL_BOUND));
        l
    }

    fn animation_budget(&self) -> u64 {
        self.max_pixels.saturating_mul(ANIMATION_PIXEL_FACTOR)
    }
}

/// The processed file: the bytes to hash and store, and what they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processed {
    pub bytes: Vec<u8>,
    pub media_type: MediaType,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Why an upload was refused (D3a), each with its own refusal kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    /// `attachment_too_many_pixels`: over the pixel bound, or an animation
    /// over its frame or pixel budget.
    TooManyPixels(String),
    /// `attachment_undecodable`: malformed, or failed its decode.
    Undecodable(String),
    /// `attachment_unsupported`: a type the daemon cannot process.
    Unsupported(String),
}

/// Process one upload. See the module docs.
pub fn process(input: Vec<u8>, limits: &ProcessLimits) -> Result<Processed, ProcessError> {
    match sniff(&input) {
        t @ (MediaType::Png | MediaType::Jpeg | MediaType::Gif | MediaType::Webp) => {
            raster(input, t, limits)
        }
        MediaType::Avif => Err(ProcessError::Unsupported(
            "AVIF images cannot be processed on this server".into(),
        )),
        MediaType::Svg => svg::sanitize(&input)
            .map(|bytes| Processed {
                bytes,
                media_type: MediaType::Svg,
                width: None,
                height: None,
            })
            .map_err(ProcessError::Undecodable),
        t @ (MediaType::Pdf | MediaType::Other) => Ok(Processed {
            bytes: input,
            media_type: t,
            width: None,
            height: None,
        }),
    }
}

fn format_of(t: MediaType) -> ImageFormat {
    match t {
        MediaType::Png => ImageFormat::Png,
        MediaType::Jpeg => ImageFormat::Jpeg,
        MediaType::Gif => ImageFormat::Gif,
        _ => ImageFormat::WebP,
    }
}

fn undecodable(e: impl std::fmt::Display) -> ProcessError {
    ProcessError::Undecodable(e.to_string())
}

fn check_pixels(w: u32, h: u32, limits: &ProcessLimits) -> Result<(), ProcessError> {
    let pixels = u64::from(w) * u64::from(h);
    if pixels > limits.max_pixels {
        return Err(ProcessError::TooManyPixels(format!(
            "{w}x{h} is {pixels} pixels, over the {} limit",
            limits.max_pixels
        )));
    }
    Ok(())
}

/// What the header says, read without decoding the pixels.
struct Header {
    orientation: Orientation,
    icc: Option<Vec<u8>>,
}

fn raster(input: Vec<u8>, t: MediaType, limits: &ProcessLimits) -> Result<Processed, ProcessError> {
    let header = read_header(&input, t, limits)?;
    let oriented = if header.orientation == Orientation::NoTransforms {
        input
    } else {
        if is_animated(&input, t)? {
            return Err(ProcessError::Unsupported(
                "an animation with an orientation tag".into(),
            ));
        }
        reorient(&input, t, header, limits)?
    };
    let stripped = match t {
        MediaType::Png => strip::png(&oriented),
        MediaType::Jpeg => strip::jpeg(&oriented),
        MediaType::Gif => strip::gif(&oriented),
        _ => strip::webp(&oriented),
    }
    .ok_or_else(|| ProcessError::Undecodable("malformed container".into()))?;
    let (w, h) = validate(&stripped, t, limits)?;
    Ok(Processed {
        bytes: stripped,
        media_type: t,
        width: Some(w),
        height: Some(h),
    })
}

fn decoder<'a>(
    input: &'a [u8],
    t: MediaType,
    limits: &ProcessLimits,
) -> Result<impl ImageDecoder + 'a, ProcessError> {
    let mut reader = ImageReader::with_format(Cursor::new(input), format_of(t));
    reader.limits(limits.decode_limits());
    reader.into_decoder().map_err(undecodable)
}

/// The header's size (checked against the bound), orientation and colour
/// profile. No pixel is decoded.
fn read_header(input: &[u8], t: MediaType, limits: &ProcessLimits) -> Result<Header, ProcessError> {
    let mut dec = decoder(input, t, limits)?;
    let (w, h) = dec.dimensions();
    check_pixels(w, h, limits)?;
    Ok(Header {
        // GIF has no orientation; a tag that cannot be read is not applied.
        orientation: dec.orientation().unwrap_or(Orientation::NoTransforms),
        icc: dec.icc_profile().ok().flatten(),
    })
}

fn is_animated(input: &[u8], t: MediaType) -> Result<bool, ProcessError> {
    let cur = Cursor::new(input);
    Ok(match t {
        MediaType::Png => PngDecoder::new(cur)
            .and_then(|d| d.is_apng())
            .map_err(undecodable)?,
        MediaType::Webp => WebPDecoder::new(cur).map_err(undecodable)?.has_animation(),
        _ => false,
    })
}

/// Decode, rotate or flip the pixels as the tag says, and encode again in the
/// same format with the same colour profile and no tag.
fn reorient(
    input: &[u8],
    t: MediaType,
    header: Header,
    limits: &ProcessLimits,
) -> Result<Vec<u8>, ProcessError> {
    let mut img = DynamicImage::from_decoder(decoder(input, t, limits)?).map_err(undecodable)?;
    img.apply_orientation(header.orientation);
    let mut out = Vec::new();
    let result = match t {
        MediaType::Jpeg => {
            let img = match img {
                DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) => img,
                other => DynamicImage::ImageRgb8(other.to_rgb8()),
            };
            let mut enc = JpegEncoder::new_with_quality(&mut out, ROTATED_JPEG_QUALITY);
            with_icc(&mut enc, header.icc);
            img.write_with_encoder(enc)
        }
        MediaType::Png => {
            let mut enc = PngEncoder::new(&mut out);
            with_icc(&mut enc, header.icc);
            img.write_with_encoder(enc)
        }
        _ => {
            let img = DynamicImage::ImageRgba8(img.to_rgba8());
            let mut enc = WebPEncoder::new_lossless(&mut out);
            with_icc(&mut enc, header.icc);
            img.write_with_encoder(enc)
        }
    };
    result.map_err(undecodable)?;
    Ok(out)
}

fn with_icc(enc: &mut impl ImageEncoder, icc: Option<Vec<u8>>) {
    if let Some(icc) = icc {
        // An encoder that cannot carry a profile drops it; the four used here can.
        let _ = enc.set_icc_profile(icc);
    }
}

/// Decode `bytes` in full under the bounds; its size on success.
fn validate(
    bytes: &[u8],
    t: MediaType,
    limits: &ProcessLimits,
) -> Result<(u32, u32), ProcessError> {
    let cur = || Cursor::new(bytes);
    match t {
        MediaType::Gif => {
            let mut d = GifDecoder::new(cur()).map_err(undecodable)?;
            d.set_limits(limits.decode_limits()).map_err(undecodable)?;
            let size = d.dimensions();
            frames(d, size, limits)
        }
        MediaType::Png => {
            let d = PngDecoder::with_limits(cur(), limits.decode_limits()).map_err(undecodable)?;
            let size = d.dimensions();
            if d.is_apng().map_err(undecodable)? {
                frames(d.apng().map_err(undecodable)?, size, limits)
            } else {
                single(d, limits)
            }
        }
        MediaType::Webp => {
            let mut d = WebPDecoder::new(cur()).map_err(undecodable)?;
            d.set_limits(limits.decode_limits()).map_err(undecodable)?;
            let size = d.dimensions();
            if d.has_animation() {
                frames(d, size, limits)
            } else {
                single(d, limits)
            }
        }
        _ => single(decoder(bytes, t, limits)?, limits),
    }
}

fn single(d: impl ImageDecoder, limits: &ProcessLimits) -> Result<(u32, u32), ProcessError> {
    let (w, h) = d.dimensions();
    check_pixels(w, h, limits)?;
    DynamicImage::from_decoder(d).map_err(undecodable)?;
    Ok((w, h))
}

/// Decode every frame, refusing BEFORE a frame that would take the animation
/// past its frame count or pixel budget.
fn frames<'a>(
    d: impl AnimationDecoder<'a>,
    (w, h): (u32, u32),
    limits: &ProcessLimits,
) -> Result<(u32, u32), ProcessError> {
    check_pixels(w, h, limits)?;
    let per_frame = u64::from(w) * u64::from(h);
    let mut iter = d.into_frames();
    let mut n: u64 = 0;
    loop {
        if n >= u64::from(limits.max_frames) || (n + 1) * per_frame > limits.animation_budget() {
            // Over budget only if another frame actually follows.
            return match iter.next() {
                None => Ok((w, h)),
                Some(_) => Err(ProcessError::TooManyPixels(format!(
                    "an animation over {n} frames of {w}x{h}"
                ))),
            };
        }
        match iter.next() {
            None => return Ok((w, h)),
            Some(f) => {
                f.map_err(undecodable)?;
                n += 1;
            }
        }
    }
}
