#!/usr/bin/env python3
"""Rebuild Prism's Selawik Regular TTF and 14/16/20 px ASCII raster atlas.

The pinned UFO source is distributed with Prism under the SIL Open Font
License. Build with the exact packages in docs/third-party/selawik-build-requirements.txt.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import platform
import subprocess
import sys
import tempfile
import zipfile
from importlib.metadata import version
from pathlib import Path

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont, features

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/prism/assets/source/Selawik-Regular.ufo.zip"
SOURCE_SHA256 = "640369d5f5c5372d16011d038c3bad54477b9f796f0179a41de17eb4cd513eca"
EXPECTED_PYTHON = "3.12.14"
EXPECTED_FREETYPE = "2.14.3"
EPOCH_1904 = 2_082_844_800


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "crates/prism/assets")
    args = parser.parse_args()

    if sha256(SOURCE) != SOURCE_SHA256:
        parser.error("pinned Selawik UFO archive SHA-256 mismatch")
    if platform.python_version() != EXPECTED_PYTHON:
        parser.error(f"Python {EXPECTED_PYTHON} required, found {platform.python_version()}")
    if features.version_module("freetype2") != EXPECTED_FREETYPE:
        parser.error(f"FreeType {EXPECTED_FREETYPE} required, found {features.version_module('freetype2')}")
    requirements = ROOT / "docs/third-party/selawik-build-requirements.txt"
    locked = [line.strip().split("==", 1) for line in requirements.read_text().splitlines()
              if line.strip() and not line.startswith("#")]
    wrong = {name: (version(name), want) for name, want in locked
             if version(name) != want}
    if wrong:
        parser.error(f"build tool versions differ from pinned versions: {wrong}")

    args.output_dir = args.output_dir.resolve()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="next-selawik-") as temp:
        build = Path(temp)
        with zipfile.ZipFile(SOURCE) as archive:
            archive.extractall(build)
        ufo = build / "Selawik-Regular.ufo"
        font_path = args.output_dir / "source/Selawik-Regular.ttf"
        font_path.parent.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ)
        env["SOURCE_DATE_EPOCH"] = "0"
        subprocess.run(
            [sys.executable, "-m", "fontmake", "-u", str(ufo), "-o", "ttf",
             "--output-path", str(font_path), "--no-production-names"],
            check=True,
            cwd=build,
            env=env,
        )
        # OpenType head timestamps otherwise vary with the build date.
        font = TTFont(font_path)
        font["head"].created = EPOCH_1904
        font["head"].modified = EPOCH_1904
        font.recalcTimestamp = False
        font.save(font_path)

    atlas = bytearray()
    advances = bytearray()
    x_offsets = bytearray()
    for size in (14, 16, 20):
        font = ImageFont.truetype(str(font_path), size)
        for code in range(32, 127):
            character = chr(code)
            # Keep negative side bearings inside the fixed atlas cell. Prism
            # applies this signed offset when placing the cached glyph.
            box = font.getbbox(character)
            x_bearing = box[0]
            shift = max(0, -x_bearing)
            if box[2] + shift > 24:
                parser.error(f"glyph U+{code:04X} exceeds the 24-pixel cell at {size}px")
            image = Image.new("L", (24, 32))
            ImageDraw.Draw(image).text((shift, -1), character, font=font, fill=255)
            atlas.extend(image.tobytes())
            advance = round(font.getlength(character))
            if not 0 <= advance <= 255:
                parser.error(f"advance for U+{code:04X} does not fit the existing byte table")
            if not -128 <= x_bearing <= 127:
                parser.error(f"x bearing for U+{code:04X} does not fit the signed-byte table")
            advances.append(advance)
            bearing = min(0, x_bearing)
            x_offsets.append(bearing & 0xFF)

    (args.output_dir / "selawik-regular-atlas.bin").write_bytes(atlas)
    (args.output_dir / "selawik-regular-advance.bin").write_bytes(advances)
    (args.output_dir / "selawik-regular-x-offset.bin").write_bytes(x_offsets)
    print(f"font sha256 {sha256(font_path)}")
    print(f"atlas sha256 {sha256(args.output_dir / 'selawik-regular-atlas.bin')}")
    print(f"advance sha256 {sha256(args.output_dir / 'selawik-regular-advance.bin')}")
    print(f"x-offset sha256 {sha256(args.output_dir / 'selawik-regular-x-offset.bin')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
