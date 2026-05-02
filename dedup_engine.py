"""
Duplicate image detection engine using perceptual hashing + SSIM verification.

Two-pass approach:
  1. pHash: Fast coarse filter — computes a 64-bit perceptual hash for each image.
     Images with Hamming distance <= threshold are candidate duplicates.
  2. SSIM: Structural Similarity Index verification on candidates.
     Only pairs exceeding the SSIM threshold are confirmed as duplicates.

The keeper in each duplicate group is the highest-resolution version (by pixel count).
"""

import os
import hashlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional
from concurrent.futures import ThreadPoolExecutor, as_completed

import imagehash
from PIL import Image
from PIL.Image import DecompressionBombError
from skimage.metrics import structural_similarity as ssim
import numpy as np


# ── Decompression-bomb guard ────────────────────────────────────────────
# Pillow defaults MAX_IMAGE_PIXELS to ~89 MP and only WARNS (doesn't raise)
# until 2× that. Lock it down to 50 MP and treat the warning as a hard error.
MAX_DECODE_PIXELS = 50_000_000
Image.MAX_IMAGE_PIXELS = MAX_DECODE_PIXELS

# ── pHash collision hardening (#6) ──────────────────────────────────────
# When MD5 differs and SSIM falls below STRICT_SSIM, require dHash to also
# agree before classifying a pair as a high-confidence duplicate.
STRICT_SSIM = 0.98
DHASH_AGREE_DISTANCE = 10

# ── Streaming MD5 (#7) ──────────────────────────────────────────────────
HASH_BUF_BYTES = 64 * 1024


def _stream_md5(path: Path) -> str:
    """Compute MD5 in 64 KiB chunks instead of loading the whole file.

    Replaces ``hashlib.md5(path.read_bytes())`` which materialized the
    entire file in RAM N times in parallel.
    """
    h = hashlib.md5()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(HASH_BUF_BYTES), b""):
            h.update(chunk)
    return h.hexdigest()


IMAGE_EXTENSIONS = {
    ".jpg",
    ".jpeg",
    ".png",
    ".gif",
    ".bmp",
    ".tiff",
    ".tif",
    ".webp",
}


@dataclass
class ImageInfo:
    path: Path
    width: int
    height: int
    file_size: int
    phash: Optional[imagehash.ImageHash] = None
    dhash: Optional[imagehash.ImageHash] = None
    md5: str = ""

    @property
    def pixel_count(self) -> int:
        return self.width * self.height

    @property
    def resolution_label(self) -> str:
        return f"{self.width}x{self.height}"


@dataclass
class DuplicateGroup:
    keeper: ImageInfo
    duplicates: list[ImageInfo] = field(default_factory=list)
    scores: dict[str, float] = field(default_factory=dict)  # path -> ssim score
    confidence: str = "high"  # "high" or "low" — see #6


def scan_images(
    folder: str,
    recursive: bool = True,
    progress_callback=None,
) -> list[ImageInfo]:
    """Scan folder for images and compute metadata + perceptual hashes."""
    folder_path = Path(folder)
    if not folder_path.is_dir():
        raise ValueError(f"Not a valid directory: {folder}")

    # Collect image paths
    image_paths = []
    if recursive:
        for root, _, files in os.walk(folder_path):
            for f in files:
                if Path(f).suffix.lower() in IMAGE_EXTENSIONS:
                    image_paths.append(Path(root) / f)
    else:
        for f in folder_path.iterdir():
            if f.is_file() and f.suffix.lower() in IMAGE_EXTENSIONS:
                image_paths.append(f)

    total = len(image_paths)
    results = []

    def process_one(img_path: Path) -> Optional[ImageInfo]:
        try:
            with Image.open(img_path) as img:
                # Cheap dimension peek before .load() — bombs never get
                # decoded into RAM. Pillow raises DecompressionBombError
                # automatically once MAX_IMAGE_PIXELS is exceeded, but we
                # also explicitly check so we can short-circuit on
                # malformed headers.
                w, h = img.size
                if w * h > MAX_DECODE_PIXELS:
                    return None
                try:
                    img.load()
                except DecompressionBombError:
                    return None
                phash = imagehash.phash(img)
                dhash = imagehash.dhash(img)

            file_size = img_path.stat().st_size

            # #7: streaming MD5 instead of read_bytes(). Avoids loading
            # the whole file into RAM in N parallel threads.
            md5 = _stream_md5(img_path)

            return ImageInfo(
                path=img_path,
                width=w,
                height=h,
                file_size=file_size,
                phash=phash,
                dhash=dhash,
                md5=md5,
            )
        except DecompressionBombError:
            return None
        except Exception:
            return None

    # Use threads for I/O-bound hashing
    with ThreadPoolExecutor(max_workers=min(8, os.cpu_count() or 4)) as pool:
        futures = {pool.submit(process_one, p): p for p in image_paths}
        for i, future in enumerate(as_completed(futures)):
            info = future.result()
            if info is not None:
                results.append(info)
            if progress_callback:
                progress_callback(i + 1, total, str(futures[future]))

    return results


