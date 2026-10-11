#!/usr/bin/env python3
"""Check built development app envelopes against raw fixtures and owner public key."""
import hashlib
from pathlib import Path
import os
import subprocess
import unittest
from initrd import ROOT, pack, signed_entries


class SignedApplications(unittest.TestCase):
    def test_all_profiles_signed_digests_match_raw_and_negative_signature_rejected(self):
        entries = signed_entries(ROOT / 'build/apps')
        raw_count = len(list((ROOT / 'build/user').glob('*.elf'))) + len(list((ROOT / 'build/compat/linux').glob('*.elf'))) + len(list((ROOT / 'build/compat/nt').glob('*.exe')))
        self.assertEqual(len(entries), raw_count + 1)
        self.assertLessEqual(len(entries), 64)
        self.assertLessEqual(len(pack(entries)), 16 * 1024 * 1024)
        # Source13's second client uses the same protocol implementation with
        # a bounded pre-publication delay, and must pass the same signing path.
        if (ROOT / 'build/user/client-input-delayed.elf').exists():
            self.assertIn('/system/client-input-delayed.elf', {path for path, _ in entries})
            self.assertIn('/system/client-input-probe.elf', {path for path, _ in entries})
        if (ROOT / 'build/user/client-reconfigure-probe.elf').exists():
            self.assertIn('/system/client-reconfigure-probe.elf', {path for path, _ in entries})
        public = os.environ.get('NEXT_DEV_PUBLIC_KEY', str(ROOT / 'work/package-dev.pub'))
        tool = ROOT / 'target/debug/next-package'
        identities = set()
        for path, bundle in entries:
            envelope = ROOT / 'build/apps' / path.lstrip('/')
            before = hashlib.sha256(envelope.read_bytes()).digest()
            run = subprocess.run([str(tool), 'verify', public, str(envelope)],
                                 capture_output=True, text=True, check=False)
            self.assertEqual(hashlib.sha256(envelope.read_bytes()).digest(), before)
            if path == '/system/untrusted.elf':
                self.assertNotEqual(run.returncode, 0)
                self.assertIn('Signature', run.stderr)
                continue
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(int.from_bytes(bundle[40:48], 'little'), 0x3f)
            self.assertEqual(int.from_bytes(bundle[164:166], 'little'), 1)
            self.assertEqual(int.from_bytes(bundle[32:36], 'little'), 2 if path == '/system/nextd.elf' else 1)
            identity = bundle[96:160].split(b'\0')[0].decode('ascii')
            self.assertNotIn(identity, identities); identities.add(identity)
            name = Path(path).name
            if path.startswith('/system/'):
                source = ROOT / 'build/user' / name; profile = 'native'
            elif path.startswith('/compat/linux/'):
                source = ROOT / 'build/compat/linux' / name; profile = 'linux'
            else:
                source = ROOT / 'build/compat/nt' / name; profile = 'nt'
            self.assertEqual(identity, f'org.next.development.{profile}.{Path(name).stem}')
            self.assertEqual(bundle[256:320].split(b'\0')[0], f'bin/{name}'.encode())
            offset = int.from_bytes(bundle[320:328], 'little')
            size = int.from_bytes(bundle[328:336], 'little')
            raw = source.read_bytes()
            self.assertEqual(bundle[offset:offset + size], raw)
            self.assertEqual(bundle[336:368], hashlib.sha256(raw).digest())
            if path == '/system/nextd.elf':
                from service_config import config
                record = 256 + 128
                self.assertEqual(bundle[record:record + 64].split(b'\0')[0], b'config/services.bin')
                offset = int.from_bytes(bundle[record + 64:record + 72], 'little')
                size = int.from_bytes(bundle[record + 72:record + 80], 'little')
                expected = config((ROOT / 'build/user/service-worker.elf').read_bytes(), 1)
                self.assertEqual(bundle[offset:offset + size], expected)
                self.assertEqual(bundle[record + 80:record + 112], hashlib.sha256(expected).digest())


if __name__ == '__main__':
    unittest.main()
