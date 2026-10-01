//! Duplicate-image engine for the `imgdedup` command-line tool.
//!
//! The algorithm is a port of the Tauri backend (`src-tauri/src/lib.rs`),
//! without tauri, pyo3 or tokio:
//!
//! 1. Scan: peek at the declared size (decompression-bomb guard), decode,
//!    compute pHash, dHash and a streaming MD5.
//! 2. Candidates: byte-identical (MD5) or pHash Hamming distance <= threshold.
//! 3. Verify: SSIM on 256x256 grayscale; a pair is a duplicate at SSIM >= threshold.
//! 4. Confidence: high = MD5 match, or SSIM >= 0.98, or dHash agrees (distance <= 10);
//!    otherwise low. A group is high only if every edge inside it is high.
//! 5. Group with Union-Find. Keeper = most pixels; tie -> larger file.
//!
//! Deletion is separate from analysis: [`apply`] takes an injected delete
//! function, and re-checks size and modified time of every file first.

use image::GrayImage;
use md5::{Digest, Md5};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// File extensions that are scanned (compared in lower case).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];

/// Decompression-bomb guard: an image whose declared size exceeds this pixel
/// budget is rejected before decode. 50 megapixels allows 8K (about 33 MP)
/// and blocks 65535 x 65535 attack files.
pub const MAX_DECODE_PIXELS: u64 = 50_000_000;

/// SSIM at or above this value is high confidence by itself.
pub const STRICT_SSIM: f64 = 0.98;

/// Below `STRICT_SSIM`, a pair is high confidence only when the dHash distance is at most this.
pub const DHASH_AGREE_DISTANCE: u32 = 10;

/// Side length of the square both images are resized to for SSIM.
pub const SSIM_SIZE: u32 = 256;

const HASH_BUF_BYTES: usize = 64 * 1024;

// ── Data types ──────────────────────────────────────────────────────────

/// What the scan recorded about one image file.
#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    /// Modified time at scan; `apply` refuses to delete a file that no longer matches.
    pub modified: SystemTime,
    pub phash: u64,
    pub dhash: u64,
    pub md5: String,
}

impl ImageInfo {
    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    High,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Low => "low",
        }
    }
}

/// A verified duplicate pair: indices into the image list, the SSIM score and the confidence.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub a: usize,
    pub b: usize,
    pub score: f64,
    pub high: bool,
}

#[derive(Clone, Debug)]
pub struct Duplicate {
    pub info: ImageInfo,
    /// SSIM against the keeper; 0.0 when the pair was linked only through another member.
    pub ssim: f64,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub keeper: ImageInfo,
    pub duplicates: Vec<Duplicate>,
    pub confidence: Confidence,
}

impl Group {
    pub fn reclaimable_bytes(&self) -> u64 {
        self.duplicates.iter().map(|d| d.info.file_size).sum()
    }
}

// ── Small pure helpers ──────────────────────────────────────────────────

pub fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// True when the declared size is over the decompression-bomb budget.
pub fn exceeds_pixel_budget(width: u32, height: u32) -> bool {
    (width as u64) * (height as u64) > MAX_DECODE_PIXELS
}

pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Perceptual hash: 32x32 grayscale, 2D DCT, top-left 8x8 block, bits above the median.
pub fn compute_phash(img: &GrayImage) -> u64 {
    const N: usize = 32;
    let resized = image::imageops::resize(
        img,
        N as u32,
        N as u32,
        image::imageops::FilterType::Lanczos3,
    );

    // cos_table[k * N + i] = cos((2i + 1) * k * PI / 64), evaluated exactly as in the reference.
    let mut cos_table = [0.0f64; N * N];
    for k in 0..N {
        for i in 0..N {
            cos_table[k * N + i] =
                ((2 * i + 1) as f64 * k as f64 * std::f64::consts::PI / 64.0).cos();
        }
    }

    let mut dct = vec![0.0f64; N * N];
    for u in 0..N {
        for v in 0..N {
            let mut sum = 0.0f64;
            for x in 0..N {
                for y in 0..N {
                    let pixel = resized.get_pixel(y as u32, x as u32)[0] as f64;
                    sum += pixel * cos_table[u * N + x] * cos_table[v * N + y];
                }
            }
            dct[u * N + v] = sum;
        }
    }

    let mut low_freq = Vec::with_capacity(64);
    for u in 0..8 {
        for v in 0..8 {
            low_freq.push(dct[u * N + v]);
        }
    }

    // The median skips the DC term.
    let mut sorted = low_freq[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    let mut hash: u64 = 0;
    for (i, val) in low_freq.iter().enumerate() {
        if *val > median {
            hash |= 1 << (63 - i);
        }
    }
    hash
}

/// Difference hash on a 9x8 grid: bit set where a pixel is brighter than its right neighbour.
pub fn compute_dhash(img: &GrayImage) -> u64 {
    let resized = image::imageops::resize(img, 9, 8, image::imageops::FilterType::Lanczos3);
    let mut hash: u64 = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let left = resized.get_pixel(x, y)[0];
            let right = resized.get_pixel(x + 1, y)[0];
            if left > right {
                hash |= 1 << (y * 8 + x);
            }
        }
    }
    hash
}

