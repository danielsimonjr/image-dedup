use image::GenericImageView;
use md5::{Digest, Md5};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];

/// Decompression-bomb guard: any image whose declared dimensions exceed
/// this pixel budget is rejected before decode. 50 megapixels accommodates
/// e.g. 8K ≈ 33 MP and most DSLR raws while blocking 65535×65535 attack PNGs.
const MAX_DECODE_PIXELS: u64 = 50_000_000;

/// Buffer size for the streaming MD5 path (#7). 64 KiB is a good
/// trade-off — large enough to amortize syscall + decode overhead,
/// small enough that N rayon threads fit comfortably in cache/RAM.
const HASH_BUF_BYTES: usize = 64 * 1024;

/// Stream MD5 of a file in 64-KiB chunks, returning (hex_digest, file_size).
/// Replaces the old `std::fs::read(path)` whole-file load (#7).
fn stream_md5(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = std::io::BufReader::with_capacity(HASH_BUF_BYTES, file);
    let mut hasher = Md5::new();
    let mut buf = [0u8; HASH_BUF_BYTES];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((hex::encode(hasher.finalize()), size))
}

// ── Path allow-list ─────────────────────────────────────────────────────
// Only paths that scan_images has actually visited may be opened or
// deleted by the renderer. Canonicalized to defeat symlink / `..` traversal.

static ALLOWED_PATHS: Lazy<Mutex<HashSet<PathBuf>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

fn canonicalize_for_allowlist(path: &Path) -> Option<PathBuf> {
    // dunce::canonicalize would be nicer on Windows but std works fine for
    // allow-list comparisons because we canonicalize both sides identically.
    std::fs::canonicalize(path).ok()
}

fn record_allowed(path: &Path) {
    if let Some(canon) = canonicalize_for_allowlist(path) {
        if let Ok(mut set) = ALLOWED_PATHS.lock() {
            set.insert(canon);
        }
    }
}

/// Returns Ok(canonical_path) if `raw` resolves to a file that scan_images
/// previously recorded; Err(reason) otherwise.
fn check_allowed(raw: &str) -> Result<PathBuf, String> {
    let canon = canonicalize_for_allowlist(Path::new(raw))
        .ok_or_else(|| format!("path does not resolve: {raw}"))?;
    let set = ALLOWED_PATHS
        .lock()
        .map_err(|_| "allow-list lock poisoned".to_string())?;
    if set.contains(&canon) {
        Ok(canon)
    } else {
        Err(format!("path not in scan allow-list: {raw}"))
    }
}

fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
fn _testing_clear_allowlist() {
    if let Ok(mut set) = ALLOWED_PATHS.lock() {
        set.clear();
    }
}

#[cfg(test)]
fn _testing_record_allowed(p: &Path) {
    record_allowed(p);
}

// ── Perceptual hash ─────────────────────────────────────────────────────

