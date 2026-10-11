#!/usr/bin/env python3
"""Read-only GPT inspection and dry-run Next partition planning; never installs."""
import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
import uuid
import zlib

U64 = (1 << 64) - 1
ENTRY_COUNT = 128
ENTRY_SIZE = 128
TABLE_BYTES = ENTRY_COUNT * ENTRY_SIZE
ESP = uuid.UUID('c12a7328-f81f-11d2-ba4b-00a0c93ec93b')
NAMESPACE = uuid.UUID('696a18b2-dbf3-4ce1-a0dc-b868257747ce')
TYPES = {name: uuid.uuid5(NAMESPACE, 'Next experimental ' + name) for name in ['system', 'data', 'recovery']}


def crc(data):
    return zlib.crc32(data) & 0xffffffff


def geometry(capacity, block_size):
    if block_size not in (512, 4096):
        raise ValueError('only 512 or 4096-byte logical blocks are supported')
    if not isinstance(capacity, int) or capacity < 0 or capacity > U64 or capacity % block_size:
        raise ValueError('capacity must be a nonnegative whole-block byte count within u64')
    blocks = capacity // block_size
    table_blocks = TABLE_BYTES // block_size
    if blocks < 2 * table_blocks + 4:
        raise ValueError('capacity cannot hold both GPT tables and usable space')
    return blocks, 2 + table_blocks, blocks - table_blocks - 2


@dataclass(frozen=True)
class Partition:
    name: str
    type_guid: uuid.UUID
    unique_guid: uuid.UUID
    first: int
    last: int
    attributes: int = 0

    def json(self):
        return {'name': self.name, 'type_guid': str(self.type_guid), 'unique_guid': str(self.unique_guid),
                'first_lba': self.first, 'last_lba': self.last, 'attributes': self.attributes}


def validate_partitions(parts, first, last):
    if len(parts) > ENTRY_COUNT:
        raise ValueError('too many GPT entries')
    ids = set()
    for part in parts:
        if part.type_guid.int == 0 or part.unique_guid.int == 0 or part.unique_guid in ids:
            raise ValueError('zero or duplicate partition GUID')
        ids.add(part.unique_guid)
        if not first <= part.first <= part.last <= last or not 0 <= part.attributes <= U64:
            raise ValueError('partition range/attributes outside GPT bounds')
        if part.attributes & (((1 << 48) - 1) & ~7):
            raise ValueError('reserved GPT partition attribute bits are nonzero')
        try:
            name = part.name.encode('utf-16-le')
        except UnicodeError as error:
            raise ValueError('invalid partition name') from error
        if len(name) > 72 or '\0' in part.name:
            raise ValueError('partition name exceeds 36 UTF-16 code units or contains NUL')
    ordered = sorted(parts, key=lambda p: p.first)
    if any(a.last >= b.first for a, b in zip(ordered, ordered[1:])):
        raise ValueError('overlapping partitions')


