# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`imgdedup` is a command-line duplicate image finder. It uses a two-pass algorithm: perceptual hashing (pHash) for fast candidates, then SSIM (Structural Similarity Index) for verification. It keeps the highest-resolution copy of each duplicate group. The project is a command-line tool only.

## Build & Run Commands

Prerequisite: the Rust toolchain (rustup).

```bash
cargo test                                   # unit, binary and test_images/ tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release                        # target/release/imgdedup
```

The CI `cli` job runs the same three checks on `windows-latest`. See the README for options and exit codes.

## Architecture

The package `imgdedup` has a library (`src/lib.rs`) and a binary (`src/main.rs`). It is pure Rust: no GUI, Python or async runtime.

- **Library:** scan, pHash, dHash, SSIM, confidence rule, Union-Find, keeper rule, report and `apply` (the delete function is injected).
- **Binary:** dry run by default. `--apply` sends DELETE copies to the Recycle Bin through `src/recycle_bin.rs` (`IFileOperation` with a veto guard; no permanent-delete fallback).
- **Tests:** `tests/` holds the integration tests. `tests/test_images.rs` runs the engine on a copy of `test_images/`.

### Core Algorithm
1. **pHash:** Resize to 32x32 grayscale, 2D DCT, top-left 8x8 coefficients, 64-bit hash. Hamming distance at or below the threshold makes a candidate pair.
2. **SSIM:** Resize candidates to 256x256 and compute the structural similarity score. A score at or above the threshold confirms a duplicate.
3. **Grouping:** Union-Find clusters related images. Keeper = most pixels; tie-break by file size, then by path (stable output).

## Build Notes
- LTO is off in the release profile for faster compile times (intentional trade-off).
- The dev profile uses `opt-level = 2`. The DCT and resize loops are too slow at level 0.

## Gotchas
- **Recycle Bin code is Windows-only:** `src/recycle_bin.rs` uses `IFileOperation`. The `recycle` tests run on Windows only, so CI uses `windows-latest`.
- **`test_images/` is gitignored but tracked:** the fixture files were added with `git add -f`. Add a new fixture with `git add -f`.
- **`Cargo.lock` is tracked:** CI runs clippy and the tests with `--locked`. Commit the lockfile with every dependency change.
- **`windows` and `windows-core` move together:** `#[windows::core::implement]` expands to `::windows_core` paths, so `windows-core` is a direct dependency. Its version must equal the `windows-core` that `windows` uses. Dependabot ignores `windows-core`; bump the two crates together by hand.
- **Never run `--apply` on a real folder in tests:** tests use `test_images/` copies and temp folders only.
