#!/usr/bin/env python3
"""Emit bounded service declarations for inclusion in the signed nextd bundle.

CRC protects parsing only. Authority comes from the enclosing owner-signed NXP,
exact PID1 principal binding, and independently verified worker executables.
"""
import argparse
import hashlib
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parents[1]


def config(worker: bytes, active_mask: int = 1) -> bytes:
    if active_mask not in (1, 3):
        raise ValueError('development profile supports active masks 1 and 3')
    result = bytearray(32 + 2 * 256)
    struct.pack_into('<8sHHHHII', result, 0, b'NXSVC001', 1, 32, 256, 2, len(result), 0)
    result[24] = active_mask
    digest = hashlib.sha256(worker).digest()
    for index, name in enumerate(('worker-primary', 'worker-dependent')):
        at = 32 + index * 256
        result[at:at+32] = name.encode().ljust(32, b'\0')
        result[at+32:at+96] = b'/system/service-worker.elf'.ljust(64, b'\0')
        result[at+96:at+160] = b'org.next.development.native.service-worker'.ljust(64, b'\0')
        result[at+160:at+192] = digest
        # Only the log/channel startup grant: READ/WRITE/DUPLICATE/TRANSFER.
        # No inherited launcher, ambient filesystem grant, or service ownership
        # claim for the production daemons named by the specification.
        struct.pack_into('<QQBB', result, at+192, 0x1b, 0, 0 if index == 0 else 1, 4)
        struct.pack_into('<QQQQ', result, at+216,
                         10_000_000_000, 20_000_000, 200_000_000, 300_000_000)
    struct.pack_into('<I', result, 20, zlib.crc32(result))
    return bytes(result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path, default=ROOT/'build/user/service-worker.elf')
    parser.add_argument('--active-mask', type=int, default=1)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    worker = args.worker.read_bytes()
    if not worker.startswith(b'\x7fELF'):
        raise SystemExit('service worker must be the original raw ELF payload')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(config(worker, args.active_mask))
    print(f'Built authenticated-member service config: active_mask={args.active_mask}, 2 declarations, 544 bytes')


if __name__ == '__main__':
    main()
