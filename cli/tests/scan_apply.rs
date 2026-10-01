mod common;

use common::{info, scene, write_bomb_png, write_png, write_valid_bomb_png};
use imgdedup::*;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const OPTS: ScanOptions = ScanOptions { recursive: true, min_width: 0, min_height: 0 };

fn names(images: &[ImageInfo]) -> Vec<String> {
    let mut v: Vec<String> = images.iter().map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn find<'a>(images: &'a [ImageInfo], name: &str) -> &'a ImageInfo {
    images.iter().find(|i| i.path.file_name().unwrap() == name).unwrap_or_else(|| panic!("{name} not scanned"))
}

fn edge(a: usize, b: usize, score: f64, high: bool) -> Edge {
    Edge { a, b, score, high }
}

// ── scan ────────────────────────────────────────────────────────────────

fn build_tree(root: &Path) {
    fs::create_dir_all(root.join("sub")).unwrap();
    write_png(&root.join("a.png"), &scene(1, 200, 150));
    write_png(&root.join("tiny.png"), &scene(1, 10, 10));
    write_valid_bomb_png(&root.join("bomb.png"));
    write_bomb_png(&root.join("bad_crc_bomb.png"));
    fs::write(root.join("garbage.jpg"), b"this is not a jpeg").unwrap();
    fs::write(root.join("notes.txt"), b"hello").unwrap();
    write_png(&root.join("sub").join("deep.png"), &scene(2, 120, 90));
}

#[test]
fn scan_recursive_counts_every_outcome() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let r = scan(dir.path(), &ScanOptions { recursive: true, min_width: 50, min_height: 50 }).unwrap();
    assert_eq!(names(&r.images), ["a.png", "deep.png"]);
    assert_eq!(r.too_small, 1, "tiny.png");
    assert_eq!(r.too_large, 1, "bomb.png");
    // A bomb with a damaged header is rejected too, just as unreadable rather than "too large".
    let mut bad: Vec<String> = r.unreadable.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    bad.sort();
    assert_eq!(bad, ["bad_crc_bomb.png", "garbage.jpg"]);
    assert_eq!(r.links, 0);
}

#[test]
fn scan_flat_ignores_subfolders() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let r = scan(dir.path(), &ScanOptions { recursive: false, min_width: 0, min_height: 0 }).unwrap();
    assert_eq!(names(&r.images), ["a.png", "tiny.png"]);
}

#[test]
fn scan_records_size_time_and_hashes() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let r = scan(dir.path(), &OPTS).unwrap();
    let a = find(&r.images, "a.png");
    assert_eq!((a.width, a.height), (200, 150));
    assert_eq!(a.file_size, fs::metadata(dir.path().join("a.png")).unwrap().len());
    assert!(a.modified > std::time::SystemTime::UNIX_EPOCH);
    assert_eq!(a.md5.len(), 32);
    assert_ne!(a.phash, 0);
}

#[test]
fn scan_min_size_uses_either_side() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("wide.png"), &scene(1, 300, 40));
    let r = scan(dir.path(), &ScanOptions { recursive: true, min_width: 100, min_height: 100 }).unwrap();
    assert!(r.images.is_empty());
    assert_eq!(r.too_small, 1);
}

#[test]
fn scan_rejects_a_missing_root() {
    assert!(scan(Path::new("definitely/not/here"), &OPTS).is_err());
}

#[cfg(windows)]
#[test]
fn scan_does_not_follow_junctions() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_png(&outside.path().join("secret.png"), &scene(3, 100, 100));
    write_png(&dir.path().join("a.png"), &scene(1, 100, 100));
    let link = dir.path().join("jct");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .expect("run mklink");
    assert!(status.status.success(), "mklink failed: {}", String::from_utf8_lossy(&status.stderr));
    for recursive in [true, false] {
        let r = scan(dir.path(), &ScanOptions { recursive, min_width: 0, min_height: 0 }).unwrap();
        assert_eq!(names(&r.images), ["a.png"], "recursive={recursive}");
        assert_eq!(r.links, 1, "recursive={recursive}");
    }
    // Remove the junction itself (not its target) before the temp dirs drop.
    fs::remove_dir(&link).unwrap();
    assert!(outside.path().join("secret.png").exists());
}

