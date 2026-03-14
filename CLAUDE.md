# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Image Dedup is a duplicate image finder that uses a two-pass detection algorithm: perceptual hashing (pHash) for fast candidate detection, then SSIM (Structural Similarity Index) for verification. It keeps the highest-resolution version of each duplicate group.

## Build & Run Commands

### Prerequisites
- Rust toolchain (rustup) — required for both Tauri and maturin builds
- Node.js — required for Tauri CLI (`@tauri-apps/cli`)

### Tauri v2 (active development)
```bash
npm install              # Install Tauri CLI
npm run tauri dev        # Dev server with hot reload
npm run tauri build      # Release build → dist/ (NSIS installer)
```

### Legacy Tkinter GUI
```bash
pip install maturin
maturin develop          # Build Rust extension (dedup_core)
python dedup_gui.py      # Run Tkinter GUI
```

### No test suite exists — manual testing uses `test_images/` directory.

## Architecture

The project has two parallel GUI systems sharing the same core algorithm:

### Tauri v2 (current)
- **Backend:** `src-tauri/src/lib.rs` — Rust, exposes Tauri commands via `#[tauri::command]`
- **Frontend:** `frontend/` — vanilla JS/HTML/CSS, communicates via Tauri IPC (`invoke()`)
- **Config:** `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`

### Legacy Tkinter
- **GUI:** `dedup_gui.py` — tries `import dedup_core` (Rust/PyO3), falls back to `dedup_engine.py` (pure Python)
- **Rust extension:** `src/lib.rs` — PyO3 bindings, built via maturin
- **Python fallback:** `dedup_engine.py` — uses imagehash + scikit-image

### Core Algorithm (in both `src-tauri/src/lib.rs` and `src/lib.rs`)
1. **pHash:** Resize to 32x32 grayscale → 2D DCT → top-left 8x8 coefficients → 64-bit hash. Hamming distance ≤ threshold = candidate pair.
2. **SSIM:** Resize candidates to 256x256 → compute structural similarity score. Score ≥ threshold = confirmed duplicate.
3. **Grouping:** Union-Find to cluster related images. Keeper = highest resolution, tie-break by file size.

### Tauri IPC Commands
- `scan_images(folder, recursive, min_width, min_height)` → `Vec<ImageInfo>`
- `find_duplicates(images, phash_threshold, ssim_threshold)` → `Vec<DuplicateGroup>`
- `get_image_base64(path)` → base64 string for preview
- `send_to_trash(paths)` → trash files via OS API

CPU-bound work uses `tokio::task::spawn_blocking()` to avoid blocking the Tauri event loop.

## Key Data Structures (Rust)

```rust
ImageInfo { path, width, height, file_size, phash: u64, md5 }
DuplicateGroup { keeper: ImageInfo, duplicates: Vec<ImageInfo>, scores: Vec<(String, f64)> }
```

## Build Notes
- LTO is disabled in release profiles for faster compile times (intentional trade-off)
- Tauri targets NSIS installer for Windows
- Root `Cargo.toml` builds the PyO3 extension (`cdylib`); `src-tauri/Cargo.toml` builds the Tauri app

## Gotchas
- **Two separate Rust crates with duplicated algorithm code:** `src/lib.rs` (PyO3) and `src-tauri/src/lib.rs` (Tauri) implement the same pHash/SSIM/Union-Find logic independently. Changes to the algorithm must be mirrored manually.
- **Frontend uses global Tauri:** `withGlobalTauri: true` in tauri.conf.json exposes `window.__TAURI__` — JS calls use `window.__TAURI__.core.invoke()`, not an npm import.
- **Lock files are gitignored:** Both `package-lock.json` and `Cargo.lock` are in `.gitignore`.
- **CSP allows `unsafe-inline`:** The security policy permits inline scripts and styles — be aware when adding new script/style sources.
