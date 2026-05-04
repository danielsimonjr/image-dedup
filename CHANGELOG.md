# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [1.1.0] - 2026-05-01

### Added

- **Surface low-confidence flag in renderer** (`frontend/main.js`,
  `frontend/style.css`). The pHash/SSIM/dHash hardening landed in
  `60eebc3` already produced a `confidence: "high"|"low"` field on
  each `DuplicateGroup`, but the table didn't read it. Low-confidence
  rows now render with an amber border, an `⚠ review` badge on the
  group's first row, are NOT pre-checked (user must opt in per-image),
  and the bulk-delete confirm dialog adds a warning line listing how
  many low-confidence matches are queued. Backwards-compatible: older
  builds that don't emit `confidence` (still possible if running
  against an older backend) fall back to "high" via `serde(default)`,
  preserving prior behavior.

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
- **IMPORTANT** (`#4`) — `fs:allow-read` (and unused `fs:default`) removed
  from `capabilities/default.json`. The renderer never directly invoked the
  fs plugin — all file reads go through our own `get_image_base64` Rust
  command, which now enforces the path allow-list — so removing the
  capability outright is safer than the originally proposed runtime scope.
  Any future frontend that needs raw fs access must add a fresh capability
  with a runtime-set scope limited to the user-chosen folder.
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