#[cfg(unix)]
#[test]
fn scan_does_not_follow_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_png(&outside.path().join("secret.png"), &scene(3, 100, 100));
    write_png(&dir.path().join("a.png"), &scene(1, 100, 100));
    std::os::unix::fs::symlink(outside.path(), dir.path().join("dirlink")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.png"), dir.path().join("filelink.png")).unwrap();
    for recursive in [true, false] {
        let r = scan(dir.path(), &ScanOptions { recursive, min_width: 0, min_height: 0 }).unwrap();
        assert_eq!(names(&r.images), ["a.png"], "recursive={recursive}");
        assert_eq!(r.links, 2, "recursive={recursive}");
    }
}

// ── find_duplicates on real files ───────────────────────────────────────

#[test]
fn find_duplicates_groups_copy_and_resize_but_not_other_pictures() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    write_png(&dir.path().join("a.png"), &scene(1, 200, 150));
    fs::copy(dir.path().join("a.png"), dir.path().join("sub").join("a_copy.png")).unwrap();
    write_png(&dir.path().join("a_small.png"), &scene(1, 100, 75));
    write_png(&dir.path().join("other.png"), &scene(2, 200, 150));
    let imgs = scan(dir.path(), &OPTS).unwrap().images;
    let r = find_duplicates(&imgs, 10, 0.90);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.groups.len(), 1);
    let g = &r.groups[0];
    assert_eq!(g.confidence, Confidence::High);
    assert_eq!(g.keeper.path.file_name().unwrap(), "a.png");
    let mut dups: Vec<String> = g.duplicates.iter().map(|d| d.info.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    dups.sort();
    assert_eq!(dups, ["a_copy.png", "a_small.png"]);
    // The byte-identical copy takes the MD5 fast path and scores exactly 1.0.
    let copy = g.duplicates.iter().find(|d| d.info.path.ends_with("a_copy.png")).unwrap();
    assert_eq!(copy.ssim, 1.0);
}

#[test]
fn find_duplicates_reports_a_file_that_vanished_after_scan() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("a.png"), &scene(1, 200, 150));
    write_png(&dir.path().join("a_small.png"), &scene(1, 100, 75));
    let imgs = scan(dir.path(), &OPTS).unwrap().images;
    fs::remove_file(dir.path().join("a_small.png")).unwrap();
    let r = find_duplicates(&imgs, 10, 0.90);
    assert!(r.groups.is_empty());
    assert_eq!(r.errors.len(), 1);
}

#[test]
fn find_duplicates_with_fewer_than_two_images_is_empty() {
    assert!(find_duplicates(&[], 10, 0.9).groups.is_empty());
    assert!(find_duplicates(&[info("x", 1, 1, 1)], 10, 0.9).groups.is_empty());
}

// ── report ──────────────────────────────────────────────────────────────

fn two_groups() -> Vec<Group> {
    let imgs = vec![info("k1.png", 200, 100, 500), info("d1.png", 100, 50, 300), info("k2.jpg", 80, 80, 90), info("d2.jpg", 40, 40, 20)];
    build_groups(&imgs, &[edge(0, 1, 0.9876, true), edge(2, 3, 0.93, false)])
}

#[test]
fn group_action_follows_confidence_and_flag() {
    let groups = two_groups();
    let high = groups.iter().find(|g| g.confidence == Confidence::High).unwrap();
    let low = groups.iter().find(|g| g.confidence == Confidence::Low).unwrap();
    assert_eq!(group_action(high, false), Action::Delete);
    assert_eq!(group_action(high, true), Action::Delete);
    assert_eq!(group_action(low, false), Action::Review);
    assert_eq!(group_action(low, true), Action::Delete);
}

fn report(groups: &[Group], include_low: bool) -> Vec<Vec<String>> {
    let mut buf = Vec::new();
    write_report(groups, include_low, &mut buf).unwrap();
    String::from_utf8(buf).unwrap().lines().map(|l| l.split('\t').map(str::to_owned).collect()).collect()
}

#[test]
fn report_has_header_keep_delete_and_review_rows() {
    let rows = report(&two_groups(), false);
    assert_eq!(rows[0], ["group", "action", "width", "height", "bytes", "ssim", "confidence", "path"]);
    assert_eq!(rows.len(), 5);
    let by_path = |p: &str| rows.iter().find(|r| r[7] == p).unwrap_or_else(|| panic!("no row for {p}"));
    assert_eq!(by_path("k1.png").as_slice(), ["1", "KEEP", "200", "100", "500", "", "high", "k1.png"]);
    assert_eq!(by_path("d1.png").as_slice(), ["1", "DELETE", "100", "50", "300", "0.9876", "high", "d1.png"]);
    assert_eq!(by_path("k2.jpg")[1], "KEEP");
    assert_eq!(by_path("d2.jpg").as_slice(), ["2", "REVIEW", "40", "40", "20", "0.9300", "low", "d2.jpg"]);
}