/// Global SSIM of two grayscale images after resizing both to `target` x `target`.
pub fn compute_ssim(img1: &GrayImage, img2: &GrayImage, target: u32) -> f64 {
    let a = image::imageops::resize(img1, target, target, image::imageops::FilterType::Lanczos3);
    let b = image::imageops::resize(img2, target, target, image::imageops::FilterType::Lanczos3);

    let n = (target * target) as f64;
    let (c1, c2) = (6.5025, 58.5225);

    let (mut mean_a, mut mean_b) = (0.0f64, 0.0f64);
    for i in 0..n as usize {
        mean_a += a.as_raw()[i] as f64;
        mean_b += b.as_raw()[i] as f64;
    }
    mean_a /= n;
    mean_b /= n;

    let (mut var_a, mut var_b, mut cov) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n as usize {
        let da = a.as_raw()[i] as f64 - mean_a;
        let db = b.as_raw()[i] as f64 - mean_b;
        var_a += da * da;
        var_b += db * db;
        cov += da * db;
    }
    var_a /= n - 1.0;
    var_b /= n - 1.0;
    cov /= n - 1.0;

    let numerator = (2.0 * mean_a * mean_b + c1) * (2.0 * cov + c2);
    let denominator = (mean_a * mean_a + mean_b * mean_b + c1) * (var_a + var_b + c2);
    numerator / denominator
}

/// Classify a pair that is not byte-identical. `None` = not a duplicate (SSIM below the threshold).
pub fn pair_confidence(score: f64, ssim_threshold: f64, dhash_distance: u32) -> Option<Confidence> {
    if score < ssim_threshold {
        return None;
    }
    if score >= STRICT_SSIM || dhash_distance <= DHASH_AGREE_DISTANCE {
        Some(Confidence::High)
    } else {
        Some(Confidence::Low)
    }
}

// ── Union-Find ──────────────────────────────────────────────────────────

/// Disjoint sets with path halving and union by rank.
pub struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    pub fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    pub fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            Ordering::Less => self.parent[ra] = rb,
            Ordering::Greater => self.parent[rb] = ra,
            Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

// ── Grouping ────────────────────────────────────────────────────────────

/// Keeper order: most pixels first, then larger file, then path (a stable final tie-break).
fn keeper_order(a: &ImageInfo, b: &ImageInfo) -> Ordering {
    b.pixel_count()
        .cmp(&a.pixel_count())
        .then(b.file_size.cmp(&a.file_size))
        .then_with(|| a.path.cmp(&b.path))
}