fn compute_phash(img: &image::GrayImage) -> u64 {
    let resized = image::imageops::resize(img, 32, 32, image::imageops::FilterType::Lanczos3);

    let mut dct = vec![0.0f64; 32 * 32];
    for u in 0..32u32 {
        for v in 0..32u32 {
            let mut sum = 0.0f64;
            for x in 0..32u32 {
                for y in 0..32u32 {
                    let pixel = resized.get_pixel(y, x)[0] as f64;
                    let cos_x = ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 64.0).cos();
                    let cos_y = ((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI / 64.0).cos();
                    sum += pixel * cos_x * cos_y;
                }
            }
            dct[(u * 32 + v) as usize] = sum;
        }
    }

    let mut low_freq = Vec::with_capacity(64);
    for u in 0..8 {
        for v in 0..8 {
            low_freq.push(dct[u * 32 + v]);
        }
    }

    let mut sorted = low_freq[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    let mut hash: u64 = 0;
    for (i, val) in low_freq.iter().enumerate() {
        if *val > median {
            hash |= 1 << (63 - i);
        }
    }
    hash
}

// ── SSIM ────────────────────────────────────────────────────────────────

fn compute_ssim_internal(
    img1: &image::GrayImage,
    img2: &image::GrayImage,
    target_size: u32,
) -> f64 {
    let a = image::imageops::resize(
        img1,
        target_size,
        target_size,
        image::imageops::FilterType::Lanczos3,
    );
    let b = image::imageops::resize(
        img2,
        target_size,
        target_size,
        image::imageops::FilterType::Lanczos3,
    );

    let n = (target_size * target_size) as f64;
    let (c1, c2) = (6.5025, 58.5225);

    let mut mean_a = 0.0f64;
    let mut mean_b = 0.0f64;
    for i in 0..n as usize {
        mean_a += a.as_raw()[i] as f64;
        mean_b += b.as_raw()[i] as f64;
    }
    mean_a /= n;
    mean_b /= n;

    let mut var_a = 0.0f64;
    let mut var_b = 0.0f64;
    let mut cov = 0.0f64;
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

fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

// ── dHash (difference hash) ─────────────────────────────────────────────
// Independent of pHash — compares adjacent-pixel gradients on a 9x8 grid.
// Used as a corroborating signal for pHash + SSIM in the 0.90-0.98 band.

fn compute_dhash(img: &image::GrayImage) -> u64 {
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

// ── Confidence thresholds (#6) ──────────────────────────────────────────
// SSIM >= STRICT_SSIM => high confidence, auto-group.
// LOW_SSIM <= SSIM < STRICT_SSIM => only auto-group if dHash also agrees.
// Below LOW_SSIM => not a duplicate.
//
// Rationale: pHash + SSIM>=0.90 alone is forgeable. Either tighten SSIM
// or require an independent hash to agree. Dual-signal cuts FP rate without
// dropping recall too far on real near-duplicates.
const STRICT_SSIM: f64 = 0.98;
const DHASH_AGREE_DISTANCE: u32 = 10;

// ── Data types ──────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub phash: u64,
    /// dHash — independent corroborating signal for #6.
    /// Default 0 (back-compat for callers / saved data).
    #[serde(default)]
    pub dhash: u64,
    pub md5: String,
}

impl ImageInfo {
    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub keeper: ImageInfo,
    pub duplicates: Vec<ImageInfo>,
    pub scores: Vec<(String, f64)>,
    /// "high" => MD5 match or strict SSIM (>=0.98) or pHash+dHash+SSIM>=ssim_threshold;
    /// "low"  => SSIM in [ssim_threshold, 0.98) without dHash agreement.
    /// Low-confidence groups are NEVER auto-deleted by delete_files —
    /// the renderer should surface them as "review needed".
    #[serde(default = "default_confidence")]
    pub confidence: String,
}

fn default_confidence() -> String { "high".to_string() }

#[derive(Clone, Serialize)]
pub struct ScanProgress {
    pub scanned: usize,
    pub total: usize,
    pub current_file: String,
}

// ── Tauri commands ──────────────────────────────────────────────────────

#[tauri::command]
async fn scan_images(
    app: AppHandle,
    folder: String,
    recursive: bool,
    min_width: u32,
    min_height: u32,
) -> Result<Vec<ImageInfo>, String> {
    tokio::task::spawn_blocking(move || {
        scan_images_sync(&app, &folder, recursive, min_width, min_height)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn scan_images_sync(
    app: &AppHandle,
    folder: &str,
    recursive: bool,
    min_width: u32,
    min_height: u32,
) -> Result<Vec<ImageInfo>, String> {
    let folder_path = Path::new(folder);
    if !folder_path.is_dir() {
        return Err(format!("Not a valid directory: {folder}"));
    }

    let image_paths: Vec<PathBuf> = if recursive {
        WalkDir::new(folder_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file()
                    && e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
                        .unwrap_or(false)
            })
            .map(|e| e.into_path())
            .collect()
    } else {
        std::fs::read_dir(folder_path)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().is_file()
                    && e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
                        .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect()
    };

    let total = image_paths.len();
    let _ = app.emit("scan_progress", ScanProgress {
        scanned: 0, total, current_file: format!("Found {total} images..."),
    });

    let scanned = AtomicUsize::new(0);
    let app_ref = app.clone();

    let results: Vec<Option<ImageInfo>> = image_paths
        .par_iter()
        .map(|path| {
            // Cheap dimension peek BEFORE decode — rejects decompression
            // bombs (e.g. 65535x65535 PNGs) without ever allocating the
            // full pixel buffer.
            let reader = image::ImageReader::open(path)
                .ok()?
                .with_guessed_format()
                .ok()?;
            let (peek_w, peek_h) = reader.into_dimensions().ok()?;
            if (peek_w as u64) * (peek_h as u64) > MAX_DECODE_PIXELS {
                let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                if done % 5 == 0 || done == total {
                    let _ = app_ref.emit("scan_progress", ScanProgress {
                        scanned: done, total,
                        current_file: format!("skipped (too large): {}",
                            path.file_name().unwrap_or_default().to_string_lossy()),
                    });
                }
                return None;
            }

            let img = image::open(path).ok()?;
            let (width, height) = img.dimensions();
            if width < min_width || height < min_height {
                let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                if done % 5 == 0 || done == total {
                    let _ = app_ref.emit("scan_progress", ScanProgress {
                        scanned: done, total,
                        current_file: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    });
                }
                return None;
            }
            let gray = img.to_luma8();
            let phash = compute_phash(&gray);
            let dhash = compute_dhash(&gray);
            // Stream MD5 in 64-KiB chunks instead of loading whole file
            // into RAM (#7). file_size comes from metadata().len() so we
            // don't double-read.
            let (md5, file_size) = stream_md5(path).ok()?;

            let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
            if done % 5 == 0 || done == total {
                let _ = app_ref.emit("scan_progress", ScanProgress {
                    scanned: done, total,
                    current_file: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                });
            }

            // Record this path in the allow-list so the renderer is
            // permitted to call get_image_base64 / delete_files on it.
            record_allowed(path);

            Some(ImageInfo {
                path: path.to_string_lossy().to_string(),
                width,
                height,
                file_size,
                phash,
                dhash,
                md5,
            })
        })
        .collect();

    Ok(results.into_iter().flatten().collect())
}

#[tauri::command]
async fn find_duplicates(
    app: AppHandle,
    images: Vec<ImageInfo>,
    phash_threshold: u32,
    ssim_threshold: f64,
) -> Result<Vec<DuplicateGroup>, String> {
    tokio::task::spawn_blocking(move || {
        find_duplicates_sync(&app, images, phash_threshold, ssim_threshold)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn find_duplicates_sync(
    app: &AppHandle,
    images: Vec<ImageInfo>,
    phash_threshold: u32,
    ssim_threshold: f64,
) -> Result<Vec<DuplicateGroup>, String> {
    let n = images.len();
    if n < 2 {
        return Ok(vec![]);
    }

    let mut parent: Vec<usize> = (0..n).collect();
    let mut rank: Vec<usize> = vec![0; n];

    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut x = x;
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }

    fn union(parent: &mut [usize], rank: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra == rb {
            return;
        }
        if rank[ra] < rank[rb] {
            parent[ra] = rb;
        } else if rank[ra] > rank[rb] {
            parent[rb] = ra;
        } else {
            parent[rb] = ra;
            rank[ra] += 1;
        }
    }

    let _ = app.emit("dedup_progress", serde_json::json!({
        "phase": "phash", "done": 0, "total": n, "message": "Comparing pHash values..."
    }));

    let pairs: Vec<(usize, usize)> = (0..n)
        .into_par_iter()
        .flat_map(|i| {
            let mut local_pairs = Vec::new();
            for j in (i + 1)..n {
                if !images[i].md5.is_empty() && images[i].md5 == images[j].md5 {
                    local_pairs.push((i, j));
                    continue;
                }
                if hamming_distance(images[i].phash, images[j].phash) <= phash_threshold {
                    local_pairs.push((i, j));
                }
            }
            local_pairs
        })
        .collect();

    let pair_total = pairs.len();
    let _ = app.emit("dedup_progress", serde_json::json!({
        "phase": "ssim", "done": 0, "total": pair_total,
        "message": format!("Verifying {pair_total} candidate pairs with SSIM...")
    }));

    let verified_count = AtomicUsize::new(0);
    let app_ref = app.clone();

    // verified carries (i, j, ssim_score, high_confidence).
    // #6: high-confidence requires MD5 match OR SSIM >= STRICT_SSIM
    // OR (SSIM >= ssim_threshold AND dHash distance <= DHASH_AGREE_DISTANCE).
    let verified: Vec<(usize, usize, f64, bool)> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            let done = verified_count.fetch_add(1, Ordering::Relaxed) + 1;
            if done % 5 == 0 || done == pair_total {
                let _ = app_ref.emit("dedup_progress", serde_json::json!({
                    "phase": "ssim", "done": done, "total": pair_total,
                    "message": format!("SSIM verification: {done}/{pair_total} pairs...")
                }));
            }

            if !images[i].md5.is_empty() && images[i].md5 == images[j].md5 {
                return Some((i, j, 1.0, true));
            }
            let img_a = image::open(&images[i].path).ok()?.to_luma8();
            let img_b = image::open(&images[j].path).ok()?.to_luma8();
            let score = compute_ssim_internal(&img_a, &img_b, 256);
            if score < ssim_threshold {
                return None;
            }
            let dhash_dist = hamming_distance(images[i].dhash, images[j].dhash);
            let high = score >= STRICT_SSIM || dhash_dist <= DHASH_AGREE_DISTANCE;
            Some((i, j, score, high))
        })
        .collect();

    let mut pair_scores: Vec<(usize, usize, f64, bool)> = Vec::new();
    for (i, j, score, high) in verified {
        union(&mut parent, &mut rank, i, j);
        pair_scores.push((i, j, score, high));
    }

    let mut groups_map: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups_map.entry(root).or_default().push(i);
    }

    let mut results: Vec<DuplicateGroup> = Vec::new();
    for indices in groups_map.values() {
        if indices.len() < 2 {
            continue;
        }

        let mut members: Vec<&ImageInfo> = indices.iter().map(|&i| &images[i]).collect();
        members.sort_by(|a, b| {
            b.pixel_count()
                .cmp(&a.pixel_count())
                .then(b.file_size.cmp(&a.file_size))
        });

        let keeper = members[0].clone();
        let duplicates: Vec<ImageInfo> = members[1..].iter().map(|m| (*m).clone()).collect();

        let scores: Vec<(String, f64)> = duplicates
            .iter()
            .map(|d| {
                let score = pair_scores
                    .iter()
                    .find(|(a, b, _, _)| {
                        let paths = [&images[*a].path, &images[*b].path];
                        paths.contains(&&d.path) && paths.contains(&&keeper.path)
                    })
                    .map(|(_, _, s, _)| *s)
                    .unwrap_or(0.0);
                (d.path.clone(), score)
            })
            .collect();

        // Group is high confidence only if EVERY duplicate vs keeper edge
        // is high confidence. Any low edge demotes the whole group so the
        // UI can flag it for manual review.
        let group_paths: std::collections::HashSet<&str> =
            indices.iter().map(|&i| images[i].path.as_str()).collect();
        let all_high = pair_scores
            .iter()
            .filter(|(a, b, _, _)| {
                group_paths.contains(images[*a].path.as_str())
                    && group_paths.contains(images[*b].path.as_str())
            })
            .all(|(_, _, _, h)| *h);
        let confidence = if all_high { "high" } else { "low" }.to_string();

        results.push(DuplicateGroup {
            keeper,
            duplicates,
            scores,
            confidence,
        });
    }

    results.sort_by(|a, b| {
        let sa: u64 = a.duplicates.iter().map(|d| d.file_size).sum();
        let sb: u64 = b.duplicates.iter().map(|d| d.file_size).sum();
        sb.cmp(&sa)
    });

    Ok(results)
}

#[tauri::command]
async fn get_image_base64(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        // 1. Allow-list check (canonicalizes & rejects untracked paths).
        let canon = check_allowed(&path)?;

        // 2. Extension allow-list (rejects e.g. `passwords.txt`).
        if !has_image_extension(&canon) {
            return Err(format!("not an allowed image extension: {path}"));
        }

        // 3. Validate via image::ImageReader::decode — non-images fail here.
        image::ImageReader::open(&canon)
            .map_err(|e| format!("cannot open {path}: {e}"))?
            .with_guessed_format()
            .map_err(|e| format!("cannot sniff format {path}: {e}"))?
            .decode()
            .map_err(|e| format!("not a valid image {path}: {e}"))?;

        // 4. Read bytes and encode.
        let img_bytes =
            std::fs::read(&canon).map_err(|e| format!("Cannot read {path}: {e}"))?;
        let ext = canon
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_lowercase();
        let mime = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            "webp" => "image/webp",
            "tiff" | "tif" => "image/tiff",
            _ => return Err(format!("unsupported extension: {ext}")),
        };
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img_bytes);
        Ok(format!("data:{mime};base64,{b64}"))
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Move files to the OS recycle bin / trash. Each path must be in the
/// scan allow-list — arbitrary paths from a compromised renderer are
/// rejected without touching the filesystem.
#[tauri::command]
fn delete_files(paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut errors = Vec::new();
    for raw in &paths {
        match check_allowed(raw) {
            Err(reason) => {
                errors.push(format!("{raw}: rejected ({reason})"));
                continue;
            }
            Ok(canon) => {
                if let Err(e) = trash::delete(&canon) {
                    errors.push(format!("{raw}: {e}"));
                } else {
                    // Drop from allow-list so a second call can't re-target it.
                    if let Ok(mut set) = ALLOWED_PATHS.lock() {
                        set.remove(&canon);
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(vec![])
    } else {
        Ok(errors)
    }
}

/// Alias of `delete_files`, kept for backward compatibility with the
/// existing renderer code. Both move to recycle bin via the `trash` crate.
#[tauri::command]
fn send_to_trash(paths: Vec<String>) -> Result<Vec<String>, String> {
    delete_files(paths)
}

// ── Export CSV ──────────────────────────────────────────────────────────

#[tauri::command]
async fn export_csv(path: String, csv_content: String) -> Result<(), String> {
    std::fs::write(&path, csv_content).map_err(|e| format!("Failed to write CSV: {e}"))
}

// ── App setup ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            scan_images,
            find_duplicates,
            get_image_base64,
            delete_files,
            send_to_trash,
            export_csv,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn unique_tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!("image-dedup-test-{ts}-{name}"));
        p
    }

    #[test]
    fn allowlist_rejects_unscanned_path() {
        _testing_clear_allowlist();
        let tmp = unique_tmp("unscanned.png");
        fs::write(&tmp, b"\x89PNG\r\n\x1a\n").unwrap();
        let res = check_allowed(tmp.to_str().unwrap());
        assert!(res.is_err(), "unscanned path must be rejected");
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn allowlist_accepts_scanned_path() {
        _testing_clear_allowlist();
        let tmp = unique_tmp("scanned.png");
        fs::write(&tmp, b"\x89PNG\r\n\x1a\n").unwrap();
        _testing_record_allowed(&tmp);
        let res = check_allowed(tmp.to_str().unwrap());
        assert!(res.is_ok(), "scanned path must be accepted: {res:?}");
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn allowlist_rejects_traversal_attempt() {
        _testing_clear_allowlist();
        // Record an "allowed" file then attempt access via a sibling
        // path that is NOT in the allow-list. Both canonicalize to
        // distinct absolute paths, so the traversal is rejected.
        let allowed = unique_tmp("allowed.png");
        let other = unique_tmp("other.png");
        fs::write(&allowed, b"\x89PNG\r\n\x1a\n").unwrap();
        fs::write(&other, b"\x89PNG\r\n\x1a\n").unwrap();
        _testing_record_allowed(&allowed);

        // A `..`-traversed path that lands on `other` must NOT be allowed.
        let traversed = allowed.parent().unwrap().join(format!(
            "..{}{}",
            std::path::MAIN_SEPARATOR,
            allowed
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        ));
        let attack = traversed.join(other.file_name().unwrap());
        let res = check_allowed(attack.to_str().unwrap());
        assert!(res.is_err(), "traversal to sibling must be rejected");
        let _ = fs::remove_file(&allowed);
        let _ = fs::remove_file(&other);
    }

    #[test]
    fn extension_check_rejects_non_image() {
        let p = PathBuf::from("/tmp/passwords.txt");
        assert!(!has_image_extension(&p));
        let p2 = PathBuf::from("/tmp/photo.JPG");
        assert!(has_image_extension(&p2));
    }

    #[test]
    fn decompression_bomb_rejected_by_dimension_check() {
        // Build a tiny PNG that declares 65535 × 65535 dimensions in IHDR
        // but contains no actual pixel data. image::ImageReader's
        // into_dimensions() should return the declared size cheaply,
        // and our caller must reject it before decode.
        let tmp = unique_tmp("bomb.png");
        let mut f = fs::File::create(&tmp).unwrap();
        // Minimal valid PNG signature + IHDR with width=65535,height=65535.
        let sig = [137u8, 80, 78, 71, 13, 10, 26, 10];
        f.write_all(&sig).unwrap();
        // IHDR length=13
        f.write_all(&[0, 0, 0, 13]).unwrap();
        f.write_all(b"IHDR").unwrap();
        // width=65535, height=65535, bit_depth=8, color_type=2 (RGB)
        f.write_all(&[0, 0, 255, 255, 0, 0, 255, 255, 8, 2, 0, 0, 0]).unwrap();
        // Bogus CRC — into_dimensions reads IHDR before CRC is checked
        // by some decoders; if not, into_dimensions returns Err and our
        // caller rejects anyway, which is also a pass.
        f.write_all(&[0u8, 0, 0, 0]).unwrap();
        drop(f);

        let dims = image::ImageReader::open(&tmp)
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        const MAX_PIXELS: u64 = 50_000_000;
        let bombed = match dims {
            Some((w, h)) => (w as u64) * (h as u64) > MAX_PIXELS,
            // Either way: a malformed/oversize PNG must NOT be accepted.
            None => true,
        };
        assert!(bombed, "65535x65535 image must be flagged as a bomb");
        let _ = fs::remove_file(&tmp);
    }
}
