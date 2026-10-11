"""The packer must match the Rust parser's wire format and size policy."""
import struct
import unittest
import zlib
from initrd import MAX_ENTRIES, MAX_IMAGE_SIZE, pack, signed_entries
from pathlib import Path
import tempfile


class InitImageTests(unittest.TestCase):
    def test_deterministic_sorted_contiguous_wire_format(self):
        entries = [('/system/z.elf', b'z!'), ('/system/nextd.elf', b'next')]
        image = pack(entries)
        self.assertEqual(image, pack(list(reversed(entries))))
        self.assertEqual(struct.unpack_from('<8sIIQ', image), (b'NEXTINIT', 1, 2, len(image)))
        first, off, size = struct.unpack_from('<64sQQ', image, 32)
        self.assertEqual(first.rstrip(b'\0'), b'/system/nextd.elf')
        self.assertEqual((off, size, image[off:off + size]), (192, 4, b'next'))
        _, second_off, second_size = struct.unpack_from('<64sQQ', image, 112)
        self.assertEqual((second_off, second_size), (196, 2))
        checksum = struct.unpack_from('<I', image, 24)[0]
        canonical = bytearray(image)
        canonical[24:28] = bytes(4)
        self.assertEqual(checksum, zlib.crc32(canonical))

    def test_reject_paths_duplicates_and_empty_payloads(self):
        for path in ['', '/', '/a//b', '/a/../b', '/a/./b', 'relative', '/a\\b', '/é', '/' + 'a' * 63]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                pack([(path, b'x')])
        for entries in [[], [('/a', b'')], [('/a', b'x'), ('/a', b'y')], [(f'/a{i}', b'x') for i in range(MAX_ENTRIES + 1)]]:
            with self.assertRaises(ValueError):
                pack(entries)

    def test_exact_entry_limit_and_complete_table_payload_bounds(self):
        entries = [(f'/a{i:02}', bytes([i + 1])) for i in range(MAX_ENTRIES)]
        image = pack(entries)
        self.assertEqual(MAX_ENTRIES, 64)
        self.assertEqual(struct.unpack_from('<I', image, 12)[0], MAX_ENTRIES)
        table_end = 32 + MAX_ENTRIES * 80
        self.assertEqual(len(image), table_end + MAX_ENTRIES)
        for index in range(MAX_ENTRIES):
            path, offset, size = struct.unpack_from('<64sQQ', image, 32 + index * 80)
            self.assertEqual(path.rstrip(b'\0'), entries[index][0].encode('ascii'))
            self.assertEqual((offset, size), (table_end + index, 1))
            self.assertEqual(image[offset:offset + size], entries[index][1])
        with self.assertRaisesRegex(ValueError, '1..64'):
            pack(entries + [('/overflow', b'x')])

    def test_signed_mirror_requires_application_envelopes_and_ignores_auxiliary_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / 'system').mkdir()
            target = directory / 'system/nextd.elf'
            with self.assertRaises(ValueError):
                signed_entries(directory)
            target.write_bytes(b'\x7fELF' + bytes(512))
            with self.assertRaisesRegex(ValueError, 'Raw or non-Application'):
                signed_entries(directory)
            envelope = bytearray(449); envelope[:8] = b'NXTAPP01'; envelope[164] = 1
            target.write_bytes(envelope)
            (directory / 'system/private.seed').write_bytes(b'not an app')
            entries = signed_entries(directory)
            self.assertEqual(entries, [('/system/nextd.elf', bytes(envelope))])
            envelope[164] = 2; target.write_bytes(envelope)
            with self.assertRaises(ValueError):
                signed_entries(directory)

    def test_exact_limit_and_one_byte_over(self):
        image = pack([('/a', b'x' * (MAX_IMAGE_SIZE - 112))])
        self.assertEqual(len(image), MAX_IMAGE_SIZE)
        with self.assertRaises(ValueError):
            pack([('/a', b'x' * (MAX_IMAGE_SIZE - 111))])


if __name__ == '__main__':
    unittest.main()
