#!/usr/bin/env python3
"""Independent golden encodings and hostile-media tests; every run hashes source."""
import hashlib
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
import cow_verify as cv

START = 10
PROTECTED = [(0, START)]


def seal(block):
    struct.pack_into("<I", block, 508, cv.crc(block[:508]))
    return block


def root(generation, files):
    block = bytearray(512)
    struct.pack_into("<8sI4xQI4x", block, 0, b"NXTRT001", 1, generation, len(files))
    for index, (name, data) in enumerate(files):
        at = 32 + index * 56
        block[at:at + len(name)] = name
        struct.pack_into("<II", block, at + 32, len(data), cv.crc(data))
    return seal(block)


def superblock(generation, bank, checksum):
    block = bytearray(512)
    struct.pack_into("<8sIIQIIQQI", block, 0, b"NXTCOW01", 1, 512,
                     generation, bank, 8, START, cv.REQUIRED, checksum)
    return seal(block)


def golden():
    image = bytearray(160 * 512)
    for bank, generation, text in [(0, 1, b"old"), (1, 2, b"new")]:
        relative = 2 + bank * 65
        data_at = (START + relative) * 512
        image[data_at:data_at + len(text)] = text
        metadata = root(generation, [(b"/a", text)])
        root_at = (START + relative + 64) * 512
        image[root_at:root_at + 512] = metadata
        at = (START + bank) * 512
        image[at:at + 512] = superblock(generation, bank, cv.number(metadata, 508))
    return image


class Verification(unittest.TestCase):
    def run_image(self, image, expected=None, **kwargs):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "scratch.img"
            path.write_bytes(image)
            before = hashlib.sha256(path.read_bytes()).digest()
            try:
                if expected:
                    with self.assertRaisesRegex(cv.Invalid, expected):
                        cv.verify(path, START, cv.REQUIRED, kwargs.get("protected", PROTECTED))
                    return None
                return cv.verify(path, START, cv.REQUIRED, kwargs.get("protected", PROTECTED))
            finally:
                self.assertEqual(hashlib.sha256(path.read_bytes()).digest(), before)

    def test_golden_latest_and_all_candidates_checked(self):
        report = self.run_image(golden())
        self.assertEqual(report["selected_generation"], 2)
        self.assertFalse(report["mount_read_only"])
        self.assertTrue(all(c["snapshot_valid"] for c in report["candidates"]))
        self.assertEqual(report["files"][0]["path"], "/a")

    def test_torn_super_fallback_readonly_but_zero_inactive_is_not_corruption(self):
        image = golden(); image[(START + 1) * 512 + 508] ^= 1
        report = self.run_image(image)
        self.assertEqual(report["selected_generation"], 1)
        self.assertTrue(report["mount_read_only"])
        image[(START + 1) * 512:(START + 2) * 512] = bytes(512)
        report = self.run_image(image)
        self.assertFalse(report["mount_read_only"])

    def test_bad_latest_root_and_data_fall_back_readonly(self):
        for offset in [(START + 2 + 65 + 64) * 512 + 12, (START + 2 + 65) * 512]:
            image = golden(); image[offset] ^= 1
            report = self.run_image(image)
            self.assertEqual(report["selected_generation"], 1)
            self.assertTrue(report["mount_read_only"])
            self.assertFalse(report["candidates"][1]["snapshot_valid"])

    def test_nonzero_payload_tail_and_old_inactive_corruption_reported(self):
        image = golden(); image[(START + 2) * 512 + 4000] = 1
        report = self.run_image(image)
        self.assertFalse(report["candidates"][0]["snapshot_valid"])
        self.assertEqual(report["selected_generation"], 2)
        self.assertFalse(report["mount_read_only"])

    def test_unknown_blank_truncated_and_extent_overlap_rejected(self):
        self.run_image(bytes(160 * 512), "no valid committed")
        self.run_image(bytes([165]) * (160 * 512), "no valid committed")
        self.run_image(golden()[:-1], "truncated media")
        self.run_image(golden()[:141 * 512], "extent outside")
        self.run_image(golden(), "protected range overlap", protected=[(START + 131, 2)])
        with self.assertRaisesRegex(cv.Invalid, "overflow"):
            cv.verify_bytes(bytes(132 * 512), cv.MAX_U64, 132, [])

    def test_every_committed_payload_corrupt_fails_closed(self):
        image = golden()
        for bank in (0, 1):
            image[(START + 2 + bank * 65) * 512] ^= 1
        self.run_image(image, "no valid committed snapshot.*payload CRC")

    def test_conflicting_equal_generations_rejected(self):
        image = golden()
        r = root(1, [(b"/a", b"new")])
        at = (START + 2 + 65 + 64) * 512; image[at:at + 512] = r
        at = (START + 1) * 512; image[at:at + 512] = superblock(1, 1, cv.number(r, 508))
        self.run_image(image, "conflicting same-generation")

    def test_canonical_metadata_and_root_crc_binding(self):
        for name in [b"/a/../b", b"/a//b", b"/a/", b"relative", b"/\xff"]:
            r = root(2, [(name, b"new")])
            image = golden(); at = (START + 2 + 65 + 64) * 512; image[at:at + 512] = r
            at = (START + 1) * 512; image[at:at + 512] = superblock(2, 1, cv.number(r, 508))
            report = self.run_image(image)
            self.assertEqual(report["selected_generation"], 1)
        image = golden(); at = (START + 2 + 65 + 64) * 512
        block = image[at:at + 512]; block[12] = 1; seal(block); image[at:at + 512] = block
        report = self.run_image(image)
        self.assertFalse(report["candidates"][1]["snapshot_valid"])

    def test_cli_readonly_success_and_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "disk.img"
            for image, expected_code in [(golden(), 0), (bytes(160 * 512), 1)]:
                path.write_bytes(image); before = path.read_bytes()
                run = subprocess.run([sys.executable, str(Path(cv.__file__)), str(path),
                                      "--start-lba", "10", "--sectors", "132", "--protect", "0:10"],
                                     capture_output=True, text=True, check=False)
                self.assertEqual(run.returncode, expected_code, run.stderr)
                self.assertEqual(path.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
