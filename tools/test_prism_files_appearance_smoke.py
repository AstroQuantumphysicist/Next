"""Host verification of gate parsers/bindings; never guest acceptance evidence."""
import hashlib
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path
import zlib
import prism_files_appearance_smoke as gate

class GateTests(unittest.TestCase):
    def test_exact_preferences_record_crc_and_reserved_fields(self):
        data=gate.record_bytes()
        self.assertEqual(len(data),32)
        self.assertEqual(data[:8],b'NXAPREF1')
        self.assertEqual(data[8:16],b'\x01\x00\x20\x00\x01\x01\x64\x00')
        self.assertEqual(data[16:28],bytes(12))
        self.assertEqual(int.from_bytes(data[28:],'little'),zlib.crc32(data[:28])&0xffffffff)

    def test_ppm_bounds_exact_binary_pixels_and_reject_truncation(self):
        image=gate.ppm(b'P6\n2 1\n255\n'+b'\x00\x20\xff\xff\x00\x01')
        self.assertEqual(gate.crop(image,(1,0,1,1)),b'\xff\x00\x01')
        for bounds in [(2,0,1,1),(-1,0,1,1),(0,0,2,2)]:
            with self.assertRaises(RuntimeError):gate.crop(image,bounds)
        with self.assertRaises(RuntimeError):gate.ppm(b'P6\n2 1\n255\n12345')

    def test_protected_hash_excludes_only_explicit_journal_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'image'
            raw=bytearray(140*512);path.write_bytes(raw)
            expected=gate.protected_digest(path,4)
            raw[5*512]=1;path.write_bytes(raw)
            self.assertEqual(gate.protected_digest(path,4),expected)
            raw[3*512]=1;path.write_bytes(raw)
            self.assertNotEqual(gate.protected_digest(path,4),expected)
            raw[3*512]=0;raw[139*512]=1;path.write_bytes(raw)
            self.assertNotEqual(gate.protected_digest(path,4),expected)

    def test_gui_rows_are_dense_metadata_order_after_slot_deletion(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'image'; raw=bytearray(132*512)
            records=[]
            for slot,name,payload in [(6,'/native/later',b'later'),(2,'/native/early',b'early')]:
                offset=(2+slot*8)*512;raw[offset:offset+len(payload)]=payload
                records.append({'slot':slot,'path':name,'size':len(payload),'crc32':zlib.crc32(payload)&0xffffffff})
            path.write_bytes(raw)
            with patch.object(gate,'verify_cow',return_value={'mount_read_only':False,'selected_bank':0,'files':records}):
                _,files=gate.journal_state(path,0)
            self.assertEqual(files['/native/early']['gui_row'],1)
            self.assertEqual(files['/native/later']['gui_row'],2)
            self.assertEqual(files['/native/later']['slot'],6)
            self.assertEqual(files['/native/later']['data'],b'later')

    def test_bound_host_rejects_unretained_source_before_compilation(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);manifest=root/'manifest.json'
            manifest.write_text('{"crates/prism/src/lib.rs":"'+hashlib.sha256(b'wrong').hexdigest()+'"}')
            with self.assertRaisesRegex(RuntimeError,'retained ninth source'):
                gate.ensure_host(manifest,root/'frozen',root/'out')

if __name__=='__main__':unittest.main()