/// Cluster images linked by `edges` and pick a keeper in each cluster of two or more.
/// Groups are sorted by reclaimable bytes, largest first.
pub fn build_groups(images: &[ImageInfo], edges: &[Edge]) -> Vec<Group> {
    let n = images.len();
    let mut uf = UnionFind::new(n);
    for e in edges {
        uf.union(e.a, e.b);
    }

    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        clusters.entry(uf.find(i)).or_default().push(i);
    }

    // First verified edge between two images supplies the score; any low edge demotes its cluster.
    let mut pair_score: HashMap<(usize, usize), f64> = HashMap::new();
    let mut low_roots: HashSet<usize> = HashSet::new();
    for e in edges {
        pair_score
            .entry((e.a.min(e.b), e.a.max(e.b)))
            .or_insert(e.score);
        if !e.high {
            low_roots.insert(uf.find(e.a));
        }
    }

    let mut groups = Vec::new();
    for (root, indices) in &clusters {
        if indices.len() < 2 {
            continue;
        }
        let mut members: Vec<usize> = indices.clone();
        members.sort_by(|&a, &b| keeper_order(&images[a], &images[b]));
        let keeper_idx = members[0];

        let duplicates = members[1..]
            .iter()
            .map(|&d| Duplicate {
                info: images[d].clone(),
                ssim: pair_score
                    .get(&(d.min(keeper_idx), d.max(keeper_idx)))
                    .copied()
                    .unwrap_or(0.0),
            })
            .collect();

        groups.push(Group {
            keeper: images[keeper_idx].clone(),
            duplicates,
            confidence: if low_roots.contains(root) {
                Confidence::Low
            } else {
                Confidence::High
            },
        });
    }

    groups.sort_by(|a, b| {
        b.reclaimable_bytes()
            .cmp(&a.reclaimable_bytes())
            .then_with(|| a.keeper.path.cmp(&b.keeper.path))
    });
    groups
}

// ── Scanning ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    pub recursive: bool,
    pub min_width: u32,
    pub min_height: u32,
}

/// Result of looking at one file.
#[derive(Debug)]
pub enum FileOutcome {
    Image(ImageInfo),
    TooSmall,
    /// Declared size over the decompression-bomb budget; never decoded.
    TooLarge,
    Unreadable(String),
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub images: Vec<ImageInfo>,
    pub too_small: usize,
    pub too_large: usize,
    /// Symbolic links and junctions that were skipped (never followed).
    pub links: usize,
    pub unreadable: Vec<(PathBuf, String)>,
}

fn peek_dimensions(path: &Path) -> Result<(u32, u32), String> {
    image::ImageReader::open(path)
        .map_err(|e| format!("cannot open: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("cannot detect format: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("cannot read image header: {e}"))
}

/// Decode to grayscale. The pixel budget is checked on the declared size before any decode.
pub fn load_gray(path: &Path) -> Result<GrayImage, String> {
    let (w, h) = peek_dimensions(path)?;
    if exceeds_pixel_budget(w, h) {
        return Err(format!(
            "{w}x{h} exceeds the {MAX_DECODE_PIXELS}-pixel limit"
        ));
    }
    image::open(path)
        .map(|i| i.to_luma8())
        .map_err(|e| format!("cannot decode: {e}"))
}

/// Stream MD5 in 64 KiB chunks; returns the hex digest and the byte count read.
fn stream_md5(path: &Path) -> io::Result<(String, u64)> {
    let mut reader = io::BufReader::with_capacity(HASH_BUF_BYTES, fs::File::open(path)?);
    let mut hasher = Md5::new();
    let mut buf = [0u8; HASH_BUF_BYTES];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

/// Inspect one file: guard, size filter, decode, hash.
pub fn analyze_file(path: &Path, min_width: u32, min_height: u32) -> FileOutcome {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return FileOutcome::Unreadable("not a regular file".into()),
        Err(e) => return FileOutcome::Unreadable(format!("cannot stat: {e}")),
    };
    let modified = match meta.modified() {
        Ok(t) => t,
        Err(e) => return FileOutcome::Unreadable(format!("no modified time: {e}")),
    };
    let (width, height) = match peek_dimensions(path) {
        Ok(d) => d,
        Err(e) => return FileOutcome::Unreadable(e),
    };
    if exceeds_pixel_budget(width, height) {
        return FileOutcome::TooLarge;
    }
    if width < min_width || height < min_height {
        return FileOutcome::TooSmall;
    }
    let gray = match load_gray(path) {
        Ok(g) => g,
        Err(e) => return FileOutcome::Unreadable(e),
    };
    let (phash, dhash) = (compute_phash(&gray), compute_dhash(&gray));
    let (md5, read) = match stream_md5(path) {
        Ok(r) => r,
        Err(e) => return FileOutcome::Unreadable(format!("cannot hash: {e}")),
    };
    let unchanged = read == meta.len()
        && fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t == modified)
            .unwrap_or(false);
    if !unchanged {
        return FileOutcome::Unreadable("file changed while it was scanned".into());
    }
    FileOutcome::Image(ImageInfo {
        path: path.to_path_buf(),
        width,
        height,
        file_size: meta.len(),
        modified,
        phash,
        dhash,
        md5,
    })
}