def compute_ssim(img1_path: Path, img2_path: Path, target_size: int = 256) -> float:
    """Compute SSIM between two images after resizing to a common dimension."""
    try:
        with Image.open(img1_path) as im1, Image.open(img2_path) as im2:
            # Bomb guard before convert/resize materialize the pixel buffer.
            for im in (im1, im2):
                w, h = im.size
                if w * h > MAX_DECODE_PIXELS:
                    return 0.0
            # Convert to grayscale and resize to common size for fair comparison
            im1_gray = im1.convert("L").resize(
                (target_size, target_size), Image.LANCZOS
            )
            im2_gray = im2.convert("L").resize(
                (target_size, target_size), Image.LANCZOS
            )

            arr1 = np.array(im1_gray)
            arr2 = np.array(im2_gray)

            return float(ssim(arr1, arr2))
    except DecompressionBombError:
        return 0.0
    except Exception:
        return 0.0


def find_duplicates(
    images: list[ImageInfo],
    phash_threshold: int = 10,
    ssim_threshold: float = 0.90,
    progress_callback=None,
) -> list[DuplicateGroup]:
    """
    Find duplicate image groups using two-pass detection.

    Pass 1: pHash Hamming distance <= phash_threshold → candidate pair
    Pass 2: SSIM score >= ssim_threshold → confirmed duplicate

    Returns groups where the keeper is the highest-resolution version.
    """
    n = len(images)
    if n < 2:
        return []

    # Build union-find for grouping
    parent = list(range(n))

    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a, b):
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[ra] = rb

    # Track SSIM scores + confidence flag for each confirmed pair (#6)
    pair_scores: dict[tuple[int, int], float] = {}
    pair_confidence: dict[tuple[int, int], bool] = {}  # True = high

    total_pairs = n * (n - 1) // 2
    checked = 0

    # Pass 1 + 2: Check all pairs
    for i in range(n):
        for j in range(i + 1, n):
            checked += 1

            # Fast path: exact MD5 match
            if images[i].md5 and images[i].md5 == images[j].md5:
                union(i, j)
                pair_scores[(i, j)] = 1.0
                pair_confidence[(i, j)] = True
                if progress_callback:
                    progress_callback(checked, total_pairs, "MD5 exact match")
                continue

            # Pass 1: pHash coarse filter
            if images[i].phash is None or images[j].phash is None:
                continue

            hamming = images[i].phash - images[j].phash
            if hamming > phash_threshold:
                if progress_callback and checked % 500 == 0:
                    progress_callback(checked, total_pairs, "pHash filtering...")
                continue

            # Pass 2: SSIM verification
            score = compute_ssim(images[i].path, images[j].path)
            if score >= ssim_threshold:
                union(i, j)
                pair_scores[(i, j)] = score
                # #6: only "high" confidence if SSIM is strict OR
                # dHash also agrees. Otherwise group must be marked
                # for manual review (no auto-delete).
                high = score >= STRICT_SSIM
                if not high and images[i].dhash is not None and images[j].dhash is not None:
                    high = (images[i].dhash - images[j].dhash) <= DHASH_AGREE_DISTANCE
                pair_confidence[(i, j)] = high

            if progress_callback and checked % 50 == 0:
                progress_callback(checked, total_pairs, f"SSIM={score:.3f}")

    # Build groups from union-find
    groups_map: dict[int, list[int]] = {}
    for i in range(n):
        root = find(i)
        groups_map.setdefault(root, []).append(i)

    # Convert to DuplicateGroup objects
    results = []
    for indices in groups_map.values():
        if len(indices) < 2:
            continue

        # Keeper = highest resolution (pixel count), break ties by file size
        members = [images[idx] for idx in indices]
        members.sort(key=lambda m: (m.pixel_count, m.file_size), reverse=True)

        keeper = members[0]
        dupes = members[1:]

        # Group is high confidence only if EVERY edge connecting members
        # is high confidence (#6).
        member_paths = {str(m.path) for m in members}
        all_high = True
        for (a, b), high in pair_confidence.items():
            if str(images[a].path) in member_paths and str(images[b].path) in member_paths:
                if not high:
                    all_high = False
                    break

        group = DuplicateGroup(
            keeper=keeper,
            duplicates=dupes,
            confidence="high" if all_high else "low",
        )

        # Attach SSIM scores
        for d in dupes:
            # Find the score for this pair
            for (a, b), sc in pair_scores.items():
                paths = {str(images[a].path), str(images[b].path)}
                if str(d.path) in paths and str(keeper.path) in paths:
                    group.scores[str(d.path)] = sc
                    break
            else:
                group.scores[str(d.path)] = 0.0

        results.append(group)

    return results


def delete_duplicates(
    groups: list[DuplicateGroup],
    send_to_trash: bool = True,
    include_low_confidence: bool = False,
):
    """Delete the lower-resolution duplicates. Returns list of deleted paths.

    #6: low-confidence groups (SSIM in [threshold, 0.98) without dHash
    agreement) are NEVER auto-deleted unless the caller opts in
    explicitly via include_low_confidence=True.
    """
    deleted = []
    for group in groups:
        if group.confidence != "high" and not include_low_confidence:
            continue
        for dupe in group.duplicates:
            try:
                if send_to_trash:
                    # Try send2trash if available, else regular delete
                    try:
                        from send2trash import send2trash as trash

                        trash(str(dupe.path))
                    except ImportError:
                        dupe.path.unlink()
                else:
                    dupe.path.unlink()
                deleted.append(str(dupe.path))
            except OSError as e:
                print(f"Failed to delete {dupe.path}: {e}")
    return deleted
