#!/usr/bin/env python3
"""Independent on-media pair decoder tests; does not constitute a guest test."""
import unittest
import zlib
from trash_smoke import trash_records


def record(payload=b'held-modified', path=b'/native/trash-proof'):
    raw = bytearray(128)
    raw[:12] = b'NXTRASH1\1\0\x80\0'
    raw[16:24] = (1).to_bytes(8,'little')
    raw[24:32] = (17).to_bytes(8,'little')
    raw[32:36] = len(payload).to_bytes(4,'little')
    raw[36:40] = zlib.crc32(payload).to_bytes(4,'little')
    raw[40] = len(path); raw[41:41+len(path)] = path
    raw[124:] = zlib.crc32(raw[:124]).to_bytes(4,'little')
    return raw


def files(raw=None, payload=b'held-modified'):
    return {'/trash/d.0000000000000011':{'data':payload},
            '/trash/m.0000000000000011':{'data':bytes(record(payload) if raw is None else raw)}}


class Decoder(unittest.TestCase):
    def test_pair_empty_and_binary(self):
        for payload in (b'', b'\0\xff', b'held-modified'):
            self.assertEqual(trash_records(files(payload=payload)),
                             [{'id':17,'original':'/native/trash-proof','data':payload}])
        self.assertEqual(trash_records({}), [])
    def test_all_bit_flips_fail_closed(self):
        for bit in range(128*8):
            raw = record(); raw[bit//8] ^= 1 << (bit%8)
            with self.assertRaises(RuntimeError): trash_records(files(raw))
    def test_recomputed_crc_does_not_grant_owner_or_path(self):
        for at, value in ((16,2),(24,18),(40,32),(72,1),(12,1),(10,127)):
            raw = record(); raw[at] = value
            raw[124:] = zlib.crc32(raw[:124]).to_bytes(4,'little')
            with self.assertRaises(RuntimeError): trash_records(files(raw))
        for path in (b'/native/../x',b'/native//x',b'/native/x/',b'/other/x'):
            with self.assertRaises(RuntimeError): trash_records(files(record(path=path)))
    def test_orphans_aliases_and_data_mismatch(self):
        for path in ('/trash/d.0000000000000011','/trash/m.0000000000000011'):
            f=files(); del f[path]
            with self.assertRaises(RuntimeError): trash_records(f)
        f=files();f['/trash/d.0000000000000011']['data']=b'tampered'
        with self.assertRaises(RuntimeError):trash_records(f)
        f=files();f['/trash/m.0000000000000011/extra']=f.pop('/trash/m.0000000000000011')
        with self.assertRaises(RuntimeError):trash_records(f)

if __name__=='__main__':unittest.main()
