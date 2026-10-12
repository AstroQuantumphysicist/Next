#!/usr/bin/env python3
"""Rebuild Prism's Selawik Regular TTF and 14/16/20 px Latin raster atlas.

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

from PIL import Image, ImageDraw, ImageFont, features

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/prism/assets/source/Selawik-Regular.ufo.zip"
SOURCE_SHA256 = "640369d5f5c5372d16011d038c3bad54477b9f796f0179a41de17eb4cd513eca"
BUILT_TTF_SHA256 = "e6887e017dd26cdb234de6aed7c15c7a4e808d1a2d1df5bd7b95fe1f555d6b63"
EXPECTED_PYTHON = "3.12.14"
EXPECTED_FREETYPE = "2.14.3"
EXPECTED_PILLOW = "12.3.0"
EPOCH_1904 = 2_082_844_800

# All printable Latin-1 codepoints plus the typographic punctuation needed by
# the UI and common Western European text. Kept sorted for the guest binary
# search; the generated table stores these as little-endian u16 values.
EXTRA_CODEPOINTS = tuple(range(0x00A0, 0x0100)) + (
    0x2010, 0x2011, 0x2012, 0x2013, 0x2014, 0x2015,
    0x2018, 0x2019, 0x201A, 0x201B, 0x201C, 0x201D, 0x201E, 0x201F,
    0x2020, 0x2021, 0x2022, 0x2026,
    0x2030, 0x2039, 0x203A, 0x20AC, 0x2122,
)
ASCII_CODEPOINTS = tuple(range(0x20, 0x7F))
CODEPOINTS = ASCII_CODEPOINTS + EXTRA_CODEPOINTS

# Selawik does not contain glyphs for these points. Use clear text-safe
# aliases where a near-equivalent exists; ordinal indicators fall back to '?'.
ALIASES = {
    0x00A0: 0x0020,  # no-break space -> ordinary space advance/blank
    0x2010: 0x002D,  # hyphen variants -> hyphen-minus
    0x2011: 0x002D,
    0x2012: 0x002D,
    0x2015: 0x002D,
    0x201B: 0x0027,  # reversed curly quotes -> straight quote
    0x201F: 0x0022,
}
QUESTION_FALLBACKS = {0x00AA, 0x00BA}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "crates/prism/assets")
    parser.add_argument(
        "--atlas-only",
        action="store_true",
        help="rasterize from the pinned built TTF without invoking fontmake",
    )
    args = parser.parse_args()

    if sha256(SOURCE) != SOURCE_SHA256:
        parser.error("pinned Selawik UFO archive SHA-256 mismatch")
    if platform.python_version() != EXPECTED_PYTHON:
        parser.error(f"Python {EXPECTED_PYTHON} required, found {platform.python_version()}")
    if features.version_module("freetype2") != EXPECTED_FREETYPE:
        parser.error(f"FreeType {EXPECTED_FREETYPE} required, found {features.version_module('freetype2')}")
    if version("Pillow") != EXPECTED_PILLOW:
        parser.error(f"Pillow {EXPECTED_PILLOW} required, found {version('Pillow')}")
    requirements = ROOT / "docs/third-party/selawik-build-requirements.txt"
    locked = [line.strip().split("==", 1) for line in requirements.read_text().splitlines()
              if line.strip() and not line.startswith("#")]
    if args.atlas_only:
        locked = [(name, want) for name, want in locked if name.lower() == "pillow"]
    wrong = {name: (version(name), want) for name, want in locked if version(name) != want}
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
        if args.atlas_only:
            font_path = (args.output_dir / "source/Selawik-Regular.ttf").resolve()
            if sha256(font_path) != BUILT_TTF_SHA256:
                parser.error("built Selawik TTF SHA-256 mismatch for atlas-only replay")
        else:
            from fontTools.ttLib import TTFont

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

    if sha256(font_path) != BUILT_TTF_SHA256:
        parser.error("built Selawik TTF SHA-256 mismatch")
    allowed_missing = set(ALIASES) | QUESTION_FALLBACKS
    if args.atlas_only:
        # The checked TTF digest pins the cmap; avoiding fontTools here lets
        # this raster-only replay depend only on the locked rasterizer.
        missing = allowed_missing
    else:
        from fontTools.ttLib import TTFont

        cmap = TTFont(font_path).getBestCmap()
        missing = set(CODEPOINTS) - set(cmap)
        if missing != allowed_missing:
            parser.error(f"Selawik character coverage changed; missing {sorted(missing)}")

    atlas = bytearray()
    advances = bytearray()
    x_offsets = bytearray()
    for size in (14, 16, 20):
        font = ImageFont.truetype(str(font_path), size)
        for code in CODEPOINTS:
            alias = ALIASES.get(code, code)
            if code in missing and code not in ALIASES:
                alias = ord("?")
            character = chr(alias)
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
    (args.output_dir / "selawik-regular-codepoints-le.bin").write_bytes(
        b"".join(code.to_bytes(2, "little") for code in EXTRA_CODEPOINTS)
    )
    print(f"font sha256 {sha256(font_path)}")
    print(f"atlas sha256 {sha256(args.output_dir / 'selawik-regular-atlas.bin')}")
    print(f"advance sha256 {sha256(args.output_dir / 'selawik-regular-advance.bin')}")
    print(f"x-offset sha256 {sha256(args.output_dir / 'selawik-regular-x-offset.bin')}")
    print(f"codepoint-map sha256 {sha256(args.output_dir / 'selawik-regular-codepoints-le.bin')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
