# Image Dedup

A fast duplicate image finder that uses perceptual hashing (pHash) for candidate detection and SSIM (Structural Similarity Index) for verification. Automatically keeps the highest-resolution version of each duplicate group.

## Features

- **Two-pass detection** — pHash for fast matching, SSIM for accurate verification
- **Smart keeper selection** — Keeps highest resolution, tie-breaks by file size
- **Image preview** — Sidebar with Ctrl+Wheel zoom
- **Sortable & filterable results** — Group, action, resolution, file size, SSIM, path
- **Safe deletion** — Sends duplicates to Recycle Bin
- **Configurable** — Minimum image size filter, recursive/flat scan

## Download

Grab the latest standalone `.exe` from the [Releases](https://github.com/danielsimonjr/image-dedup/releases) page — no installation required.

## How It Works

1. **Scan** — Walks the selected directory, loading each image and computing a 64-bit perceptual hash (pHash) and MD5 checksum
2. **Candidate matching** — Compares pHash values using Hamming distance. Pairs within the threshold are candidate duplicates
3. **SSIM verification** — Resizes candidate pairs to 256x256 and computes structural similarity. Pairs above the SSIM threshold are confirmed duplicates
4. **Grouping** — Uses Union-Find to cluster related duplicates. The image with the highest pixel count (resolution) is selected as the keeper

## Build from Source

### Prerequisites

- [Rust](https://rustup.rs/) toolchain
- [Node.js](https://nodejs.org/)

### Build

```bash
npm install
npx tauri build
```

The standalone executable will be at `src-tauri/target/release/image-dedup.exe` and the NSIS installer at `src-tauri/target/release/bundle/nsis/ImageDedup_1.0.0_x64-setup.exe`.

### Development

```bash
npx tauri dev    # Dev server with hot reload
```

## Architecture

The app is built with [Tauri v2](https://v2.tauri.app/) — a Rust backend with a lightweight WebView frontend.

```
src-tauri/src/lib.rs    Rust backend — pHash, SSIM, file scanning, Tauri IPC commands
frontend/main.js        JavaScript UI — table rendering, preview, zoom, filtering
frontend/index.html     HTML structure
frontend/style.css      Styling
```

### Core Algorithm (Rust)

| Step | Method | Parameters |
|------|--------|------------|
| pHash | Resize to 32x32 grayscale → 2D DCT → top-left 8x8 coefficients → 64-bit hash | Hamming distance ≤ 10 |
| SSIM | Resize both to 256x256 → structural similarity comparison | Score ≥ 0.90 |
| Grouping | Union-Find with path compression and union by rank | Keeper = max resolution |

### IPC Commands

| Command | Description |
|---------|-------------|
| `scan_images` | Walk directory, compute pHash + MD5 for each image |
| `find_duplicates` | Compare hashes, verify with SSIM, group results |
| `get_image_base64` | Load image as base64 for preview |
| `send_to_trash` | Move files to Recycle Bin |

## Command-line tool (`imgdedup`)

The `cli/` folder holds `imgdedup`, a pure-Rust command-line version of the duplicate finder. It has no GUI, Tauri or Python dependency. It uses the same algorithm as the desktop app.

### Build

```bash
cd cli
cargo build --release    # binary: cli/target/release/imgdedup
cargo test               # unit and end-to-end tests
```

### Usage

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

### Report

The report is a tab-separated file with the columns `group`, `action`, `width`, `height`, `bytes`, `ssim`, `confidence` and `path`. The `action` column holds one of these values:

- `KEEP`: the tool keeps this file. It has the most pixels in its group. On a tie, the larger file wins.
- `DELETE`: the tool sends this file to the Recycle Bin with `--apply`.
- `REVIEW`: the match has low confidence. The tool keeps the file unless `--include-low-confidence` is set.

A match has high confidence when the files are byte-identical, or when SSIM is 0.98 or higher, or when the dHash distance is 10 or less. A group has high confidence only when every match in it has high confidence.

With `--apply`, the tool checks each copy and its keeper again before it deletes. If the size or the modified time differs from the scan, the tool leaves the copy and lists it in the report. The tool appends a `result` section to the report with the outcome of each delete.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success. |
| 1 | Usage error or I/O error. The message goes to stderr. |
| 3 | At least one delete failed. |

## License

MIT