def plan(capacity, block_size=512, alignment=1 << 20, esp=256 << 20, system=1 << 30, recovery=256 << 20, minimum_data=64 << 20):
    blocks, first, last = geometry(capacity, block_size)
    if not isinstance(alignment, int) or alignment < block_size or alignment > U64 or alignment & (alignment - 1) or alignment % block_size:
        raise ValueError('alignment must be a power of two and a whole logical block')
    for size in (esp, system, recovery, minimum_data):
        if not isinstance(size, int) or size < 0 or size > U64 or size % block_size:
            raise ValueError('partition sizes must be bounded whole-block byte counts')
    if esp == 0 or system == 0 or minimum_data == 0:
        raise ValueError('ESP, both system slots and minimum data must be nonempty')
    quantum = alignment // block_size
    align = lambda lba: (lba + quantum - 1) // quantum * quantum
    parts = []
    cursor = align(first)
    for name, kind, size in [('Next ESP', ESP, esp), ('Next System A', TYPES['system'], system), ('Next System B', TYPES['system'], system)]:
        end = cursor + size // block_size - 1
        if end > last:
            raise ValueError('capacity is insufficient for requested layout')
        parts.append(Partition(name, kind, uuid.uuid4(), cursor, end))
        cursor = align(end + 1)
    if recovery:
        recovery_start = ((last + 1 - recovery // block_size) // quantum) * quantum
        data_end = recovery_start - 1
    else:
        recovery_start = None
        data_end = last
    if data_end < cursor or data_end - cursor + 1 < minimum_data // block_size:
        raise ValueError('capacity is insufficient for minimum data plus recovery')
    parts.append(Partition('Next Data', TYPES['data'], uuid.uuid4(), cursor, data_end))
    if recovery_start is not None:
        parts.append(Partition('Next Recovery', TYPES['recovery'], uuid.uuid4(), recovery_start, recovery_start + recovery // block_size - 1))
    validate_partitions(parts, first, last)
    return parts


def encode_header(block_size, current, backup, first, last, disk_guid, entries_lba, table_crc):
    block = bytearray(block_size)
    struct.pack_into('<8sIIIIQQQQ16sQIII', block, 0, b'EFI PART', 0x10000, 92, 0, 0,
                     current, backup, first, last, disk_guid.bytes_le, entries_lba, ENTRY_COUNT, ENTRY_SIZE, table_crc)
    struct.pack_into('<I', block, 16, crc(block[:92]))
    return bytes(block)


def encode(capacity, block_size, disk_guid, parts):
    blocks, first, last = geometry(capacity, block_size)
    if disk_guid.int == 0:
        raise ValueError('disk GUID must be nonzero')
    validate_partitions(parts, first, last)
    table = bytearray(TABLE_BYTES)
    for index, p in enumerate(parts):
        struct.pack_into('<16s16sQQQ72s', table, index * ENTRY_SIZE, p.type_guid.bytes_le, p.unique_guid.bytes_le,
                         p.first, p.last, p.attributes, p.name.encode('utf-16-le'))
    mbr = bytearray(block_size)
    struct.pack_into('<B3sB3sII', mbr, 446, 0, b'\0\2\0', 0xee, b'\xff\xff\xff', 1, min(blocks - 1, 0xffffffff))
    mbr[510:512] = b'\x55\xaa'
    backup_table = blocks - TABLE_BYTES // block_size - 1
    return {0: bytes(mbr), 1: encode_header(block_size, 1, blocks - 1, first, last, disk_guid, 2, crc(table)),
            2: bytes(table), backup_table: bytes(table), blocks - 1: encode_header(block_size, blocks - 1, 1, first, last, disk_guid, backup_table, crc(table))}


def decode_header(block, capacity, block_size, expected):
    blocks, first, last = geometry(capacity, block_size)
    if len(block) != block_size or block[:8] != b'EFI PART':
        raise ValueError('missing/truncated GPT signature')
    fields = struct.unpack_from('<8sIIIIQQQQ16sQIII', block)
    _, revision, size, checksum, reserved, current, backup, lo, hi, guid, table_lba, count, entry_size, table_crc = fields
    if revision != 0x10000 or size != 92 or reserved != 0 or any(block[92:]):
        raise ValueError('unsupported GPT header revision/size/reserved bytes')
    check = bytearray(block[:92]);check[16:20] = b'\0' * 4
    if crc(check) != checksum:
        raise ValueError('GPT header CRC mismatch')
    other = blocks - 1 if expected == 1 else 1
    table_expected = 2 if expected == 1 else blocks - TABLE_BYTES // block_size - 1
    if current != expected or backup != other or (lo, hi) != (first, last) or table_lba != table_expected:
        raise ValueError('GPT header locations/usable bounds differ from bounded profile')
    if count != ENTRY_COUNT or entry_size != ENTRY_SIZE:
        raise ValueError('unsupported GPT entry dimensions')
    disk_guid = uuid.UUID(bytes_le=guid)
    if disk_guid.int == 0:
        raise ValueError('zero disk GUID')
    return disk_guid, table_lba, table_crc


def decode_table(table, capacity, block_size, checksum):
    _, first, last = geometry(capacity, block_size)
    if len(table) != TABLE_BYTES or crc(table) != checksum:
        raise ValueError('GPT entry table length/CRC mismatch')
    parts = []
    for at in range(0, TABLE_BYTES, ENTRY_SIZE):
        raw = table[at:at + ENTRY_SIZE]
        type_id, unique, start, end, attributes, name_bytes = struct.unpack('<16s16sQQQ72s', raw)
        if type_id == bytes(16):
            if any(raw):
                raise ValueError('nonzero unused GPT entry')
            continue
        try:
            decoded = name_bytes.decode('utf-16-le')
        except UnicodeError as error:
            raise ValueError('invalid UTF-16 partition name') from error
        name, _, padding = decoded.partition('\0')
        if any(c != '\0' for c in padding):
            raise ValueError('noncanonical partition-name padding')
        parts.append(Partition(name, uuid.UUID(bytes_le=type_id), uuid.UUID(bytes_le=unique), start, end, attributes))
    validate_partitions(parts, first, last)
    return parts


def inspect(read, capacity, block_size):
    blocks, _, _ = geometry(capacity, block_size)
    mbr = read(0, block_size)
    if len(mbr) != block_size or mbr[510:512] != b'\x55\xaa':
        raise ValueError('missing protective MBR')
    record = struct.unpack_from('<B3sB3sII', mbr, 446)
    if record[0] != 0 or record[2] != 0xee or record[4:] != (1, min(blocks - 1, 0xffffffff)) or any(mbr[462:510]):
        raise ValueError('invalid/hybrid protective MBR')
    headers = [decode_header(read(lba * block_size, block_size), capacity, block_size, lba) for lba in (1, blocks - 1)]
    tables = [read(header[1] * block_size, TABLE_BYTES) for header in headers]
    parts = [decode_table(table, capacity, block_size, header[2]) for header, table in zip(headers, tables)]
    if headers[0][0] != headers[1][0] or tables[0] != tables[1]:
        raise ValueError('primary and backup GPT copies disagree')
    return {'disk_guid': str(headers[0][0]), 'capacity_bytes': capacity, 'block_size': block_size,
            'partitions': [p.json() for p in parts[0]], 'both_copies_valid': True,
            'scope': 'Strict bounded GPT profile inspection only; does not inspect filesystems, signatures, encryption or installability.'}


def open_target(path, block_size):
    fd = os.open(path, os.O_RDONLY)
    try:
        info = os.fstat(fd)
        if stat.S_ISREG(info.st_mode):
            capacity = info.st_size
        elif stat.S_ISBLK(info.st_mode):
            import fcntl
            capacity = struct.unpack('Q', fcntl.ioctl(fd, 0x80081272, bytes(8)))[0]
            observed_block = struct.unpack('I', fcntl.ioctl(fd, 0x1268, bytes(4)))[0]
            if observed_block != block_size:
                raise ValueError('requested block size differs from target logical block size')
        else:
            raise ValueError('target must be a regular image or block device')
        return fd, capacity
    except BaseException:
        os.close(fd)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true')
    sub = parser.add_subparsers(dest='command')
    show = sub.add_parser('inspect');show.add_argument('target', type=Path);show.add_argument('--block-size', type=int, default=512)
    dry = sub.add_parser('plan');dry.add_argument('--capacity-bytes', type=int);dry.add_argument('--target', type=Path)
    dry.add_argument('--block-size', type=int, default=512);dry.add_argument('--alignment-bytes', type=int, default=1 << 20)
    dry.add_argument('--esp-bytes', type=int, default=256 << 20);dry.add_argument('--system-bytes', type=int, default=1 << 30)
    dry.add_argument('--recovery-bytes', type=int, default=256 << 20);dry.add_argument('--minimum-data-bytes', type=int, default=64 << 20)
    args = parser.parse_args()
    if args.self_test:
        return 0 if unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(GptTests)).wasSuccessful() else 1
    if not args.command:
        parser.error('choose plan or inspect, or --self-test')
    fd = None
    try:
        if args.block_size not in (512, 4096):
            raise ValueError('only 512 or 4096-byte logical blocks are supported')
        if args.command == 'inspect':
            fd, capacity = open_target(args.target, args.block_size)
            result = inspect(lambda offset, size: os.pread(fd, size, offset), capacity, args.block_size)
        else:
            target_info = None
            if args.target is not None:
                fd, observed = open_target(args.target, args.block_size)
                if args.capacity_bytes is not None and args.capacity_bytes != observed:
                    raise ValueError('requested capacity differs from actual target')
                capacity = observed
                # Existing GPT must validate; unknown nonblank boot metadata fails closed.
                head = os.pread(fd, args.block_size * 2, 0)
                if any(head):
                    target_info = inspect(lambda offset, size: os.pread(fd, size, offset), capacity, args.block_size)
                else:
                    target_info = {'scope': 'Initial metadata is zero; this does not prove the remaining target is empty.'}
            else:
                capacity = args.capacity_bytes
            if capacity is None:
                raise ValueError('plan requires capacity-bytes or a read-only target')
            parts = plan(capacity, args.block_size, args.alignment_bytes, args.esp_bytes, args.system_bytes, args.recovery_bytes, args.minimum_data_bytes)
            disk = uuid.uuid4();metadata = encode(capacity, args.block_size, disk, parts)
            result = {'dry_run': True, 'writes_performed': False, 'capacity_bytes': capacity, 'block_size': args.block_size,
                      'alignment_bytes': args.alignment_bytes, 'disk_guid': str(disk), 'partitions': [p.json() for p in parts],
                      'metadata': [{'first_lba': lba, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()} for lba, data in sorted(metadata.items())],
                      'existing_target': target_info,
                      'scope': 'Proposed layout only. Existing partitions/data are not preserved by this proposal. No target writes, filesystem formatting, boot assets, accounts, encryption, A/B rollback or installation. Next type GUIDs are experimental project identifiers.'}
        print(json.dumps(result, indent=2));return 0
    except (ValueError, OSError, OverflowError, struct.error) as error:
        print(json.dumps({'error': str(error), 'writes_performed': False}));return 1
    finally:
        if fd is not None:os.close(fd)


class GptTests(unittest.TestCase):
    def setUp(self):
        self.capacity = 4 << 30;self.block = 512
        self.parts = plan(self.capacity)
        self.metadata = encode(self.capacity, self.block, uuid.uuid4(), self.parts)

    def reader(self, metadata=None):
        metadata = self.metadata if metadata is None else metadata
        def read(offset, size):
            for lba, data in metadata.items():
                start = lba * self.block
                if start <= offset and offset + size <= start + len(data):return data[offset-start:offset-start+size]
            return b''
        return read

    def test_roundtrip_primary_backup_guid_crc_and_aligned_layout(self):
        for block in (512, 4096):
            self.block = block
            parts = plan(self.capacity, block)
            metadata = encode(self.capacity, block, uuid.uuid4(), parts)
            report = inspect(self.reader(metadata), self.capacity, block)
            self.assertEqual(report['partitions'], [p.json() for p in parts])
            self.assertTrue(all(p.first * block % (1 << 20) == 0 for p in parts))
            self.assertEqual(len(parts), 5)
        self.assertEqual(len(plan(self.capacity, recovery=0)), 4)

    def test_capacity_alignment_size_and_overflow_reject(self):
        for capacity in [-1, 0, 513, U64 + 1, 1 << 20]:
            with self.assertRaises(ValueError):plan(capacity)
        for block in [0, 256, 1000, 8192]:
            with self.assertRaises(ValueError):plan(self.capacity, block)
        for alignment in [0, 511, 513, 3 << 20, U64 + 1]:
            with self.assertRaises(ValueError):plan(self.capacity, alignment=alignment)
        for size in [-1, 1, U64 + 1]:
            with self.assertRaises(ValueError):plan(self.capacity, system=size)
        with self.assertRaises(ValueError):plan(self.capacity, esp=0)

    def test_corruption_truncation_and_inconsistent_copies_reject(self):
        for lba, data in self.metadata.items():
            for index in sorted(set([0, len(data)//2, len(data)-1])):
                changed = self.metadata.copy();bad = bytearray(data);bad[index] ^= 1;changed[lba] = bytes(bad)
                # Protective MBR bootstrap bytes have no integrity contract.
                if lba == 0 and index not in range(446, 512):continue
                with self.assertRaises(ValueError):inspect(self.reader(changed), self.capacity, self.block)
            changed = self.metadata.copy();changed[lba] = data[:-1]
            with self.assertRaises(ValueError):inspect(self.reader(changed), self.capacity, self.block)
        different = encode(self.capacity, self.block, uuid.uuid4(), self.parts)
        changed = self.metadata.copy();last = self.capacity//self.block-1;changed[last] = different[last]
        with self.assertRaisesRegex(ValueError, 'disagree'):inspect(self.reader(changed), self.capacity, self.block)

    def test_semantically_hostile_entries_with_valid_crc_reject(self):
        table = self.metadata[2]
        mutations = [(32, struct.pack('<Q', self.parts[0].last + 1)),
                     (16, bytes(16)), (ENTRY_SIZE + 16, self.parts[0].unique_guid.bytes_le),
                     (ENTRY_SIZE + 32, struct.pack('<Q', self.parts[0].first)),
                     (5 * ENTRY_SIZE + 16, b'\x01'), (56, b'\0\xd8'), (48, struct.pack('<Q', 8))]
        for offset, payload in mutations:
            bad = bytearray(table);bad[offset:offset+len(payload)] = payload
            with self.assertRaises(ValueError):decode_table(bytes(bad), self.capacity, self.block, crc(bad))

    def test_every_header_prefix_and_untrusted_dimensions_reject(self):
        header = self.metadata[1]
        for length in range(len(header)):
            with self.assertRaises(ValueError):decode_header(header[:length], self.capacity, self.block, 1)
        for offset, value in [(12, 0xffffffff), (80, 0xffffffff), (84, 0xffffffff), (72, U64)]:
            bad = bytearray(header);struct.pack_into('<Q' if offset == 72 else '<I', bad, offset, value)
            bad[16:20] = bytes(4);struct.pack_into('<I', bad, 16, crc(bad[:92]))
            with self.assertRaises(ValueError):decode_header(bytes(bad), self.capacity, self.block, 1)

    def test_actual_cli_readonly_inspect_plan_and_failure_preserve_file(self):
        # A small complete GPT permits whole-image hashes, not metadata-only hashes.
        capacity = 8 << 20
        parts = plan(capacity, esp=1 << 20, system=1 << 20, recovery=1 << 20, minimum_data=1 << 20)
        metadata = encode(capacity, 512, uuid.uuid4(), parts)
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp) / 'disk.img'
            image = bytearray(capacity)
            for lba, data in metadata.items():
                image[lba*512:lba*512+len(data)] = data
            target.write_bytes(image)
            script = str(Path(__file__).resolve())
            digest_before = hashlib.sha256(target.read_bytes()).hexdigest()
            commands = [(['inspect', str(target)], 0),
                        (['plan', '--target', str(target), '--esp-bytes', str(1 << 20), '--system-bytes', str(1 << 20), '--recovery-bytes', str(1 << 20), '--minimum-data-bytes', str(1 << 20)], 0),
                        (['plan', '--target', str(target), '--capacity-bytes', str(capacity + 512)], 1)]
            for arguments, expected in commands:
                completed = subprocess.run([sys.executable, script, *arguments], capture_output=True, text=True, timeout=5)
                self.assertEqual(completed.returncode, expected, completed.stdout + completed.stderr)
                self.assertIsInstance(json.loads(completed.stdout), dict)
                self.assertEqual(hashlib.sha256(target.read_bytes()).hexdigest(), digest_before)
            image[512 + 16] ^= 1
            target.write_bytes(image)
            corrupted_hash = hashlib.sha256(target.read_bytes()).hexdigest()
            completed = subprocess.run([sys.executable, script, 'inspect', str(target)], capture_output=True, text=True, timeout=5)
            self.assertEqual(completed.returncode, 1)
            self.assertIn('CRC', json.loads(completed.stdout)['error'])
            self.assertEqual(hashlib.sha256(target.read_bytes()).hexdigest(), corrupted_hash)


if __name__ == '__main__':raise SystemExit(main())
