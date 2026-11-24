//! Metadata removal at the container level (ADR-115 D3a): each format's
//! segments, chunks or blocks are copied when they carry pixels or how to show
//! them (including an embedded colour profile), and dropped otherwise. Nothing
//! is re-encoded here, so a stripped file is a fixed point: stripping it again
//! returns the same bytes (the idempotence D3a requires).
//!
//! Each function stops at the format's own end marker, so anything appended
//! after it (the usual way to build a polyglot) is dropped too. `None`: the
//! container is malformed; the caller refuses the upload as undecodable.

/// JPEG: keeps the frame, tables and scans, an `ICC_PROFILE` APP2 and an
/// `Adobe` APP14 (which says how to read the colour components). Drops every
/// other APPn (EXIF, XMP, IPTC, MPF, maker notes, a JFIF thumbnail) and COM.
pub fn jpeg(input: &[u8]) -> Option<Vec<u8>> {
    if !input.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    loop {
        if *input.get(i)? != 0xFF {
            return None;
        }
        while input.get(i) == Some(&0xFF) {
            i += 1;
        }
        let marker = *input.get(i)?;
        i += 1;
        match marker {
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Some(out);
            }
            0x01 | 0xD0..=0xD7 => out.extend_from_slice(&[0xFF, marker]),
            _ => {
                let len = usize::from(u16::from_be_bytes([*input.get(i)?, *input.get(i + 1)?]));
                let segment = input.get(i..i + len).filter(|_| len >= 2)?;
                if keep_jpeg_segment(marker, &segment[2..]) {
                    out.extend_from_slice(&[0xFF, marker]);
                    out.extend_from_slice(segment);
                }
                i += len;
                if marker == 0xDA {
                    let end = scan_end(input, i)?;
                    out.extend_from_slice(&input[i..end]);
                    i = end;
                }
            }
        }
    }
}

fn keep_jpeg_segment(marker: u8, payload: &[u8]) -> bool {
    match marker {
        0xE2 => payload.starts_with(b"ICC_PROFILE\0"),
        0xEE => payload.starts_with(b"Adobe"),
        0xE0..=0xEF | 0xFE => false,
        _ => true,
    }
}

/// Where the entropy-coded data after a SOS segment ends: the first `0xFF`
/// that is neither a stuffed `0xFF 0x00` nor a restart marker.
fn scan_end(input: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    loop {
        let k = j + input.get(j..)?.iter().position(|&b| b == 0xFF)?;
        let next = *input.get(k + 1)?;
        if next == 0x00 || (0xD0..=0xD7).contains(&next) {
            j = k + 2;
        } else {
            return Some(k);
        }
    }
}

/// PNG chunks kept: the critical ones, transparency, colour (including
/// `iCCP` and the HDR colour chunks), background, histogram, pixel aspect, and
/// APNG's animation chunks. Text (`tEXt`, `zTXt`, `iTXt`, where XMP lives),
/// `eXIf`, `tIME` and every private or unknown chunk are dropped.
const PNG_KEEP: &[&[u8; 4]] = &[
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB",
    b"cICP", b"mDCV", b"mDCv", b"cLLI", b"cLLi", b"bKGD", b"hIST", b"pHYs", b"acTL", b"fcTL",
    b"fdAT",
];

