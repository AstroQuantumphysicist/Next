#!/usr/bin/env python3
"""Require actual EFI rejection of corrupted init images in disposable VM media."""
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from initrd import MAX_ENTRIES

ROOT = Path(__file__).resolve().parents[1]


def seal(payload):
    payload[24:28] = bytes(4)
    struct.pack_into('<I', payload, 24, zlib.crc32(payload))
    return payload


def cases(original):
    yield 'missing-initrd', None, 'open \\EFI\\NEXT\\INITRD.NXI:'
    yield 'truncated-initrd', original[:31], 'init image rejected: Header'
    for name, offset, fmt, value, diagnostic in [
        ('initrd-version', 8, 'I', 2, 'Version'),
        ('initrd-size', 16, 'Q', len(original) + 1, 'Size'),
        ('initrd-count', 12, 'I', MAX_ENTRIES + 1, 'Size'),
        ('initrd-reserved', 28, 'I', 1, 'Reserved'),
        ('initrd-range-overflow', 96, 'Q', 2**64 - 1, 'Range'),
    ]:
        payload = bytearray(original)
        struct.pack_into('<' + fmt, payload, offset, value)
        yield name, seal(payload), 'init image rejected: ' + diagnostic
    payload = bytearray(original)
    payload[-1] ^= 1
    yield 'initrd-checksum', payload, 'init image rejected: Checksum'
    payload = bytearray(original)
    payload[32:96] = bytes(64)
    payload[32:37] = b'/../a'
    yield 'initrd-traversal', seal(payload), 'init image rejected: Path'
    payload = bytearray(original)
    count = struct.unpack_from('<I', payload, 12)[0]
    for index in range(count):
        at = 32 + index * 80
        if payload[at:at + 64].split(b'\0')[0] == b'/system/nextd.elf':
            payload[at:at + 64] = bytes(64)
            name = b'/system/other.elf'
            payload[at:at + len(name)] = name
    yield 'initrd-missing-supervisor', seal(payload), 'init image missing /system/nextd.elf'


def main():
    image = ROOT / 'build/next-dev.img'
    original = (ROOT / 'build/esp/EFI/NEXT/INITRD.NXI').read_bytes()
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    work = ROOT / 'work'
    work.mkdir(exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix='initrd-failures-', dir=work) as temp_name:
        temp = Path(temp_name)
        for name, payload, diagnostic in cases(original):
            copy = temp / 'mutated.img'
            shutil.copyfile(image, copy)
            if payload is None:
                subprocess.run(['mdel', '-i', str(copy), '::/EFI/NEXT/INITRD.NXI'], check=True)
            else:
                archive = temp / 'INITRD.NXI'
                archive.write_bytes(payload)
                subprocess.run(['mcopy', '-o', '-i', str(copy), str(archive), '::/EFI/NEXT/INITRD.NXI'], check=True)
            output = ROOT / 'build/failures' / name
            command = [sys.executable, str(ROOT / 'tools/boot_smoke.py'), '--image', str(copy),
                       '--output-dir', str(output), '--expect-boot-failure', diagnostic,
                       '--expect-failure-stage', 'initrd']
            result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            report = json.loads((output / 'boot-smoke.json').read_text())
            results.append({'case': name, 'passed': result.returncode == 0 and report['passed'],
                            'checks': report['checks'], 'artifact': str(output.relative_to(ROOT) / 'serial.log'),
                            'error': report.get('error')})
            print(f"{name}: {'PASS' if results[-1]['passed'] else 'FAIL'}", flush=True)
    unchanged = hashlib.sha256(image.read_bytes()).hexdigest() == digest
    report = {'passed': unchanged and all(item['passed'] for item in results),
              'source_image_sha256': digest, 'source_image_unchanged': unchanged,
              'scope': 'QEMU/OVMF corrupted unsigned init archive rejection; CRC is not authentication', 'cases': results}
    (ROOT / 'build/initrd-failures.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
