# Image Dedup

`imgdedup` is a command-line tool that finds duplicate images. It uses perceptual hashing (pHash) to find candidates. It uses SSIM (Structural Similarity Index) to verify them. It keeps the highest-resolution copy of each group.

## Features

- **Two-pass detection:** pHash finds candidates fast. SSIM confirms them.
- **Keeper rule:** The copy with the most pixels wins. On a tie, the larger file wins.
- **Dry run by default:** The tool writes a report and deletes nothing until you pass `--apply`.
- **Safe deletion:** `--apply` sends copies to the Recycle Bin. The tool never deletes permanently.
- **Configurable:** Set a minimum image size, the thresholds, flat or recursive scan, and the thread count.

## Install

[Rust](https://rustup.rs/) is the only prerequisite.

```bash
cargo install --git https://github.com/danielsimonjr/image-dedup
```

To build from a checkout:

```bash
cargo build --release    # binary: target/release/imgdedup
cargo test               # unit and end-to-end tests
```

The Recycle Bin code uses the Windows shell. The tool builds on other systems, but `--apply` works on Windows only.

## How It Works

1. **Scan:** The tool walks the root folder. It computes a 64-bit pHash, a dHash and an MD5 checksum for each image.
2. **Candidate matching:** The tool compares pHash values with Hamming distance. A pair within the threshold is a candidate. Byte-identical files are candidates too.
3. **SSIM verification:** The tool resizes each candidate pair to 256x256 and computes SSIM. A pair at or above the SSIM threshold is a duplicate.
4. **Grouping:** The tool clusters related duplicates with Union-Find. The image with the most pixels is the keeper.

| Step | Method | Default |
|------|--------|---------|
| pHash | Resize to 32x32 grayscale, 2D DCT, top-left 8x8 coefficients, 64-bit hash | Hamming distance at most 10 |
| SSIM | Resize both images to 256x256, compare structure | Score at least 0.90 |
| Grouping | Union-Find with path compression and union by rank | Keeper = most pixels |

## Usage

```text
imgdedup <ROOT> [--apply] [--flat] [--min-width N] [--min-height N]
                [--phash-threshold 10] [--ssim-threshold 0.90] [--include-low-confidence]
                [--report FILE] [--threads N]
```

| Option | Effect |
|--------|--------|
| (none) | Dry run. The tool scans, writes the report and deletes nothing. |
| `--apply` | Sends the DELETE copies to the Recycle Bin. The tool never deletes permanently. |
| `--flat` | Scans only ROOT. The default scans ROOT and all subfolders. |
| `--min-width N`, `--min-height N` | Skips images smaller than N pixels. |
| `--phash-threshold N` | Sets the pHash Hamming-distance limit (0 to 64). Default: 10. |
| `--ssim-threshold F` | Sets the SSIM limit (above 0, at most 1). Default: 0.90. |
| `--include-low-confidence` | Also deletes low-confidence copies. Default: the tool holds them as REVIEW. |
| `--report FILE` | Sets the report path. Default: `imgdedup-report.tsv`. |
| `--threads N` | Sets the worker thread count. Default: the CPU count, at most 8. |

The tool never follows symbolic links or junctions. It skips images that declare more than 50 megapixels.

## Report

The report is a tab-separated file with the columns `group`, `action`, `width`, `height`, `bytes`, `ssim`, `confidence` and `path`. The `action` column holds one of these values:

- `KEEP`: the tool keeps this file. It has the most pixels in its group. On a tie, the larger file wins.
- `DELETE`: the tool sends this file to the Recycle Bin with `--apply`.
- `REVIEW`: the match has low confidence. The tool keeps the file unless `--include-low-confidence` is set.

A match has high confidence when the files are byte-identical, or when SSIM is 0.98 or higher, or when the dHash distance is 10 or less. A group has high confidence only when every match in it has high confidence.

## Recycle Bin

The tool uses the Windows shell `IFileOperation` call with a guard. If Windows cannot send a file to the Recycle Bin (for example, a network path), the tool vetoes the delete. The file stays in place and the report lists it as `FAILED` with the reason. There is no permanent-delete fallback.

With `--apply`, the tool checks each copy and its keeper again before it deletes. If the size or the modified time differs from the scan, the tool leaves the copy and lists it in the report. The tool appends a `result` section to the report with the outcome of each delete.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success. |
| 1 | Usage error or I/O error. The message goes to stderr. |
| 3 | At least one delete failed. |

## License

MIT