/// Collect candidate image paths without following symbolic links or junctions.
fn collect_paths(root: &Path, recursive: bool, report: &mut ScanReport) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if recursive {
        for entry in WalkDir::new(root).follow_links(false) {
            match entry {
                Err(e) => report
                    .unreadable
                    .push((e.path().unwrap_or(root).to_path_buf(), e.to_string())),
                Ok(e) if e.file_type().is_symlink() => report.links += 1,
                Ok(e) if e.file_type().is_file() && has_image_extension(e.path()) => {
                    paths.push(e.into_path())
                }
                Ok(_) => {}
            }
        }
    } else {
        match fs::read_dir(root) {
            Err(e) => report.unreadable.push((root.to_path_buf(), e.to_string())),
            Ok(rd) => {
                for entry in rd {
                    match entry.and_then(|e| e.file_type().map(|t| (e.path(), t))) {
                        Err(e) => report.unreadable.push((root.to_path_buf(), e.to_string())),
                        Ok((_, t)) if t.is_symlink() => report.links += 1,
                        Ok((p, t)) if t.is_file() && has_image_extension(&p) => paths.push(p),
                        Ok(_) => {}
                    }
                }
            }
        }
    }
    paths.sort();
    paths
}

/// Scan `root` for images. Fails only when `root` is not a plain directory.
pub fn scan(root: &Path, opts: &ScanOptions) -> io::Result<ScanReport> {
    let meta = fs::symlink_metadata(root)?;
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "not a plain directory (links are never followed): {}",
                root.display()
            ),
        ));
    }
    let mut report = ScanReport::default();
    let paths = collect_paths(root, opts.recursive, &mut report);
    let outcomes: Vec<FileOutcome> = paths
        .par_iter()
        .map(|p| analyze_file(p, opts.min_width, opts.min_height))
        .collect();
    for (path, outcome) in paths.into_iter().zip(outcomes) {
        match outcome {
            FileOutcome::Image(i) => report.images.push(i),
            FileOutcome::TooSmall => report.too_small += 1,
            FileOutcome::TooLarge => report.too_large += 1,
            FileOutcome::Unreadable(why) => report.unreadable.push((path, why)),
        }
    }
    Ok(report)
}

// ── Duplicate search ────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct FindReport {
    pub groups: Vec<Group>,
    /// Files that could not be re-read for SSIM verification (one entry per file).
    pub errors: Vec<(PathBuf, String)>,
}

enum PairResult {
    Edge(Edge),
    Rejected,
    Unreadable(PathBuf, String),
}

fn compare_pair(images: &[ImageInfo], i: usize, j: usize, ssim_threshold: f64) -> PairResult {
    let (a, b) = (&images[i], &images[j]);
    if !a.md5.is_empty() && a.md5 == b.md5 {
        return PairResult::Edge(Edge {
            a: i,
            b: j,
            score: 1.0,
            high: true,
        });
    }
    let load = |info: &ImageInfo| {
        load_gray(&info.path).map_err(|e| PairResult::Unreadable(info.path.clone(), e))
    };
    let (img_a, img_b) = match (load(a), load(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let score = compute_ssim(&img_a, &img_b, SSIM_SIZE);
    match pair_confidence(score, ssim_threshold, hamming_distance(a.dhash, b.dhash)) {
        None => PairResult::Rejected,
        Some(c) => PairResult::Edge(Edge {
            a: i,
            b: j,
            score,
            high: c == Confidence::High,
        }),
    }
}

/// Find duplicate groups among scanned images.
pub fn find_duplicates(
    images: &[ImageInfo],
    phash_threshold: u32,
    ssim_threshold: f64,
) -> FindReport {
    let n = images.len();
    if n < 2 {
        return FindReport::default();
    }

    let pairs: Vec<(usize, usize)> = (0..n)
        .into_par_iter()
        .flat_map_iter(|i| {
            ((i + 1)..n)
                .filter(move |&j| {
                    let (a, b) = (&images[i], &images[j]);
                    (!a.md5.is_empty() && a.md5 == b.md5)
                        || hamming_distance(a.phash, b.phash) <= phash_threshold
                })
                .map(move |j| (i, j))
        })
        .collect();

    let results: Vec<PairResult> = pairs
        .par_iter()
        .map(|&(i, j)| compare_pair(images, i, j, ssim_threshold))
        .collect();

    let mut edges = Vec::new();
    let mut errors: BTreeMap<PathBuf, String> = BTreeMap::new();
    for r in results {
        match r {
            PairResult::Edge(e) => edges.push(e),
            PairResult::Rejected => {}
            PairResult::Unreadable(p, why) => {
                errors.entry(p).or_insert(why);
            }
        }
    }
    FindReport {
        groups: build_groups(images, &edges),
        errors: errors.into_iter().collect(),
    }
}

// ── Plan, report, apply ─────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Keep,
    Delete,
    Review,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Keep => "KEEP",
            Action::Delete => "DELETE",
            Action::Review => "REVIEW",
        }
    }
}

