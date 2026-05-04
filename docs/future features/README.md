# Future Features

## High Impact, Lower Effort

### Side-by-side comparison
Click two images in a group to see them next to each other with a visual diff overlay (pixel difference heatmap). Makes it obvious why they're duplicates and which is better quality.

### Progress events during scan
Right now the UI shows "Scanning..." with no granularity. Emit Tauri events from Rust (`app.emit("scan_progress", ...)`) so the frontend shows a real progress bar (e.g., "Processing 47/312 images...").

### Undo delete
Currently deletion goes to Recycle Bin but there's no in-app undo. Add a session-level undo stack that remembers what was trashed, with a toast notification: "Deleted 5 files. [Undo]".

### Drag-and-drop folder
Let users drag a folder onto the window instead of requiring the Browse dialog. Tauri supports this via the `onDragDropEvent` API.

### Export report
"Export CSV" button that saves the duplicate groups table (paths, resolutions, SSIM scores, actions) for auditing or scripting.

## Medium Impact, Medium Effort

### Adjustable thresholds in the UI
Expose pHash distance and SSIM threshold as sliders in the toolbar, so users can tune sensitivity without rebuilding. Currently hardcoded at `phashThreshold: 10, ssimThreshold: 0.90`.

### Multi-folder scan
Scan multiple directories at once (e.g., find duplicates across "Photos" and "Backup"). Would require `scan_images` to accept `Vec<String>` folders.

### Thumbnail grid view
Alternative to the table: a visual grid showing duplicate groups as thumbnail clusters. More intuitive for photo-heavy workflows.

### Remember last session
Persist last folder, thresholds, and window size across launches using Tauri's `tauri-plugin-store`.

### Keyboard navigation
Arrow keys to navigate rows, Space to toggle check, Enter to preview, Delete to mark for deletion.

## Higher Effort, Big Payoff

### GPU-accelerated SSIM
The SSIM step is the bottleneck for large collections. Using `wgpu` or compute shaders could parallelize the 256x256 image comparisons massively.

### Watch mode
Monitor a folder for new images and automatically flag duplicates as they appear. Useful for photographers importing from cameras.

### Cross-platform packaging
Add macOS `.dmg` and Linux `.AppImage` targets to the Tauri build. The Rust code is already cross-platform; it's just build config.

### Smart keeper selection
Beyond just resolution, consider EXIF metadata (original vs edited), file format (lossless vs lossy), and creation date to pick the best "keeper".

### Similarity browser
Instead of binary "duplicate or not", show a similarity graph where users can explore clusters of similar (not identical) images at various thresholds.
