//! Shared test fixtures. Not every test file uses every helper.
#![allow(dead_code)]

use image::{GrayImage, Luma};
use imgdedup::ImageInfo;
use std::path::{Path, PathBuf};

/// A resolution-independent test picture: a gradient background plus
/// eight rectangles placed from `seed`. The same seed gives the same
/// picture at any size, so a 256x256 and a 64x64 render are "resized copies".
pub fn scene(seed: u32, w: u32, h: u32) -> GrayImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12_345);
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / 16_777_216.0
    };
    let rects: Vec<(f32, f32, f32, f32, u8)> = (0..8)
        .map(|_| {
            let x0 = next() * 0.8;
            let y0 = next() * 0.8;
            let rw = 0.1 + next() * 0.3;
            let rh = 0.1 + next() * 0.3;
            let v = (next() * 255.0) as u8;
            (x0, y0, x0 + rw, y0 + rh, v)
        })
        .collect();
    GrayImage::from_fn(w, h, |x, y| {
        let fx = (x as f32 + 0.5) / w as f32;
        let fy = (y as f32 + 0.5) / h as f32;
        let bg = if seed.is_multiple_of(2) { fx } else { fy };
        let mut v = (40.0 + bg * 170.0) as u8;
        for &(x0, y0, x1, y1, c) in &rects {
            if fx >= x0 && fx < x1 && fy >= y0 && fy < y1 {
                v = c;
            }
        }
        Luma([v])
    })
}

/// Pseudo-random noise image (no structure).
pub fn noise(seed: u32, w: u32, h: u32) -> GrayImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(99);
    GrayImage::from_fn(w, h, |_, _| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        Luma([(state >> 24) as u8])
    })
}

/// Hand-built metadata record for grouping tests (no file behind it).
pub fn info(name: &str, w: u32, h: u32, size: u64) -> ImageInfo {
    ImageInfo {
        path: PathBuf::from(name),
        width: w,
        height: h,
        file_size: size,
        modified: std::time::SystemTime::UNIX_EPOCH,
        phash: 0,
        dhash: 0,
        md5: format!("md5-of-{name}"),
    }
}

pub fn write_png(path: &Path, img: &GrayImage) {
    img.save(path).expect("write png");
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A bomb as an attacker writes it: the IHDR checksum is correct, so the decoder
/// accepts the header and reports 65535 x 65535 without any pixel data behind it.
pub fn write_valid_bomb_png(path: &Path) {
    use std::io::Write;
    let mut chunk = b"IHDR".to_vec();
    chunk.extend_from_slice(&[0, 0, 255, 255, 0, 0, 255, 255, 8, 2, 0, 0, 0]);
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&[137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
    f.write_all(&[0, 0, 0, 13]).unwrap();
    f.write_all(&chunk).unwrap();
    f.write_all(&crc32(&chunk).to_be_bytes()).unwrap();
    // The decoder reads the header up to the first IDAT chunk; an empty one is enough.
    f.write_all(&[0, 0, 0, 0]).unwrap();
    f.write_all(b"IDAT").unwrap();
    f.write_all(&crc32(b"IDAT").to_be_bytes()).unwrap();
}

/// Same bomb with a bogus IHDR checksum (the original unit test's file).
pub fn write_bomb_png(path: &Path) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&[137u8, 80, 78, 71, 13, 10, 26, 10]).unwrap();
    f.write_all(&[0, 0, 0, 13]).unwrap();
    f.write_all(b"IHDR").unwrap();
    f.write_all(&[0, 0, 255, 255, 0, 0, 255, 255, 8, 2, 0, 0, 0])
        .unwrap();
    f.write_all(&[0u8, 0, 0, 0]).unwrap();
}