#[test]
fn report_marks_low_confidence_as_delete_when_included() {
    let rows = report(&two_groups(), true);
    assert!(rows.iter().skip(1).all(|r| r[1] != "REVIEW"));
    assert_eq!(rows.iter().filter(|r| r[1] == "DELETE").count(), 2);
}

#[test]
fn report_keeps_one_row_per_line_even_for_odd_names() {
    let imgs = vec![info("a\tb\nc.png", 20, 20, 5), info("d.png", 10, 10, 4)];
    let groups = build_groups(&imgs, &[edge(0, 1, 1.0, true)]);
    let rows = report(&groups, false);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.len() == 8));
}

// ── apply ───────────────────────────────────────────────────────────────

struct Fixture {
    dir: tempfile::TempDir,
    imgs: Vec<ImageInfo>,
    groups: Vec<Group>,
}

impl Fixture {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// Files: k1 (keeper) + d1 (high-confidence copy); k2 (keeper) + l2 (low-confidence copy); u (unrelated).
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("k1.png"), &scene(1, 200, 150));
    write_png(&dir.path().join("d1.png"), &scene(1, 100, 75));
    write_png(&dir.path().join("k2.png"), &scene(2, 200, 150));
    write_png(&dir.path().join("l2.png"), &scene(2, 100, 75));
    write_png(&dir.path().join("u.png"), &scene(3, 120, 90));
    let imgs = scan(dir.path(), &OPTS).unwrap().images;
    assert!(imgs.iter().all(|i| i.modified > SystemTime::UNIX_EPOCH));
    let at = |n: &str| imgs.iter().position(|i| i.path.file_name().unwrap() == n).unwrap();
    let groups = build_groups(
        &imgs,
        &[edge(at("k1.png"), at("d1.png"), 0.99, true), edge(at("k2.png"), at("l2.png"), 0.93, false)],
    );
    assert_eq!(groups.len(), 2);
    Fixture { dir, imgs, groups }
}

/// Injected delete function: records the call and really removes the file (inside the temp dir).
fn recording_delete(calls: &mut Vec<PathBuf>) -> impl FnMut(&Path) -> io::Result<()> + '_ {
    move |p: &Path| {
        calls.push(p.to_path_buf());
        fs::remove_file(p)
    }
}

#[test]
fn apply_deletes_only_delete_copies() {
    let fx = fixture();
    let mut calls = Vec::new();
    let out = apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert_eq!(calls, [fx.path("d1.png")]);
    assert_eq!(out.deleted, [fx.path("d1.png")]);
    assert!(out.left.is_empty() && out.failed.is_empty());
    for kept in ["k1.png", "k2.png", "l2.png", "u.png"] {
        assert!(fx.path(kept).exists(), "{kept} must survive");
    }
    assert!(!fx.path("d1.png").exists());
}

#[test]
fn apply_with_include_low_confidence_also_deletes_review_copies() {
    let fx = fixture();
    let mut calls = Vec::new();
    let out = apply(&fx.groups, true, &mut recording_delete(&mut calls));
    calls.sort();
    assert_eq!(calls, [fx.path("d1.png"), fx.path("l2.png")]);
    assert_eq!(out.deleted.len(), 2);
    assert!(fx.path("k1.png").exists() && fx.path("k2.png").exists() && fx.path("u.png").exists());
}

#[test]
fn apply_never_deletes_a_keeper_even_if_the_keeper_is_listed_as_copy_elsewhere() {
    let fx = fixture();
    let mut calls = Vec::new();
    apply(&fx.groups, true, &mut recording_delete(&mut calls));
    assert!(!calls.contains(&fx.path("k1.png")) && !calls.contains(&fx.path("k2.png")));
}

#[test]
fn apply_leaves_a_copy_whose_size_changed_after_the_scan() {
    let fx = fixture();
    let mut bytes = fs::read(fx.path("d1.png")).unwrap();
    bytes.extend_from_slice(b"trailing");
    fs::write(fx.path("d1.png"), bytes).unwrap();
    let mut calls = Vec::new();
    let out = apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert!(calls.is_empty());
    assert!(out.deleted.is_empty());
    assert_eq!(out.left.len(), 1);
    assert_eq!(out.left[0].0, fx.path("d1.png"));
    assert!(out.left[0].1.contains("changed"), "reason: {}", out.left[0].1);
    assert!(fx.path("d1.png").exists());
}

