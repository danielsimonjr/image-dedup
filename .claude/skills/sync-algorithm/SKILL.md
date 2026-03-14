---
name: sync-algorithm
description: Synchronize pHash/SSIM algorithm changes between src/lib.rs (PyO3) and src-tauri/src/lib.rs (Tauri) after modifying one crate
disable-model-invocation: true
---

# Sync Algorithm Between Rust Crates

This project has two Rust crates with duplicated core algorithm code:
- `src/lib.rs` — PyO3 extension (legacy Tkinter GUI)
- `src-tauri/src/lib.rs` — Tauri backend (current GUI)

## Workflow

1. First, launch the `crate-sync-checker` agent to identify differences
2. Review the diff report with the user
3. Ask which direction to sync (PyO3 → Tauri, Tauri → PyO3, or selective)
4. Port the algorithm changes while preserving each crate's API surface:
   - Keep PyO3 decorators (`#[pyclass]`, `#[pymethods]`, `#[pyfunction]`) in `src/lib.rs`
   - Keep Tauri decorators (`#[tauri::command]`, serde derives) in `src-tauri/src/lib.rs`
   - Only sync the pure algorithm logic: pHash, SSIM, hamming distance, Union-Find, keeper selection
5. After syncing, run `cargo check` in both crate directories to verify compilation

## Important

- Never replace API-layer code (bindings, decorators, async wrappers)
- Only sync the internal algorithm functions and data structures
- If a difference is intentional (e.g., a Tauri-only optimization), confirm with the user before overwriting
