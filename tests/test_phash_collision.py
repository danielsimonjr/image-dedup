"""pHash collision-hardening test (#6).

Two visually distinct images with different MD5 must NOT end up in the
same high-confidence duplicate group. They may legitimately appear as
"low-confidence" if pHash + SSIM weakly agree, but auto-delete must skip
them.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest
from PIL import Image as PILImage

PROJECT_ROOT = Path(__file__).resolve().parent.parent
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

import dedup_engine  # noqa: E402


def _make_distinct_image(path: Path, seed: int) -> None:
    rng = np.random.default_rng(seed)
    arr = rng.integers(0, 255, size=(128, 128, 3), dtype=np.uint8)
    PILImage.fromarray(arr, "RGB").save(path)


def test_distinct_images_not_auto_deleted(tmp_path: Path) -> None:
    a = tmp_path / "a.png"
    b = tmp_path / "b.png"
    _make_distinct_image(a, seed=1)
    _make_distinct_image(b, seed=42)

    images = dedup_engine.scan_images(str(tmp_path), recursive=False)
    assert len(images) == 2

    # MD5 must differ (sanity check on the test fixtures)
    assert images[0].md5 != images[1].md5

    # Run with very loose thresholds — this is the worst case where
    # the OLD code would have happily merged them.
    groups = dedup_engine.find_duplicates(
        images, phash_threshold=64, ssim_threshold=0.0
    )

    # Either: no group at all (SSIM rejected them), or low-confidence.
    for g in groups:
        # If they DID get grouped, it must NOT be high confidence.
        member_paths = {str(g.keeper.path), *(str(d.path) for d in g.duplicates)}
        if {str(a), str(b)} <= member_paths:
            assert g.confidence == "low", (
                "distinct images grouped at high confidence — pHash collision regression"
            )

    # And delete_duplicates with default args must NOT touch them.
    deleted = dedup_engine.delete_duplicates(groups, send_to_trash=False)
    assert str(a) not in deleted and str(b) not in deleted


def test_identical_images_high_confidence(tmp_path: Path) -> None:
    """Positive control: byte-identical images must still be grouped high."""
    src = tmp_path / "src.png"
    dst = tmp_path / "copy.png"
    PILImage.new("RGB", (64, 64), (10, 200, 30)).save(src)
    dst.write_bytes(src.read_bytes())  # exact byte copy → MD5 match

    images = dedup_engine.scan_images(str(tmp_path), recursive=False)
    assert len(images) == 2
    assert images[0].md5 == images[1].md5

    groups = dedup_engine.find_duplicates(
        images, phash_threshold=10, ssim_threshold=0.90
    )
    assert len(groups) == 1
    assert groups[0].confidence == "high"
