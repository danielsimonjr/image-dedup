use image::GenericImageView;
use md5::{Digest, Md5};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];

/// Perceptual hash: resize to 32x32 grayscale, compute DCT, take top-left 8x8,
/// binarize around median → 64-bit hash.
fn compute_phash(img: &image::GrayImage) -> u64 {
    let resized = image::imageops::resize(img, 32, 32, image::imageops::FilterType::Lanczos3);

    // Compute 32x32 DCT (type-II, unnormalized)
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

    // Extract top-left 8x8 (excluding DC component at [0,0])
    let mut low_freq = Vec::with_capacity(64);
    for u in 0..8 {
        for v in 0..8 {
            low_freq.push(dct[u * 32 + v]);
        }
    }

    // Median (excluding DC)
    let mut sorted = low_freq[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    // Build 64-bit hash
    let mut hash: u64 = 0;
    for (i, val) in low_freq.iter().enumerate() {
        if *val > median {
            hash |= 1 << (63 - i);
        }
    }

    hash
}

/// SSIM between two grayscale images resized to target_size x target_size.
/// Returns a value in [-1, 1], where 1 = identical.
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
    let (c1, c2) = (6.5025, 58.5225); // (0.01*255)^2, (0.03*255)^2

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

/// Information about a scanned image, returned to Python.
#[pyclass]
#[derive(Clone)]
struct ImageInfo {
    #[pyo3(get)]
    path: String,
    #[pyo3(get)]
    width: u32,
    #[pyo3(get)]
    height: u32,
    #[pyo3(get)]
    file_size: u64,
    #[pyo3(get)]
    phash: u64,
    #[pyo3(get)]
    md5: String,
}

#[pymethods]
impl ImageInfo {
    #[getter]
    fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    #[getter]
    fn resolution_label(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

/// A group of duplicate images. The keeper is the highest-resolution version.
#[pyclass]
#[derive(Clone)]
struct DuplicateGroup {
    #[pyo3(get)]
    keeper: ImageInfo,
    #[pyo3(get)]
    duplicates: Vec<ImageInfo>,
    #[pyo3(get)]
    scores: Vec<(String, f64)>, // (path, ssim_score)
}

/// Scan a folder for images, computing metadata + perceptual hashes.
/// Uses rayon for parallel I/O and hashing.
#[pyfunction]
#[pyo3(signature = (folder, recursive=true))]
fn scan_images(folder: &str, recursive: bool) -> PyResult<Vec<ImageInfo>> {
    let folder_path = Path::new(folder);
    if !folder_path.is_dir() {
        return Err(PyValueError::new_err(format!(
            "Not a valid directory: {folder}"
        )));
    }

    // Collect image paths
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
            .map_err(|e| PyValueError::new_err(e.to_string()))?
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

    // Process in parallel with rayon
    let results: Vec<Option<ImageInfo>> = image_paths
        .par_iter()
        .map(|path| {
            let img = image::open(path).ok()?;
            let (width, height) = img.dimensions();
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

/// Hamming distance between two 64-bit perceptual hashes.
fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Find duplicate groups using pHash (coarse) + SSIM (verification).
/// Returns groups sorted by total recoverable space (descending).
#[pyfunction]
#[pyo3(signature = (images, phash_threshold=10, ssim_threshold=0.90))]
fn find_duplicates(
    images: Vec<ImageInfo>,
    phash_threshold: u32,
    ssim_threshold: f64,
) -> PyResult<Vec<DuplicateGroup>> {
    let n = images.len();
    if n < 2 {
        return Ok(vec![]);
    }

    // Union-find
    let mut parent: Vec<usize> = (0..n).collect();
    let mut rank: Vec<usize> = vec![0; n];

    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut x = x;
        while parent[x] != x {
            parent[x] = parent[parent[x]]; // path compression
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

    // Collect candidate pairs (pHash pass) in parallel
    let pairs: Vec<(usize, usize)> = (0..n)
        .into_par_iter()
        .flat_map(|i| {
            let mut local_pairs = Vec::new();
            for j in (i + 1)..n {
                // Fast path: exact MD5 match
                if !images[i].md5.is_empty() && images[i].md5 == images[j].md5 {
                    local_pairs.push((i, j));
                    continue;
                }
                // pHash coarse filter
                if hamming_distance(images[i].phash, images[j].phash) <= phash_threshold {
                    local_pairs.push((i, j));
                }
            }
            local_pairs
        })
        .collect();

    // SSIM verification on candidates (parallel)
    let verified: Vec<(usize, usize, f64)> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            // Exact MD5 → skip SSIM
            if !images[i].md5.is_empty() && images[i].md5 == images[j].md5 {
                return Some((i, j, 1.0));
            }

            // Load and compute SSIM
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

    // Build union-find from verified pairs
    let mut pair_scores: Vec<(usize, usize, f64)> = Vec::new();
    for (i, j, score) in verified {
        union(&mut parent, &mut rank, i, j);
        pair_scores.push((i, j, score));
    }

    // Build groups
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

        // Sort by pixel count descending, then file size descending
        let mut members: Vec<&ImageInfo> = indices.iter().map(|&i| &images[i]).collect();
        members.sort_by(|a, b| {
            b.pixel_count()
                .cmp(&a.pixel_count())
                .then(b.file_size.cmp(&a.file_size))
        });

        let keeper = members[0].clone();
        let duplicates: Vec<ImageInfo> = members[1..].iter().map(|m| (*m).clone()).collect();

        // Attach SSIM scores
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

    // Sort by total recoverable space descending
    results.sort_by(|a, b| {
        let sa: u64 = a.duplicates.iter().map(|d| d.file_size).sum();
        let sb: u64 = b.duplicates.iter().map(|d| d.file_size).sum();
        sb.cmp(&sa)
    });

    Ok(results)
}

/// Compute SSIM between two image files. Returns score in [-1, 1].
#[pyfunction]
#[pyo3(signature = (path1, path2, target_size=256))]
fn compute_ssim(path1: &str, path2: &str, target_size: u32) -> PyResult<f64> {
    let img1 = image::open(path1)
        .map_err(|e| PyValueError::new_err(format!("Cannot open {path1}: {e}")))?
        .to_luma8();
    let img2 = image::open(path2)
        .map_err(|e| PyValueError::new_err(format!("Cannot open {path2}: {e}")))?
        .to_luma8();
    Ok(compute_ssim_internal(&img1, &img2, target_size))
}

/// Hamming distance between two perceptual hashes.
#[pyfunction]
fn phash_distance(a: u64, b: u64) -> u32 {
    hamming_distance(a, b)
}

#[pymodule]
fn dedup_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(scan_images, m)?)?;
    m.add_function(wrap_pyfunction!(find_duplicates, m)?)?;
    m.add_function(wrap_pyfunction!(compute_ssim, m)?)?;
    m.add_function(wrap_pyfunction!(phash_distance, m)?)?;
    m.add_class::<ImageInfo>()?;
    m.add_class::<DuplicateGroup>()?;
    Ok(())
}
