"""Independent read-only checks of the real Rust in-memory fixture producer."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import cow_verify

ROOT = Path(__file__).resolve().parents[1]

class JournalFixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        subprocess.run(['bash', 'tools/build-journal-fixture.sh'], cwd=ROOT,
                       check=True, capture_output=True)

    def produce(self, destination, start=100, capacity=1000):
        return subprocess.run([str(ROOT / 'work/journal-fixture'), str(start),
                               str(capacity), str(destination)], capture_output=True, timeout=5)

    def test_real_journal_blob_verifies_at_exact_geometry(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'work') as temporary:
            destination = Path(temporary) / 'extent.bin'
            self.assertEqual(self.produce(destination).returncode, 0)
            data = destination.read_bytes()
            self.assertEqual(len(data), 132 * 512)
            report = cow_verify.verify_bytes(data, 100, 132, [(0, 100), (232, 768)])
            self.assertEqual(report['selected_generation'], 1)
            self.assertEqual(report['files'], [])
            self.assertFalse(report['mount_read_only'])
            with self.assertRaises(cow_verify.Invalid):
                cow_verify.verify_bytes(data, 101, 132, [])

    def test_never_overwrites_existing_files_symlinks_or_fifo(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'work') as temporary:
            directory = Path(temporary)
            existing = directory / 'existing.img'
            existing.write_bytes(b'caller media unchanged')
            self.assertNotEqual(self.produce(existing).returncode, 0)
            self.assertEqual(existing.read_bytes(), b'caller media unchanged')
            link = directory / 'link'
            link.symlink_to(existing)
            self.assertNotEqual(self.produce(link).returncode, 0)
            self.assertEqual(existing.read_bytes(), b'caller media unchanged')
            fifo = directory / 'fifo'
            os.mkfifo(fifo)
            self.assertNotEqual(self.produce(fifo).returncode, 0)
            invalid = directory / 'invalid'
            for start, capacity in [(0, 1000), (999, 1000), ((1 << 64) - 1, 1000)]:
                self.assertNotEqual(self.produce(invalid, start, capacity).returncode, 0)
                self.assertFalse(invalid.exists())

if __name__ == '__main__':
    unittest.main()
