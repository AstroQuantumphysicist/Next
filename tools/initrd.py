#!/usr/bin/env python3
"""Deterministic, bounded NEXTINIT development archive (CRC is not signing)."""
from pathlib import Path
import re
import struct
import zlib

ROOT = Path(__file__).resolve().parents[1]
MAX_ENTRIES = 64
MAX_IMAGE_SIZE = 16 * 1024 * 1024


def valid_path(path):
    return (1 < len(path) < 64 and path.startswith('/')
            and re.fullmatch(r'[A-Za-z0-9/._-]+', path) is not None
            and all(part not in ('', '.', '..') for part in path[1:].split('/')))


def pack(entries):
    """Pack explicit (absolute archive path, byte payload) pairs in path order."""
    entries = sorted(entries)
    if not 1 <= len(entries) <= MAX_ENTRIES:
        raise ValueError(f'archive must have 1..{MAX_ENTRIES} entries')
    names = set()
    offset = 32 + len(entries) * 80
    table = bytearray()
    payloads = bytearray()
    for path, data in entries:
        if not valid_path(path) or path in names:
            raise ValueError('invalid or duplicate canonical archive path')
        if not data or offset + len(data) > MAX_IMAGE_SIZE:
            raise ValueError('empty payload or archive exceeds 16 MiB')
        names.add(path)
        table += struct.pack('<64sQQ', path.encode('ascii'), offset, len(data))
        payloads += data
        offset += len(data)
    result = bytearray(struct.pack('<8sIIQII', b'NEXTINIT', 1, len(entries), offset, 0, 0))
    result += table + payloads
    struct.pack_into('<I', result, 24, zlib.crc32(result))
    return bytes(result)


def signed_entries(directory):
    """Read only the signed mirror; individual signatures are checked at launch."""
    if not (directory / 'system/nextd.elf').is_file():
        raise ValueError('Build signed application envelopes first; nextd.elf is required')
    entries = []
    for prefix, pattern in [('system', '*.elf'), ('compat/linux', '*.elf'), ('compat/nt', '*.exe')]:
        for file in sorted((directory / prefix).glob(pattern)):
            data = file.read_bytes()
            if len(data) < 449 or data[:8] != b'NXTAPP01' or struct.unpack_from('<H', data, 164)[0] != 1:
                raise ValueError(f'Raw or non-Application payload in signed mirror: {prefix}/{file.name}')
            entries.append((f'/{prefix}/{file.name}', data))
    return entries


def main():
    entries = signed_entries(ROOT / 'build/apps')
    archive = pack(entries)
    destination = ROOT / 'build/esp/EFI/NEXT/INITRD.NXI'
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(archive)
    print(f'Built {destination} ({len(entries)} entries, {len(archive)} bytes; signed application envelopes, deployment signs archive)')


if __name__ == '__main__':
    main()
