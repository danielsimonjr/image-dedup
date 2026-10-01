# Future Features

## Lower effort

### Export formats
Write the report as CSV or JSON as well as TSV, for auditing and scripting.

### Progress output
Print a progress line to stderr during the scan (for example, "Processing 47/312 images"). Add a `--quiet` flag to turn it off.

### Smart keeper selection
Look beyond resolution. Consider format (lossless or lossy), EXIF metadata (original or edited) and creation date. The keeper-rule row in `TODO.md` tracks the first part of this.

## Medium effort

### Multi-folder scan
Accept more than one ROOT, so the tool finds duplicates across "Photos" and "Backup".

### Undo log
Write the list of recycled files to a log. A later `imgdedup --restore LOG` command restores them from the Recycle Bin.

## Higher effort

### GPU-accelerated SSIM
SSIM is the bottleneck for large collections. Compute shaders (for example `wgpu`) could compare the 256x256 images in parallel.

### Watch mode
Monitor a folder for new images and report duplicates as they appear.

### Similarity clusters
Report similar (not identical) images as clusters at several thresholds, instead of a binary duplicate decision.
