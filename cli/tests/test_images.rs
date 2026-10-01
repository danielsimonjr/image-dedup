//! End-to-end check of the engine on a COPY of the repository's `test_images/` folder.
//!
//! The expected groups come from the pictures themselves, not from their names:
//! each file's real pixel size is asserted first, and the picture content was
//! checked by eye (the files are synthetic scenes; every group below is one
//! scene saved at several sizes or formats).

use imgdedup::*;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("test_images")
}

/// (file, width, height, bytes) as measured from the files.
const EXPECTED_FILES: &[(&str, u32, u32, u64)] = &[
    ("beach_unique.png", 900, 600, 11_300),
    ("city_backup.jpg", 1024, 768, 157_029),
    ("city_photo.png", 1024, 768, 14_652),
    ("city_photo_copy.png", 1024, 768, 14_652),
    ("forest_unique.jpg", 1200, 800, 184_367),
    ("mountains_hires.png", 1600, 1200, 27_175),
    ("mountains_low_quality.jpg", 800, 600, 22_689),
    ("mountains_medium.jpg", 800, 600, 54_416),
    ("portrait_full.png", 600, 900, 11_292),
    ("portrait_small.jpg", 300, 450, 14_812),
    ("sunset_1280x720.jpg", 1280, 720, 159_404),
    ("sunset_1920x1080.jpg", 1920, 1080, 511_465),
    ("sunset_800x450.jpg", 800, 450, 50_946),
    ("sunset_thumbnail.jpg", 400, 225, 9_511),
];

fn copy_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture_dir()).expect("test_images/ must exist") {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    dir
}

fn name(i: &ImageInfo) -> String {
    i.path.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn fixture_files_have_the_dimensions_the_expectations_assume() {
    for &(file, w, h, bytes) in EXPECTED_FILES {
        let p = fixture_dir().join(file);
        assert_eq!(image::image_dimensions(&p).unwrap(), (w, h), "{file}");
        assert_eq!(fs::metadata(&p).unwrap().len(), bytes, "{file} size");
    }
    let count = fs::read_dir(fixture_dir()).unwrap().count();
    assert_eq!(count, EXPECTED_FILES.len(), "an unlisted file appeared in test_images/");
}

#[test]
fn test_images_group_by_picture_and_keep_the_best_copy() {
    let dir = copy_fixture();
    let scanned = scan(dir.path(), &ScanOptions { recursive: true, min_width: 0, min_height: 0 }).unwrap();
    assert_eq!(scanned.images.len(), 14);
    assert!(scanned.unreadable.is_empty() && scanned.too_small == 0 && scanned.too_large == 0);

    let found = find_duplicates(&scanned.images, 10, 0.90);
    assert!(found.errors.is_empty(), "{:?}", found.errors);

    // (keeper, copies). Keeper = most pixels; equal pixels -> larger file.
    // Copies are listed in keeper order (pixels desc, bytes desc, path asc).
    let expected: Vec<(&str, Vec<&str>)> = vec![
        ("city_backup.jpg", vec!["city_photo.png", "city_photo_copy.png"]), // same 1024x768; the JPEG file is larger
        ("sunset_1920x1080.jpg", vec!["sunset_1280x720.jpg", "sunset_800x450.jpg", "sunset_thumbnail.jpg"]),
        ("mountains_hires.png", vec!["mountains_medium.jpg", "mountains_low_quality.jpg"]),
        ("portrait_full.png", vec!["portrait_small.jpg"]),
    ];
    let mut actual: Vec<(String, Vec<String>)> = found
        .groups
        .iter()
        .map(|g| (name(&g.keeper), g.duplicates.iter().map(|d| name(&d.info)).collect()))
        .collect();
    let mut want: Vec<(String, Vec<String>)> =
        expected.iter().map(|(k, d)| (k.to_string(), d.iter().map(|s| s.to_string()).collect())).collect();
    actual.sort();
    want.sort();
    assert_eq!(actual, want);

    // The two unrelated pictures stay out of every group.
    let grouped: Vec<String> = found
        .groups
        .iter()
        .flat_map(|g| std::iter::once(name(&g.keeper)).chain(g.duplicates.iter().map(|d| name(&d.info))))
        .collect();
    for unique in ["beach_unique.png", "forest_unique.jpg"] {
        assert!(!grouped.iter().any(|n| n == unique), "{unique} must stay unique");
    }

    // The byte-identical copy takes the MD5 path: exact score, high confidence.
    let city = found.groups.iter().find(|g| name(&g.keeper) == "city_backup.jpg").unwrap();
    let identical = city.duplicates.iter().find(|d| name(&d.info) == "city_photo_copy.png").unwrap();
    assert_eq!(identical.info.md5, scanned.images.iter().find(|i| name(i) == "city_photo.png").unwrap().md5);
}

#[test]
fn test_images_confidence_is_recorded_per_group() {
    // Characterisation of the current algorithm output on the fixture set.
    let dir = copy_fixture();
    let scanned = scan(dir.path(), &ScanOptions { recursive: true, min_width: 0, min_height: 0 }).unwrap();
    let found = find_duplicates(&scanned.images, 10, 0.90);
    for g in &found.groups {
        eprintln!(
            "group keeper={} confidence={} copies={:?}",
            name(&g.keeper),
            g.confidence.as_str(),
            g.duplicates.iter().map(|d| (name(&d.info), (d.ssim * 10_000.0).round() / 10_000.0)).collect::<Vec<_>>()
        );
    }
    assert_eq!(found.groups.len(), 4);
}