/// What happens to the copies of `group`. Low-confidence copies are held for review unless included.
pub fn group_action(group: &Group, include_low: bool) -> Action {
    match group.confidence {
        Confidence::High => Action::Delete,
        Confidence::Low if include_low => Action::Delete,
        Confidence::Low => Action::Review,
    }
}

fn tsv_field(s: &str) -> String {
    s.replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Write the tab-separated plan: group, action, width, height, bytes, ssim, confidence, path.
pub fn write_report(groups: &[Group], include_low: bool, out: &mut dyn Write) -> io::Result<()> {
    writeln!(
        out,
        "group\taction\twidth\theight\tbytes\tssim\tconfidence\tpath"
    )?;
    for (n, g) in groups.iter().enumerate() {
        let conf = g.confidence.as_str();
        let k = &g.keeper;
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t\t{}\t{}",
            n + 1,
            Action::Keep.as_str(),
            k.width,
            k.height,
            k.file_size,
            conf,
            tsv_field(&k.path.to_string_lossy())
        )?;
        let action = group_action(g, include_low);
        for d in &g.duplicates {
            writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}\t{:.4}\t{}\t{}",
                n + 1,
                action.as_str(),
                d.info.width,
                d.info.height,
                d.info.file_size,
                d.ssim,
                conf,
                tsv_field(&d.info.path.to_string_lossy())
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub deleted: Vec<PathBuf>,
    /// Copies left in place, with the reason (changed since the scan, keeper changed, gone).
    pub left: Vec<(PathBuf, String)>,
    pub failed: Vec<(PathBuf, String)>,
}

/// Err(reason) when the file on disk no longer matches what the scan recorded.
fn check_unchanged(info: &ImageInfo) -> Result<(), String> {
    let meta = fs::symlink_metadata(&info.path).map_err(|e| format!("cannot stat: {e}"))?;
    if !meta.is_file() {
        return Err("no longer a regular file".into());
    }
    if meta.len() != info.file_size {
        return Err(format!("size {} -> {} bytes", info.file_size, meta.len()));
    }
    match meta.modified() {
        Ok(t) if t == info.modified => Ok(()),
        Ok(_) => Err("modified time differs".into()),
        Err(e) => Err(format!("no modified time: {e}")),
    }
}

/// Delete the DELETE copies through `delete`. Before each delete, the copy and its keeper
/// must still have the scanned size and modified time; otherwise the copy is left in place.
/// A failed delete is recorded and the run goes on.
pub fn apply(
    groups: &[Group],
    include_low: bool,
    delete: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Outcome {
    let mut out = Outcome::default();
    for g in groups {
        if group_action(g, include_low) != Action::Delete {
            continue;
        }
        if let Err(why) = check_unchanged(&g.keeper) {
            for d in &g.duplicates {
                out.left.push((
                    d.info.path.clone(),
                    format!(
                        "keeper changed since scan ({}): {why}",
                        g.keeper.path.display()
                    ),
                ));
            }
            continue;
        }
        for d in &g.duplicates {
            if d.info.path == g.keeper.path {
                continue; // a keeper is never a delete target
            }
            if let Err(why) = check_unchanged(&d.info) {
                out.left
                    .push((d.info.path.clone(), format!("changed since scan: {why}")));
                continue;
            }
            match delete(&d.info.path) {
                Ok(()) => out.deleted.push(d.info.path.clone()),
                Err(e) => out.failed.push((d.info.path.clone(), e.to_string())),
            }
        }
    }
    out
}

mod recycle_bin;

/// Send one file to the Recycle Bin. Never deletes permanently: when Windows cannot recycle
/// the file (for example a network path), the file stays and an error is returned.
pub use recycle_bin::recycle;
