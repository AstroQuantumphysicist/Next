#!/usr/bin/env python3
"""The readonly query oracle must bind every source it compiles."""
import json
from pathlib import Path
import tempfile
import unittest
import files_query_host as host

class BindingTests(unittest.TestCase):
    def fixture(self, root):
        records = {}
        for relative in ['crates/prism/src/lib.rs', 'crates/next-fs/src/lib.rs', host.BACKEND]:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('// retained source\n')
            records[relative] = host.digest(path)
        manifest = root / 'manifest.json'
        manifest.write_text(json.dumps(records))
        return manifest, records

    def test_exact_complete_source_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, records = self.fixture(root)
            self.assertEqual(host.verified_sources(manifest, root), records)

    def test_changed_backend_or_missing_crate_fails_before_compile(self):
        for relative in [host.BACKEND, 'crates/next-fs/src/lib.rs', 'crates/prism/src/lib.rs']:
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                manifest, _ = self.fixture(root)
                (root / relative).write_text('// different source\n')
                with self.assertRaisesRegex(RuntimeError, 'differs from manifest'):
                    host.verified_sources(manifest, root)

    def test_unmanifested_module_or_missing_backend_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, records = self.fixture(root)
            extra = root / 'crates/next-fs/src/extra.rs'
            extra.write_text('// malicious module\n')
            with self.assertRaisesRegex(RuntimeError, 'unmanifested'):
                host.verified_sources(manifest, root)
            extra.unlink()
            records.pop(host.BACKEND)
            manifest.write_text(json.dumps(records))
            with self.assertRaisesRegex(RuntimeError, 'lacks query backend'):
                host.verified_sources(manifest, root)

    def test_source_symlink_cannot_escape_retained_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'retained'
            root.mkdir()
            manifest, records = self.fixture(root)
            outside = root.parent / 'outside.rs'
            outside.write_text('// retained source\n')
            target = root / host.BACKEND
            target.unlink()
            target.symlink_to(outside)
            self.assertEqual(host.digest(outside), records[host.BACKEND])
            with self.assertRaisesRegex(RuntimeError, 'differs from manifest'):
                host.verified_sources(manifest, root)

if __name__ == '__main__':
    unittest.main()
