#!/usr/bin/env python3
"""Planner-encoded sparse scratch GPT with a minimal real FAT32 ESP. No boot writes."""
from pathlib import Path
import struct
import uuid
import gpt_plan

ROOT = Path(__file__).resolve().parents[1]
NOTE = b'NEXT GPT readonly ESP fixture\n'

def main():
    target = ROOT / 'work/gpt-fixtures/gpt-fat.img'
    target.parent.mkdir(parents=True, exist_ok=True)
    capacity = 70 << 20
    parts = gpt_plan.plan(capacity, 512, alignment=4096, esp=64 << 20,
                          system=65536, recovery=65536, minimum_data=65536)
    parts = [gpt_plan.Partition(p.name, p.type_guid, uuid.UUID(int=i + 1),
                               p.first, p.last, p.attributes) for i, p in enumerate(parts)]
    metadata = gpt_plan.encode(capacity, 512, uuid.UUID(int=101), parts)
    esp = next(p for p in parts if p.type_guid == gpt_plan.ESP)
    boot = bytearray(512)
    boot[:3] = b'\xeb\x58\x90'
    boot[3:11] = b'NEXTTEST'
    struct.pack_into('<H', boot, 11, 512)
    boot[13] = 1
    struct.pack_into('<H', boot, 14, 32)
    boot[16] = 1
    struct.pack_into('<I', boot, 32, esp.last - esp.first + 1)
    struct.pack_into('<I', boot, 36, 1024)
    struct.pack_into('<I', boot, 44, 2)
    boot[510:] = b'\x55\xaa'
    fat = bytearray(512)
    struct.pack_into('<4I', fat, 0, 0x0ffffff8, 0xffffffff, 0x0fffffff, 0x0fffffff)
    directory = bytearray(512)
    directory[:11] = b'NEXT    TXT'
    directory[11] = 0x20
    struct.pack_into('<H', directory, 26, 3)
    struct.pack_into('<I', directory, 28, len(NOTE))
    metadata[esp.first] = boot
    metadata[esp.first + 32] = fat
    metadata[esp.first + 1056] = directory
    metadata[esp.first + 1057] = NOTE.ljust(512, b'\0')
    with target.open('wb') as stream:
        stream.truncate(capacity)
        for lba, data in sorted(metadata.items()):
            stream.seek(lba * 512)
            stream.write(data)
    print('Built sparse GPT/FAT scratch fixture; no guest boot image changed')

if __name__ == '__main__':
    main()
