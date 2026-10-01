mod common;

use common::{noise, scene};
use image::{GrayImage, Luma};
use imgdedup::*;
use std::path::Path;

#[test]
fn hamming_counts_differing_bits() {
    assert_eq!(hamming_distance(0, 0), 0);
    assert_eq!(hamming_distance(0, u64::MAX), 64);
    assert_eq!(hamming_distance(0b1010, 0b0110), 2);
    assert_eq!(hamming_distance(u64::MAX, u64::MAX), 0);
}

#[test]
fn phash_is_deterministic() {
    let img = scene(1, 200, 150);
    assert_eq!(compute_phash(&img), compute_phash(&img));
}

#[test]
fn phash_survives_resize() {
    for seed in [1, 2, 3, 4] {
        let big = compute_phash(&scene(seed, 512, 384));
        let small = compute_phash(&scene(seed, 128, 96));
        assert!(
            hamming_distance(big, small) <= 10,
            "seed {seed}: resized copy drifted by {}",
            hamming_distance(big, small)
        );
    }
}

#[test]
fn phash_separates_different_pictures() {
    for (a, b) in [(1, 2), (3, 4), (5, 8)] {
        let d = hamming_distance(
            compute_phash(&scene(a, 256, 256)),
            compute_phash(&scene(b, 256, 256)),
        );
        assert!(d > 10, "scenes {a} and {b} too close: {d}");
    }
}

#[test]
fn dhash_of_descending_gradient_sets_every_bit() {
    // Every pixel is brighter than its right neighbour, so left > right for all 64 cells.
    let img = GrayImage::from_fn(90, 80, |x, _| Luma([255 - (x * 255 / 89) as u8]));
    assert_eq!(compute_dhash(&img), u64::MAX);
}

#[test]
fn dhash_of_flat_image_is_zero() {
    let img = GrayImage::from_pixel(64, 64, Luma([120]));
    assert_eq!(compute_dhash(&img), 0);
}

#[test]
fn dhash_survives_resize_and_separates_pictures() {
    let big = compute_dhash(&scene(1, 512, 384));
    let small = compute_dhash(&scene(1, 128, 96));
    assert!(hamming_distance(big, small) <= 10);
    for (a, b) in [(1, 2), (3, 4)] {
        let d = hamming_distance(
            compute_dhash(&scene(a, 256, 256)),
            compute_dhash(&scene(b, 256, 256)),
        );
        assert!(d > 10, "scenes {a} and {b} too close: {d}");
    }
}

#[test]
fn ssim_of_identical_image_is_one() {
    let img = scene(1, 300, 200);
    let s = compute_ssim(&img, &img, SSIM_SIZE);
    assert!((s - 1.0).abs() < 1e-9, "got {s}");
}

#[test]
fn ssim_of_resized_copy_is_high() {
    let s = compute_ssim(&scene(1, 600, 400), &scene(1, 300, 200), SSIM_SIZE);
    assert!(s > 0.95, "got {s}");
}

#[test]
fn ssim_of_different_pictures_is_far_below_threshold() {
    let s = compute_ssim(&scene(1, 256, 256), &scene(2, 256, 256), SSIM_SIZE);
    assert!(s < 0.5, "different scenes scored {s}");
    let n = compute_ssim(&scene(1, 256, 256), &noise(7, 256, 256), SSIM_SIZE);
    assert!(n < 0.5, "scene vs noise scored {n}");
}

#[test]
fn ssim_of_inverted_image_is_negative() {
    let a = scene(1, 256, 256);
    let inv = GrayImage::from_fn(256, 256, |x, y| Luma([255 - a.get_pixel(x, y)[0]]));
    assert!(compute_ssim(&a, &inv, SSIM_SIZE) < 0.0);
}

#[test]
fn confidence_rule() {
    use Confidence::{High, Low};
    // Below the threshold: not a duplicate at all.
    assert_eq!(pair_confidence(0.89, 0.90, 0), None);
    // SSIM at or above the strict bar is high regardless of dHash.
    assert_eq!(pair_confidence(1.0, 0.90, 40), Some(High));
    assert_eq!(pair_confidence(0.98, 0.90, 64), Some(High));
    assert_eq!(pair_confidence(0.985, 0.90, 40), Some(High));
    // Between the threshold and the strict bar: dHash agreement decides.
    assert_eq!(pair_confidence(0.95, 0.90, 0), Some(High));
    assert_eq!(pair_confidence(0.95, 0.90, 10), Some(High));
    assert_eq!(pair_confidence(0.95, 0.90, 11), Some(Low));
    assert_eq!(pair_confidence(0.90, 0.90, 40), Some(Low));
    assert_eq!(pair_confidence(0.979_999, 0.90, 11), Some(Low));
    // A threshold above the strict bar still needs only the threshold.
    assert_eq!(pair_confidence(0.99, 0.985, 40), Some(High));
    assert_eq!(pair_confidence(0.98, 0.985, 0), None);
}

#[test]
fn pixel_budget_guard() {
    assert!(!exceeds_pixel_budget(10_000, 5_000)); // exactly 50 MP is allowed
    assert!(exceeds_pixel_budget(10_001, 5_000));
    assert!(!exceeds_pixel_budget(7_071, 7_071));
    assert!(exceeds_pixel_budget(7_072, 7_072));
    assert!(exceeds_pixel_budget(65_535, 65_535));
    assert!(!exceeds_pixel_budget(8_000, 4_000)); // 8K is 33 MP
    assert!(exceeds_pixel_budget(u32::MAX, u32::MAX)); // product must not overflow
}

#[test]
fn extension_check() {
    assert!(!has_image_extension(Path::new("/tmp/passwords.txt")));
    assert!(has_image_extension(Path::new("/tmp/photo.JPG")));
    assert!(has_image_extension(Path::new("a/b/c.tif")));
    assert!(has_image_extension(Path::new("x.WebP")));
    assert!(!has_image_extension(Path::new("noext")));
    assert!(!has_image_extension(Path::new("archive.png.zip")));
}
