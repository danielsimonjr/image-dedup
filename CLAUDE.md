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

### Command-line tool (`cli/`)
```bash
cd cli
cargo test                                   # unit, binary and test_images/ tests
cargo clippy --all-targets -- -D warnings
cargo build --release                        # cli/target/release/imgdedup
```
The GUI crates have no automated Rust tests of the algorithm. The `cli/` tests are the reference tests. Manual GUI testing uses the `test_images/` directory.

## Architecture

The project has three front ends. The `cli/` crate is the reference implementation of the core algorithm:

### Command-line tool (reference implementation)
- **Crate:** `cli/` — package `imgdedup`, library `cli/src/lib.rs` plus binary `cli/src/main.rs`. Pure Rust: no tauri, pyo3 or tokio.
- **Library:** scan, pHash, dHash, SSIM, confidence rule, Union-Find, keeper rule, report and `apply` (the delete function is injected).
- **Binary:** dry run by default; `--apply` sends DELETE copies to the Recycle Bin through `cli/src/recycle_bin.rs` (`IFileOperation` with a veto guard; no permanent-delete fallback). `src-tauri` still uses the `trash` crate, which destroys a file on a path with no Recycle Bin: switching it to `imgdedup::recycle` is part of the follow-up. See the README for options and exit codes.
- **Follow-up:** point `src-tauri` at this crate to end the duplication.

### Tauri v2 (current)
- **Backend:** `src-tauri/src/lib.rs` — Rust, exposes Tauri commands via `#[tauri::command]`
- **Frontend:** `frontend/` — vanilla JS/HTML/CSS, communicates via Tauri IPC (`invoke()`)
- **Config:** `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`

### Legacy Tkinter
- **GUI:** `dedup_gui.py` — tries `import dedup_core` (Rust/PyO3), falls back to `dedup_engine.py` (pure Python)
- **Rust extension:** `src/lib.rs` — PyO3 bindings, built via maturin
- **Python fallback:** `dedup_engine.py` — uses imagehash + scikit-image

### Core Algorithm (reference: `cli/src/lib.rs`; copies in `src-tauri/src/lib.rs` and `src/lib.rs`)
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
- **Three Rust crates with duplicated algorithm code:** `cli/src/lib.rs` (the reference implementation), `src/lib.rs` (PyO3) and `src-tauri/src/lib.rs` (Tauri) implement the same pHash/SSIM/Union-Find logic independently. Change the algorithm in `cli/` first, then mirror the change by hand in the other two. The planned fix is to make `src-tauri` depend on `cli/` and delete its copy.
- **The CLI differs from the other copies in small ways:** it ties the keeper rule to the path as a final tie-break (stable output), skips symbolic links and junctions in flat mode, and computes the pHash with a cosine table (same values, faster).
- **`cli/` keeps its own `.gitignore`:** the root `.gitignore` anchors `/target` to the repository root, so `cli/target` needs the entry in `cli/.gitignore`.
- **Frontend uses global Tauri:** `withGlobalTauri: true` in tauri.conf.json exposes `window.__TAURI__` — JS calls use `window.__TAURI__.core.invoke()`, not an npm import.
- **Lock files are gitignored:** Both `package-lock.json` and `Cargo.lock` are in `.gitignore`.
- **CSP `script-src` is `'self'` only:** Inline `<script>` and `onclick=` handlers are blocked. Wire events from `main.js`. (`style-src` still allows `'unsafe-inline'` because of two `style="width:50px"` attrs in `index.html`.)
- **Path allow-list:** `delete_files` and `get_image_base64` reject any path that wasn't previously seen by `scan_images`. Don't introduce new IPC commands that take raw paths from the renderer without going through `check_allowed`.
