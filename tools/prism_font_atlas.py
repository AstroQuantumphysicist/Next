#!/usr/bin/env python3
"""Regenerate the checked-in Open Sans coverage atlas; not a build dependency.

Requires Pillow12.3.0/FreeType2.14.3 and the exact Apache2 source listed in
crates/prism/assets/NOTICE.txt. Refuses another font before writing outputs.
"""
import argparse
import hashlib
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

SOURCE_SHA256 = 'e64e508b2aa2880f907e470c4550980ec4c0694d103a43f36150ac3f93189bee'

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('font', type=Path)
    args = parser.parse_args()
    if hashlib.sha256(args.font.read_bytes()).hexdigest() != SOURCE_SHA256:
        parser.error('source font SHA-256 does not match audited Open Sans')
    output = Path(__file__).resolve().parents[1]/'crates/prism/assets'
    atlas, advances = bytearray(), bytearray()
    for size in (14,16,20):
        font = ImageFont.truetype(str(args.font),size)
        for code in range(32,127):
            image = Image.new('L',(24,32))
            ImageDraw.Draw(image).text((0,-1),chr(code),font=font,fill=255)
            atlas.extend(image.tobytes())
            advances.append(round(font.getlength(chr(code))))
    (output/'open-sans-atlas.bin').write_bytes(atlas)
    (output/'open-sans-advance.bin').write_bytes(advances)

if __name__ == '__main__':
    main()