#[test]
fn apply_leaves_a_copy_whose_size_changed_even_when_the_modified_time_was_restored() {
    let fx = fixture();
    let old = fx.imgs.iter().find(|i| i.path.ends_with("d1.png")).unwrap().modified;
    let mut bytes = fs::read(fx.path("d1.png")).unwrap();
    bytes.extend_from_slice(b"trailing");
    fs::write(fx.path("d1.png"), bytes).unwrap();
    let f = fs::OpenOptions::new().write(true).open(fx.path("d1.png")).unwrap();
    f.set_modified(old).unwrap();
    drop(f);
    let mut calls = Vec::new();
    let out = apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert!(calls.is_empty());
    assert_eq!(out.left.len(), 1);
    assert!(out.left[0].1.contains("size"), "reason: {}", out.left[0].1);
}

#[test]
fn apply_leaves_a_copy_whose_modified_time_changed_after_the_scan() {
    let fx = fixture();
    let old = fx.imgs.iter().find(|i| i.path.ends_with("d1.png")).unwrap().modified;
    let f = fs::OpenOptions::new().write(true).open(fx.path("d1.png")).unwrap();
    f.set_modified(old + Duration::from_secs(3600)).unwrap(); // same size, different time
    drop(f);
    let mut calls = Vec::new();
    let out = apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert!(calls.is_empty());
    assert_eq!(out.left.len(), 1);
    assert!(fx.path("d1.png").exists());
}

#[test]
fn apply_leaves_copies_when_the_keeper_changed_or_vanished() {
    let fx = fixture();
    fs::remove_file(fx.path("k1.png")).unwrap();
    let mut calls = Vec::new();
    let out = apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert!(calls.is_empty(), "the only remaining original must not be removed");
    assert_eq!(out.left.len(), 1);
    assert_eq!(out.left[0].0, fx.path("d1.png"));
    assert!(out.left[0].1.contains("keeper"), "reason: {}", out.left[0].1);
    assert!(fx.path("d1.png").exists());

    let fx = fixture();
    let f = fs::OpenOptions::new().write(true).open(fx.path("k1.png")).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(7200)).unwrap();
    drop(f);
    let mut calls = Vec::new();
    apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert!(calls.is_empty());
    assert!(fx.path("d1.png").exists());
}

#[test]
fn apply_leaves_a_copy_that_is_already_gone() {
    let fx = fixture();
    fs::remove_file(fx.path("d1.png")).unwrap();
    let mut calls = Vec::new();
    let out = apply(&fx.groups, false, &mut recording_delete(&mut calls));
    assert!(calls.is_empty());
    assert_eq!(out.left.len(), 1);
}

#[test]
fn apply_reports_a_failed_delete_and_carries_on() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("k.png"), &scene(1, 200, 150));
    write_png(&dir.path().join("d1.png"), &scene(1, 100, 75));
    write_png(&dir.path().join("d2.png"), &scene(1, 80, 60));
    let imgs = scan(dir.path(), &OPTS).unwrap().images;
    let at = |n: &str| imgs.iter().position(|i| i.path.file_name().unwrap() == n).unwrap();
    let groups = build_groups(&imgs, &[edge(at("k.png"), at("d1.png"), 0.99, true), edge(at("k.png"), at("d2.png"), 0.99, true)]);
    let mut seen = Vec::new();
    let mut flaky = |p: &Path| -> io::Result<()> {
        seen.push(p.to_path_buf());
        if p.ends_with("d1.png") {
            Err(io::Error::other("bin is full"))
        } else {
            fs::remove_file(p)
        }
    };
    let out = apply(&groups, false, &mut flaky);
    assert_eq!(seen.len(), 2, "a failure must not stop later deletes");
    assert_eq!(out.failed.len(), 1);
    assert!(out.failed[0].0.ends_with("d1.png") && out.failed[0].1.contains("bin is full"));
    assert_eq!(out.deleted.len(), 1);
    assert!(dir.path().join("d1.png").exists(), "a failed delete never falls back to another method");
    assert!(!dir.path().join("d2.png").exists());
}
