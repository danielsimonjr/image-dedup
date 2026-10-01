# TODO

- [x] 2026-10-01 10:27 Rust CLI (`cli/`, package `imgdedup`): a pure-Rust library plus binary that
      ports the reference algorithm from `src-tauri/src/lib.rs` (MD5 fast path, pHash, dHash,
      256x256 SSIM, confidence rule, 50-megapixel decode guard, Union-Find, keeper = highest pixel
      count then larger file). Dry run by default with a TSV report; `--apply` sends DELETE copies
      to the Recycle Bin; low-confidence matches are REVIEW and are never deleted without
      `--include-low-confidence`. Test-first, clippy clean, end-to-end on a copy of `test_images/`.
      Done: 58 tests pass, clippy `-D warnings` clean, end-to-end recycled 8 of 14 copies of
      `test_images/` into 4 keepers. The Recycle Bin call vetoes any item Windows would delete
      permanently (a network path is refused and left in place).
- [ ] `src-tauri` still deletes through the `trash` crate, which permanently deleted a file on a
      network path in the CLI's RED test. Switch it to `imgdedup::recycle` (part of the item below).
- [ ] Keeper rule: a group of same-size images keeps the LARGER file, so `city_backup.jpg`
      (157 KB JPEG) wins over `city_photo.png` (14 KB PNG) and the lossless copy is deleted. Decide
      whether a lossless format should win a pixel-count tie.
- [ ] Point `src-tauri` at the `cli/` library crate so one copy of the algorithm remains, then
      decide whether to retire the PyO3 crate (`src/lib.rs`) and the Tkinter / Python path.
      Waiting on an owner decision.
