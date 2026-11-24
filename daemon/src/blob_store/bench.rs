//! **ADR-115's Performance section, the store's half: kept runnable.**
//!
//! `#[ignore]`d: they build images of up to 40 megapixels and a gigabyte of
//! blobs. Documentation that executes, never part of the gating suite, and with
//! no assertion on time: each prints its numbers and the budget beside them.
//! Run in release, on a disk-backed `TMPDIR` (not tmpfs: the files are large):
//!
//! ```text
//! cd daemon && TMPDIR=~/.cache/mae-bench cargo test --release --lib blob_bench -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Peak memory is the process's high-water mark (`VmHWM`), reset between cases
//! through `/proc/self/clear_refs`: Linux only, and it reports 0 elsewhere.
//! The upload, download and backup numbers are the routes' half, in the
//! binary's `blob_bench_tests`.

use std::io::Cursor;
use std::time::Instant;

use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};

use super::process::{process, ProcessLimits};

const LIMITS: ProcessLimits = ProcessLimits {
    max_pixels: 40_000_000,
    max_frames: 1000,
};

fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))?
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

/// Reset the high-water mark to the current RSS (Linux `clear_refs` 5).
pub(crate) fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// Peak RSS since the last [`reset_peak`], MiB.
pub(crate) fn peak_mib() -> f64 {
    status_kb("VmHWM:") as f64 / 1024.0
}

/// A photo-like image: smooth gradients plus noise, so the encoders do real
/// work (a flat image compresses to nothing and measures nothing).
fn photo(w: u32, h: u32) -> RgbImage {
    let mut state = 0x2545_F491_u32;
    RgbImage::from_fn(w, h, |x, y| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let n = (state & 0x1F) as u8;
        image::Rgb([
            ((x * 255 / w.max(1)) as u8).wrapping_add(n),
            ((y * 255 / h.max(1)) as u8).wrapping_add(n),
            (((x + y) * 127 / (w + h).max(1)) as u8).wrapping_add(n),
        ])
    })
}

fn encode(img: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    let dynamic = match format {
        ImageFormat::WebP => {
            DynamicImage::ImageRgba8(DynamicImage::ImageRgb8(img.clone()).to_rgba8())
        }
        _ => DynamicImage::ImageRgb8(img.clone()),
    };
    dynamic
        .write_to(&mut Cursor::new(&mut out), format)
        .unwrap();
    out
}

/// An EXIF APP1 with orientation 6, inserted after SOI: the rotate path.
fn rotated_jpeg(jpeg: &[u8]) -> Vec<u8> {
    let mut tiff = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    let mut app1 = b"Exif\0\0".to_vec();
    app1.append(&mut tiff);
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&(app1.len() as u16 + 2).to_be_bytes());
    seg.extend_from_slice(&app1);
    [&jpeg[..2], &seg[..], &jpeg[2..]].concat()
}

fn measure(label: &str, mp: f64, input: Vec<u8>) {
    let size = input.len();
    reset_peak();
    let before = peak_mib();
    let t = Instant::now();
    let out = process(input, &LIMITS);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let peak = peak_mib() - before;
    match out {
        Ok(p) => println!(
            "process {label:<14} {mp:>5.1} MP  in {:>9} B  out {:>9} B  {ms:>9.1} ms  peak +{peak:>7.1} MiB",
            size,
            p.bytes.len()
        ),
        Err(e) => println!("process {label:<14} {mp:>5.1} MP  REFUSED {e:?}"),
    }
}

/// Decode, strip and rotate cost per format at 1, 12 and 40 megapixels (4:3).
/// These set `PROCESS_TIMEOUT` and confirm the decode's memory bound.
#[test]
#[ignore = "blob_bench: 40-megapixel images"]
fn blob_bench_processing() {
    for mp in [1.0f64, 12.0, 40.0] {
        let w = (mp * 1e6 * 4.0 / 3.0).sqrt() as u32;
        let h = (mp * 1e6 / f64::from(w)) as u32;
        let img = photo(w, h);
        let jpeg = encode(&img, ImageFormat::Jpeg);
        measure("jpeg", mp, jpeg.clone());
        measure("jpeg rotated", mp, rotated_jpeg(&jpeg));
        measure("png", mp, encode(&img, ImageFormat::Png));
        measure("webp", mp, encode(&img, ImageFormat::WebP));
        drop(img);
        // An animated GIF of 10 frames whose total is the same pixel count.
        let (fw, fh) = (w / 4, h / 3 + 1);
        let frames = (mp * 1e6 / f64::from(fw * fh)).round().max(1.0) as usize;
        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut gif, 30);
            for i in 0..frames {
                let f = RgbaImage::from_fn(fw, fh, |x, y| {
                    image::Rgba([(x as u8).wrapping_add(i as u8), y as u8, 40, 255])
                });
                enc.encode_frame(image::Frame::new(f)).unwrap();
            }
        }
        measure(&format!("gif x{frames}"), mp, gif);
    }
}

/// SQLite's internal-versus-file crossover (D1's evidence), re-measured here:
/// reading a blob from a table against reading it from a file, per size.
#[test]
#[ignore = "blob_bench: writes ~100 MB"]
fn blob_bench_sqlite_crossover() {
    let dir = tempfile::tempdir().unwrap();
    let db = sqlite::Connection::open(dir.path().join("x.db")).unwrap();
    db.execute("PRAGMA journal_mode=WAL; CREATE TABLE b (id INTEGER PRIMARY KEY, v BLOB)")
        .unwrap();
    const N: usize = 50;
    for size in [10 * 1024, 50 * 1024, 100 * 1024, 500 * 1024, 1024 * 1024] {
        let data: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
        for i in 0..N {
            let mut st = db.prepare("INSERT INTO b (id, v) VALUES (?, ?)").unwrap();
            st.bind((1, (size * 1000 + i) as i64)).unwrap();
            st.bind((2, &data[..])).unwrap();
            st.next().unwrap();
            std::fs::write(dir.path().join(format!("{size}-{i}")), &data).unwrap();
        }
        let t = Instant::now();
        for i in 0..N {
            let mut st = db.prepare("SELECT v FROM b WHERE id = ?").unwrap();
            st.bind((1, (size * 1000 + i) as i64)).unwrap();
            st.next().unwrap();
            let v: Vec<u8> = st.read(0).unwrap();
            assert_eq!(v.len(), size);
        }
        let internal = t.elapsed().as_secs_f64();
        let t = Instant::now();
        for i in 0..N {
            let v = std::fs::read(dir.path().join(format!("{size}-{i}"))).unwrap();
            assert_eq!(v.len(), size);
        }
        let external = t.elapsed().as_secs_f64();
        println!(
            "crossover {:>7} B  sqlite {:>8.1} us  file {:>8.1} us  external/internal {:.3}",
            size,
            internal / N as f64 * 1e6,
            external / N as f64 * 1e6,
            external / internal
        );
    }
}
