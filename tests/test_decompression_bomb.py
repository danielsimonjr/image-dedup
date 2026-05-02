"""Smoke tests for the decompression-bomb guard (#5).

Builds a tiny PNG that *declares* 65535x65535 dimensions in the IHDR
chunk but contains no pixel data, then asserts that dedup_engine refuses
to decode it. A failure here means a malicious user could OOM the
process by feeding a hand-crafted PNG.
"""

from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path

import pytest

# Ensure the project root is on sys.path when running pytest from a fresh shell
PROJECT_ROOT = Path(__file__).resolve().parent.parent
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

import dedup_engine  # noqa: E402


def _png_chunk(tag: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(tag + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)


def _make_bomb_png(path: Path, width: int = 65535, height: int = 65535) -> None:
    """Write a minimal PNG declaring (width x height) but with no IDAT data.

    The header alone is enough for Pillow to compute width*height and
    raise DecompressionBombError — we never reach IDAT.
    """
    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    iend = b""
    blob = sig + _png_chunk(b"IHDR", ihdr) + _png_chunk(b"IEND", iend)
    path.write_bytes(blob)


def test_bomb_png_rejected_without_oom(tmp_path: Path) -> None:
    bomb = tmp_path / "bomb.png"
    _make_bomb_png(bomb)

    # MAX_DECODE_PIXELS must be locked to our budget regardless of Pillow
    # default behavior.
    assert dedup_engine.MAX_DECODE_PIXELS == 50_000_000

    # Use the public scan_images entry-point — this is the call site we
    # actually want to harden.
    results = dedup_engine.scan_images(str(tmp_path), recursive=False)

    paths = {str(r.path) for r in results}
    assert str(bomb) not in paths, (
        "decompression-bomb PNG must be silently dropped, not decoded"
    )


def test_normal_png_still_processed(tmp_path: Path) -> None:
    """Sanity check: a tiny well-formed PNG must still be picked up."""
    from PIL import Image as PILImage

    ok = tmp_path / "ok.png"
    PILImage.new("RGB", (64, 64), (200, 50, 50)).save(ok)

    results = dedup_engine.scan_images(str(tmp_path), recursive=False)
    paths = {str(r.path) for r in results}
    assert str(ok) in paths
