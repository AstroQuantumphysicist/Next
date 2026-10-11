import hashlib
from pathlib import Path
import struct
import tempfile
import unittest
import uuid
import zlib
import gpt_mount_smoke as gm
import gpt_plan as gp
from initrd import pack

class GptMountHarness(unittest.TestCase):
    def test_archive_extraction_preserves_exact_payloads_and_rejects_corruption(self):
        members=[('/system/nextd.elf',b'owner application'),('/compat/nt/example.exe',b'other app')]
        encoded=pack(members)
        self.assertEqual(dict(gm.archive_members(encoded)),dict(members))
        for corrupted in [encoded[:31],encoded[:100],encoded+b'\0']:
            with self.assertRaises(ValueError): gm.archive_members(corrupted)
        corrupted=bytearray(encoded); corrupted[-1]^=1
        with self.assertRaises(ValueError): gm.archive_members(corrupted)

    def test_exact_policy_bytes_use_guid_disk_order_and_full_partition_geometry(self):
        disk=uuid.UUID('01020304-0506-0708-090a-0b0c0d0e0f10')
        data=next(p for p in gp.plan(gm.CAPACITY,esp=64<<20,system=65536,
                  recovery=65536,minimum_data=65536) if p.type_guid==gp.TYPES['data'])
        encoded=gm.policy_bytes(disk,data)
        fields=struct.unpack('<8sHHI16s16sQQQII',encoded)
        self.assertEqual(fields[:4],(b'NXTSTOR1',1,80,1))
        self.assertEqual(fields[4:6],(disk.bytes_le,data.unique_guid.bytes_le))
        self.assertEqual(fields[6:],(data.first,data.last-data.first+1,gm.CAPACITY//512,512,0))
        self.assertEqual(struct.unpack_from('<I',gm.policy_bytes(disk,data,False),12)[0],0)

    def test_decoding_verified_package_still_checks_member_digests(self):
        payload=b'verified payload'
        raw=bytearray(256+128+len(payload)+64)
        struct.pack_into('<II',raw,32,1,0)
        struct.pack_into('<Q',raw,40,0x3f)
        struct.pack_into('<III',raw,48,0,1,0)
        raw[96:102]=b'org.os'
        raw[256:270]=b'bin/nextd.elf\0'
        struct.pack_into('<QQ',raw,320,384,len(payload))
        raw[336:368]=hashlib.sha256(payload).digest()
        raw[384:384+len(payload)]=payload
        members,identity,version,permissions,entry=gm.package_members(raw)
        self.assertEqual(members,[('bin/nextd.elf',payload)])
        self.assertEqual((identity,version,permissions,entry),('org.os','0.1.0',0x3f,'bin/nextd.elf'))
        raw[384]^=1
        with self.assertRaises(ValueError): gm.package_members(raw)
        with self.assertRaises(ValueError): gm.package_members(raw[:300])

    def test_backup_disagreement_remains_individually_crc_valid(self):
        disk = uuid.uuid4()
        parts = gp.plan(gm.CAPACITY,esp=64<<20,system=65536,
                        recovery=65536,minimum_data=65536)
        original = gp.encode(gm.CAPACITY,512,disk,parts)[gm.CAPACITY//512-1]
        changed = gm.conflicting_backup_header(original)
        self.assertNotEqual(changed[56:72],original[56:72])
        self.assertEqual(changed[:16],original[:16])
        self.assertEqual(changed[20:56],original[20:56])
        self.assertEqual(changed[72:],original[72:])
        crc = struct.unpack_from('<I',changed,16)[0]
        zeroed = bytearray(changed); zeroed[16:20] = bytes(4)
        self.assertEqual(crc,zlib.crc32(zeroed[:92]))
        invalid = bytearray(original); invalid[16] ^= 1
        with self.assertRaises(ValueError): gm.conflicting_backup_header(invalid)
        with self.assertRaises(ValueError): gm.conflicting_backup_header(original[:91])

    def test_protection_covers_both_sides_of_exact_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            image=Path(directory)/'owned.img'; image.write_bytes(bytes(256*512))
            before=gm.protected_hash(image,64)
            with image.open('r+b') as stream:
                stream.seek(64*512+100); stream.write(b'journal mutation')
            self.assertEqual(gm.protected_hash(image,64),before)
            with image.open('r+b') as stream:
                stream.seek((64+132)*512); stream.write(b'protected mutation')
            self.assertNotEqual(gm.protected_hash(image,64),before)

if __name__ == '__main__': unittest.main()
