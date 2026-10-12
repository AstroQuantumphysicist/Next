"""Image construction failures must leave the existing boot artifact intact."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import image


class ImageBuildTests(unittest.TestCase):
    def fixture(self, root):
        for name in ['EFI/BOOT/BOOTX64.EFI', 'EFI/NEXT/HELIX.ELF', 'EFI/NEXT/INITRD.NXI', 'EFI/NEXT/SYSTEM.NXP']:
            file = root / 'build/esp' / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(b'test input')
        for slot in ('A', 'B'):
            for name in ('HELIX.ELF', 'INITRD.NXI', 'SYSTEM.NXP'):
                file = root / 'build/esp/EFI/NEXT' / slot / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(b'test slot input')
        destination = root / 'build/next-dev.img'
        destination.write_bytes(b'previous boot image')
        return destination

    def test_missing_tool_does_not_touch_previous_image(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = self.fixture(root)
            with patch.object(image, 'ROOT', root), patch.object(image.shutil, 'which', return_value=None):
                with self.assertRaisesRegex(SystemExit, 'source tools/env.sh'):
                    image.main()
            self.assertEqual(destination.read_bytes(), b'previous boot image')
            self.assertFalse((root / 'build/next-dev.img.tmp').exists())

    def test_each_format_or_copy_failure_preserves_previous_image(self):
        # Formatting, directory creation, and all eleven copies may fail.
        for failed_call in range(13):
            with self.subTest(failed_call=failed_call), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                destination = self.fixture(root)
                calls = 0

                def run(command, check):
                    nonlocal calls
                    self.assertTrue(check)
                    calls += 1
                    if calls == failed_call + 1:
                        raise subprocess.CalledProcessError(1, command)

                with patch.object(image, 'ROOT', root), patch.object(image.shutil, 'which', return_value='/tool'), patch.object(image.subprocess, 'run', side_effect=run):
                    with self.assertRaises(subprocess.CalledProcessError):
                        image.main()
                self.assertEqual(destination.read_bytes(), b'previous boot image')
                self.assertFalse((root / 'build/next-dev.img.tmp').exists())


if __name__ == '__main__':
    unittest.main()
