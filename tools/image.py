#!/usr/bin/env python3
"""Build an ESP-only removable UEFI development image (not an installer)."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]

def main():
    build = ROOT / 'build'
    image = build / 'next-dev.img'
    binary = build / 'esp/EFI/BOOT/BOOTX64.EFI'
    kernel = build / 'esp/EFI/NEXT/HELIX.ELF'
    initrd = build / 'esp/EFI/NEXT/INITRD.NXI'
    package = build / 'esp/EFI/NEXT/SYSTEM.NXP'
    slots = [build / f'esp/EFI/NEXT/{slot}/{name}' for slot in ('A', 'B')
             for name in ('HELIX.ELF', 'INITRD.NXI', 'SYSTEM.NXP')]
    if not all(p.is_file() for p in [binary, kernel, initrd, package, *slots]):
        raise SystemExit('Build the UEFI loader, kernel and init image first with tools/build.sh')
    missing = [tool for tool in ['mkfs.fat', 'mmd', 'mcopy'] if shutil.which(tool) is None]
    if missing:
        raise SystemExit(f"Missing tools: {', '.join(missing)}; source tools/env.sh first")
    # Write only the generated file; never accept a device path.
    temporary = build / 'next-dev.img.tmp'
    with temporary.open('wb') as output:
        output.truncate(64 * 1024 * 1024)
    note = build / 'NEXT.TXT'
    note.write_text('Read by Helix from the Next development disk.\n')
    try:
        subprocess.run(['mkfs.fat', '-F', '32', '-n', 'NEXTDEV', str(temporary)], check=True)
        subprocess.run(['mmd', '-i', str(temporary), '::/EFI', '::/EFI/BOOT', '::/EFI/NEXT',
                        '::/EFI/NEXT/A', '::/EFI/NEXT/B'], check=True)
        for source, destination in [(note, '::/NEXT.TXT'), (binary, '::/EFI/BOOT/BOOTX64.EFI'),
                                    (kernel, '::/EFI/NEXT/HELIX.ELF'), (initrd, '::/EFI/NEXT/INITRD.NXI'),
                                    (package, '::/EFI/NEXT/SYSTEM.NXP')]:
            subprocess.run(['mcopy', '-i', str(temporary), str(source), destination], check=True)
        for slot in ('A', 'B'):
            for name in ('HELIX.ELF', 'INITRD.NXI', 'SYSTEM.NXP'):
                source = build / f'esp/EFI/NEXT/{slot}/{name}'
                subprocess.run(['mcopy', '-i', str(temporary), str(source),
                                f'::/EFI/NEXT/{slot}/{name}'], check=True)
        # Failed formatting/copying must preserve the previous bootable image.
        # The FAT32 BPB remains a 64 MiB ESP. A separate bounded development
        # data region follows it; this is not GPT or an installed-system layout.
        with temporary.open('r+b') as output:
            output.truncate(65 * 1024 * 1024)
        temporary.replace(image)
    finally:
        temporary.unlink(missing_ok=True)
    manifest = {
        'artifact': 'Next development ESP image; NOT Next 1.0 or an installer',
        'uefi_secure_boot_certified': False,
        'deployment_bundle': 'SHA256/Ed25519 signed with workspace development key; not a production trust chain',
        'sha256': {('next-dev.img' if p == image else str(p.relative_to(build / 'esp'))):
                   hashlib.sha256(p.read_bytes()).hexdigest()
                   for p in [binary, kernel, initrd, package, *slots, image]},
    }
    (build / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Built {image} ({image.stat().st_size} bytes)')

if __name__ == '__main__':
    main()
