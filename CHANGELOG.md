# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed
- `cli/`: replaced the `trash` crate with an own `recycle()` that cannot delete permanently. The `trash` crate reported success and destroyed a file on a network-form path (`\\localhost\C$\...`) that has no Recycle Bin. The new code uses `IFileOperation` with a progress sink. `PreDeleteItem` returns `E_ABORT` when Windows clears `TSF_DELETE_RECYCLE_IF_POSSIBLE`. `PostDeleteItem` must report a new bin item, or the call is an error. A vetoed file stays in place and is reported as failed. Dependencies: `windows` 0.62 and `windows-core` 0.62 replace `trash`.

### Added
- `cli/`: a new pure-Rust crate `imgdedup` (library and binary) with no tauri, pyo3 or tokio dependency. The binary finds duplicate images, writes a tab-separated report and sends the DELETE copies to the Recycle Bin with `--apply` (dry run by default). Options: `--flat`, `--min-width`, `--min-height`, `--phash-threshold`, `--ssim-threshold`, `--include-low-confidence`, `--report`, `--threads`. Low-confidence copies are held as REVIEW. Before each delete the tool checks that the copy and its keeper keep the scanned size and modified time. Symbolic links and junctions are never followed. Exit code 3 means at least one delete failed.
- The `cli/` crate ports the `src-tauri/src/lib.rs` algorithm and is now the reference implementation. Pointing `src-tauri` at it, to end the duplicated code, is a follow-up.
- Tests for the new crate: hashes, SSIM, confidence rule, Union-Find, keeper rule, 50-megapixel guard, scan, report, apply with an injected delete function, the binary, and a copy of `test_images/`.

### CI
- New `cli` job on `windows-latest`: `cargo fmt --check`, `cargo clippy --all-targets -D warnings` and `cargo test` in `cli/`. The `rust` job builds only the root PyO3 crate, so no job compiled `cli/` before.
- Every CI job has a time limit. The `tauri` job's apt step stops a stalled connection after 30 s and retries it, so a slow package mirror fails the step instead of holding the runner for hours.
- `cli/` is now formatted with `cargo fmt`. The Recycle Bin tests take their folder, drive and admin share from the system temp folder, not from a fixed path.

### Security
- Bumped `pyo3` `0.25 → 0.29` in the legacy `dedup_core` Python-extension crate, resolving a HIGH and a MEDIUM advisory against pyo3 `< 0.29`. The bindings (`src/lib.rs`) compile unchanged under 0.29 — `cargo build --release` is clean (two forward-compat `FromPyObject` deprecation notices only). The `dedup_core` module is consumed by `dedup_gui.py`.

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
