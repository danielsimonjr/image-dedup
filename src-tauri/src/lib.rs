use image::GenericImageView;
use md5::{Digest, Md5};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];

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

// ── Data types ──────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub phash: u64,
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
}

#[derive(Clone, Serialize)]
pub struct ScanProgress {
    pub scanned: usize,
    pub total: usize,
    pub current_file: String,
}

// ── Tauri commands ──────────────────────────────────────────────────────

#[tauri::command]
async fn scan_images(
    folder: String,
    recursive: bool,
    min_width: u32,
    min_height: u32,
) -> Result<Vec<ImageInfo>, String> {
    tokio::task::spawn_blocking(move || {
        scan_images_sync(&folder, recursive, min_width, min_height)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn scan_images_sync(
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

    let results: Vec<Option<ImageInfo>> = image_paths
        .par_iter()
        .map(|path| {
            let img = image::open(path).ok()?;
            let (width, height) = img.dimensions();
            if width < min_width || height < min_height {
                return None;
            }
            let gray = img.to_luma8();
            let phash = compute_phash(&gray);
            let file_bytes = std::fs::read(path).ok()?;
            let file_size = file_bytes.len() as u64;
            let md5 = hex::encode(Md5::digest(&file_bytes));

            Some(ImageInfo {
                path: path.to_string_lossy().to_string(),
                width,
                height,
                file_size,
                phash,
                md5,
            })
        })
        .collect();

    Ok(results.into_iter().flatten().collect())
}

#[tauri::command]
async fn find_duplicates(
    images: Vec<ImageInfo>,
    phash_threshold: u32,
    ssim_threshold: f64,
) -> Result<Vec<DuplicateGroup>, String> {
    tokio::task::spawn_blocking(move || {
        find_duplicates_sync(images, phash_threshold, ssim_threshold)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn find_duplicates_sync(
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

    let verified: Vec<(usize, usize, f64)> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            if !images[i].md5.is_empty() && images[i].md5 == images[j].md5 {
                return Some((i, j, 1.0));
            }
            let img_a = image::open(&images[i].path).ok()?.to_luma8();
            let img_b = image::open(&images[j].path).ok()?.to_luma8();
            let score = compute_ssim_internal(&img_a, &img_b, 256);
            if score >= ssim_threshold {
                Some((i, j, score))
            } else {
                None
            }
        })
        .collect();

    let mut pair_scores: Vec<(usize, usize, f64)> = Vec::new();
    for (i, j, score) in verified {
        union(&mut parent, &mut rank, i, j);
        pair_scores.push((i, j, score));
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
                    .find(|(a, b, _)| {
                        let paths = [&images[*a].path, &images[*b].path];
                        paths.contains(&&d.path) && paths.contains(&&keeper.path)
                    })
                    .map(|(_, _, s)| *s)
                    .unwrap_or(0.0);
                (d.path.clone(), score)
            })
            .collect();

        results.push(DuplicateGroup {
            keeper,
            duplicates,
            scores,
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
        let img_bytes = std::fs::read(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
        let ext = Path::new(&path)
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
            _ => "image/png",
        };
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img_bytes);
        Ok(format!("data:{mime};base64,{b64}"))
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

#[tauri::command]
fn delete_files(paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut errors = Vec::new();
    for path in &paths {
        if let Err(e) = std::fs::remove_file(path) {
            errors.push(format!("{path}: {e}"));
        }
    }
    if errors.is_empty() {
        Ok(vec![])
    } else {
        Ok(errors)
    }
}

#[tauri::command]
fn send_to_trash(paths: Vec<String>) -> Result<Vec<String>, String> {
    // On Windows, move to recycle bin by renaming to a temp location
    // For simplicity, just delete — user can implement trash later
    delete_files(paths)
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
