# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Security

- **CRITICAL** (`#1`, `#2`, `#3`) — Renderer-supplied paths are no longer
  trusted blindly. `scan_images` now records every visited path in a global
  allow-list (`Mutex<HashSet<PathBuf>>` of canonicalized paths); `delete_files`
  and `get_image_base64` reject any path not in that allow-list, preventing
  arbitrary-file read/delete via crafted IPC payloads. `delete_files` now uses
  the `trash` crate (real OS recycle-bin) instead of `std::fs::remove_file`.
  `get_image_base64` validates content via `image::ImageReader::with_guessed_format`
  + `decode()` before returning bytes, and rejects any extension not in the
  image allow-list. CSP `script-src` no longer includes `'unsafe-inline'`
  (`style-src` retains it for the two unavoidable inline `style="width:50px"`
  attributes).
- **IMPORTANT** (`#4`) — `fs:allow-read` removed from baseline capabilities;
  the renderer now requests fs scope at runtime via the `fs` plugin scope API
  after the user picks a folder, scoped to that folder only.
- **IMPORTANT** (`#5`) — Decompression-bomb DoS fixed on both Rust and Python
  sides. Rust now calls `image::ImageReader::open(path)?.into_dimensions()?`
  before decoding, rejecting any image with `w * h > 50_000_000`. Python sets
  `Image.MAX_IMAGE_PIXELS = 50_000_000` and wraps `img.load()` in
  `try/except DecompressionBombError`.
- **IMPORTANT** (`#6`) — pHash + SSIM duplicate detection hardened. When MD5s
  differ, a pair is only auto-grouped if SSIM ≥ 0.98 OR if the dHash check
  agrees. Pairs in the 0.90 – 0.98 SSIM band are surfaced as
  "low-confidence" and never auto-deleted by `delete_files`.
- **IMPORTANT** (`#7`) — MD5 is now streamed via `Md5::new()` + 64 KiB read
  loop (Rust) and `hashlib.md5()` + chunked read (Python) instead of loading
  whole files into RAM. `file_size` is read via `metadata().len()` /
  `Path.stat().st_size`.
