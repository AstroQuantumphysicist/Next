#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
import cow_recover as recovery
import cow_verify as cv
from test_cow_verify import golden, START, PROTECTED


class Recovery(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'source.img'
        self.source.write_bytes(golden())
        self.before = self.source.read_bytes()
        self.addCleanup(self.assert_source_unchanged)

    def assert_source_unchanged(self):
        self.assertEqual(self.source.read_bytes(), self.before)

    def export(self, destination='export', **options):
        return recovery.export(self.source, self.root / destination,
                               START, cv.REQUIRED, PROTECTED, **options)

    def test_verified_current_and_explicit_older_generation_private_files(self):
        for generation, payload in [(None, b'new'), (1, b'old')]:
            destination = f'generation-{generation}'
            report = self.export(destination, generation=generation)
            directory = self.root / destination
            self.assertEqual(recovery.verify_export(directory), report)
            self.assertEqual((directory / 'slot-0.bin').read_bytes(), payload)
            self.assertEqual(report['source_sha256'], hashlib.sha256(self.before).hexdigest())
            self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
            self.assertTrue(all(stat.S_IMODE(p.stat().st_mode) == 0o600 for p in directory.iterdir()))
            self.assertFalse((directory / 'INCOMPLETE').exists())

    def test_corrupt_latest_falls_back_but_unverified_generation_never_exports(self):
        image = bytearray(self.before); image[(START + 2 + 65) * 512] ^= 1
        self.source.write_bytes(image); self.before = bytes(image)
        report = self.export()
        self.assertEqual(report['generation'], 1)
        self.assertEqual((self.root / 'export/slot-0.bin').read_bytes(), b'old')
        with self.assertRaisesRegex(cv.Invalid, 'not a verified'):
            self.export('unverified', generation=2)
        self.assertFalse((self.root / 'unverified').exists())

    def test_both_corrupt_unknown_metadata_and_overlap_create_nothing(self):
        for image in [bytes(len(self.before)), bytes([165]) * len(self.before)]:
            self.source.write_bytes(image); self.before = image
            with self.assertRaises(cv.Invalid): self.export()
            self.assertFalse((self.root / 'export').exists())
        self.source.write_bytes(golden()); self.before = bytes(golden())
        with self.assertRaisesRegex(cv.Invalid, 'protected'):
            recovery.export(self.source, self.root / 'export', START, cv.REQUIRED, [(START,1)])
        self.assertFalse((self.root / 'export').exists())

    def test_existing_destinations_and_symlink_ancestors_are_never_overwritten(self):
        target = self.root / 'export'; target.mkdir(); (target / 'unknown').write_bytes(b'keep')
        with self.assertRaises(FileExistsError): self.export()
        self.assertEqual((target / 'unknown').read_bytes(), b'keep')
        (self.root / 'alias').symlink_to(target, target_is_directory=True)
        with self.assertRaises(OSError): self.export('alias/child')
        self.assertFalse((target / 'child').exists())
        (self.root / 'target-link').symlink_to(target, target_is_directory=True)
        with self.assertRaises(FileExistsError): self.export('target-link')

    def test_group_writable_parent_rejected_without_output(self):
        parent = self.root / 'unsafe'; parent.mkdir(mode=0o770); parent.chmod(0o770)
        with self.assertRaisesRegex(cv.Invalid, 'not group/world writable'):
            self.export('unsafe/export')
        self.assertFalse((parent / 'export').exists())

    def test_each_publish_fault_retains_incomplete_and_no_unknown_data_removed(self):
        for stage in ['incomplete', 'slot-0.bin', 'manifest-staged', 'manifest-published']:
            def fail(current):
                if current == stage: raise OSError('injected interruption')
            with self.assertRaisesRegex(OSError, 'injected'):
                self.export(stage, checkpoint=fail)
            output = self.root / stage
            self.assertTrue((output / 'INCOMPLETE').is_file())
            (output / 'user-evidence').write_bytes(b'preserve')
            with self.assertRaisesRegex(cv.Invalid, 'incomplete'):
                recovery.verify_export(output)
            self.assertEqual((output / 'user-evidence').read_bytes(), b'preserve')

    def test_real_process_exit_before_completion_cannot_look_like_verified_export(self):
        script = '''import os,sys
sys.path.insert(0,sys.argv[1])
import cow_recover
cow_recover.export(sys.argv[2],sys.argv[3],10,132,[(0,10)],checkpoint=lambda stage:os._exit(42) if stage=='manifest-published' else None)
'''
        destination = self.root / 'crashed'
        run = subprocess.run([sys.executable, '-c', script, str(Path(recovery.__file__).parent),
                              str(self.source), str(destination)], check=False)
        self.assertEqual(run.returncode, 42)
        self.assertTrue((destination / 'manifest.json').exists())
        with self.assertRaisesRegex(cv.Invalid, 'incomplete'):
            recovery.verify_export(destination)

    def test_corrupt_payload_symlink_and_unexpected_files_reject_without_repair(self):
        self.export(); directory = self.root / 'export'; data = directory / 'slot-0.bin'
        data.write_bytes(b'bad')
        with self.assertRaisesRegex(cv.Invalid, 'checksum'): recovery.verify_export(directory)
        data.unlink(); data.symlink_to(self.source)
        with self.assertRaises(OSError): recovery.verify_export(directory)
        data.unlink(); data.write_bytes(b'new'); (directory / 'unknown').write_bytes(b'preserve')
        with self.assertRaisesRegex(cv.Invalid, 'unexpected'): recovery.verify_export(directory)
        self.assertEqual((directory / 'unknown').read_bytes(), b'preserve')

    def test_manifest_paths_duplicate_keys_sizes_and_hashes_are_bounded(self):
        self.export(); directory = self.root / 'export'; manifest = directory / 'manifest.json'
        original = json.loads(manifest.read_text())
        for key, value in [('file','../outside'), ('size',4097), ('path','/a/../b'), ('sha256','bad')]:
            changed = json.loads(json.dumps(original)); changed['files'][0][key] = value
            manifest.write_text(json.dumps(changed))
            with self.assertRaises(cv.Invalid): recovery.verify_export(directory)
        manifest.write_text('{"format":1,"format":2}')
        with self.assertRaisesRegex(cv.Invalid, 'duplicate'): recovery.verify_export(directory)
        self.assertFalse((self.root / 'outside').exists())

    def test_changed_source_and_deeply_nested_manifest_fail_closed(self):
        from unittest.mock import patch
        with patch.object(cv, 'digest', side_effect=['a' * 64, 'b' * 64]):
            with self.assertRaisesRegex(cv.Invalid, 'source changed'):
                self.export('changed')
        self.assertFalse((self.root / 'changed').exists())
        self.export()
        manifest = self.root / 'export/manifest.json'
        manifest.write_bytes(b'[' * 2000 + b']' * 2000)
        with self.assertRaisesRegex(cv.Invalid, 'nested|unknown recovery manifest'):
            recovery.verify_export(self.root / 'export')

    def test_fifo_entries_do_not_block_and_cli_verifies_without_source_write(self):
        self.export(); directory = self.root / 'export'; data = directory / 'slot-0.bin'
        data.unlink(); os.mkfifo(data)
        with self.assertRaisesRegex(cv.Invalid, 'regular'): recovery.verify_export(directory)
        run = subprocess.run([sys.executable, str(Path(recovery.__file__)), 'verify', str(directory)],
                             capture_output=True, timeout=5, check=False)
        self.assertEqual(run.returncode, 1)


if __name__ == '__main__':
    unittest.main()