/// PNG: the kept chunks, verbatim with their CRCs, up to and including IEND.
pub fn png(input: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !input.starts_with(SIG) {
        return None;
    }
    let mut out = SIG.to_vec();
    let mut i = SIG.len();
    loop {
        let len = u32::from_be_bytes(input.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind: &[u8; 4] = input.get(i + 4..i + 8)?.try_into().ok()?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = input.get(i..end)?;
        if PNG_KEEP.contains(&kind) {
            out.extend_from_slice(chunk);
        }
        if kind == b"IEND" {
            return Some(out);
        }
        i = end;
    }
}

/// GIF application extensions kept: the loop count (two spellings) and an ICC
/// profile. Everything else under 0xFF (XMP among them) is dropped.
const GIF_APPS_KEPT: &[&[u8; 11]] = &[b"NETSCAPE2.0", b"ANIMEXTS1.0", b"ICCRGBG1012"];

/// GIF: header, colour tables, frames and their graphic control blocks, and the
/// kept application extensions, up to the trailer. Comments, plain-text blocks
/// and every other extension are dropped.
pub fn gif(input: &[u8]) -> Option<Vec<u8>> {
    if !(input.starts_with(b"GIF87a") || input.starts_with(b"GIF89a")) {
        return None;
    }
    let packed = *input.get(10)?;
    let mut i = 13 + color_table_len(packed);
    let mut out = input.get(..i)?.to_vec();
    loop {
        match *input.get(i)? {
            0x3B => {
                out.push(0x3B);
                return Some(out);
            }
            0x2C => {
                let packed = *input.get(i + 9)?;
                // Descriptor, local table, LZW minimum code size, then data.
                let data = i + 10 + color_table_len(packed) + 1;
                let end = sub_blocks_end(input, data)?;
                out.extend_from_slice(input.get(i..end)?);
                i = end;
            }
            0x21 => {
                let label = *input.get(i + 1)?;
                let end = sub_blocks_end(input, i + 2)?;
                let keep = match label {
                    0xF9 => true,
                    0xFF => {
                        let id = input.get(i + 3..i + 14)?;
                        *input.get(i + 2)? == 11 && GIF_APPS_KEPT.iter().any(|k| &k[..] == id)
                    }
                    _ => false,
                };
                if keep {
                    out.extend_from_slice(input.get(i..end)?);
                }
                i = end;
            }
            _ => return None,
        }
    }
}

fn color_table_len(packed: u8) -> usize {
    if packed & 0x80 == 0 {
        0
    } else {
        3 << ((packed & 0x07) + 1)
    }
}

/// The index just past a run of GIF data sub-blocks and its terminator.
fn sub_blocks_end(input: &[u8], mut j: usize) -> Option<usize> {
    loop {
        let n = usize::from(*input.get(j)?);
        j += 1;
        if n == 0 {
            return Some(j);
        }
        j += n;
    }
}

/// WebP chunks kept: the bitstreams, alpha, the extended header, animation
/// and the ICC profile. `EXIF`, `XMP ` and unknown chunks are dropped, and the
/// extended header's EXIF and XMP flags cleared to match.
const WEBP_KEEP: &[&[u8; 4]] = &[
    b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP",
];

/// WebP: the kept chunks inside a RIFF header sized to them; anything past the
/// RIFF size the file declares is dropped.
pub fn webp(input: &[u8]) -> Option<Vec<u8>> {
    if input.len() < 12 || &input[..4] != b"RIFF" || &input[8..12] != b"WEBP" {
        return None;
    }
    let riff_len = u32::from_le_bytes(input[4..8].try_into().ok()?) as usize;
    let body = input.get(12..8usize.checked_add(riff_len)?)?;
    let mut chunks = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let kind: &[u8; 4] = body.get(i..i + 4)?.try_into().ok()?;
        let len = u32::from_le_bytes(body.get(i + 4..i + 8)?.try_into().ok()?) as usize;
        let padded = len.checked_add(len & 1)?;
        let end = i.checked_add(8)?.checked_add(padded)?;
        let chunk = body.get(i..end)?;
        if WEBP_KEEP.contains(&kind) {
            let at = chunks.len();
            chunks.extend_from_slice(chunk);
            if kind == b"VP8X" && len > 0 {
                chunks[at + 8] &= !(0x08 | 0x04);
            }
        }
        i = end;
    }
    let mut out = Vec::with_capacity(chunks.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&u32::try_from(chunks.len() + 4).ok()?.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&chunks);
    Some(out)
}
