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

## License

MIT
