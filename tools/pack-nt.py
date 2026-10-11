#!/usr/bin/env python3
"""Original standard PE32+ packer for fixed Win64 import regression fixtures.

Only our GNU-assembled code is packaged. No Windows toolchain, DLL, header,
resource, or other Microsoft artifact is downloaded or redistributed.
"""
import struct
from pathlib import Path

BASE = 0x4000000000
ALIGN = 512

def align(size):
    return (size + ALIGN - 1) & -ALIGN

def build(directory, output_name="hello"):
    text = (directory / 'text.bin').read_bytes()
    rodata = (directory / 'rdata.bin').read_bytes()
    data = (directory / 'data.bin').read_bytes()
    imports = bytearray(512)
    struct.pack_into('<IIIII', imports, 0, 0x3080, 0, 0, 0x30c0, 0x3040)
    names = [('GetStdHandle', 0xe0), ('WriteFile', 0xf0), ('ExitProcess', 0x100)]
    for index, (name, offset) in enumerate(names):
        struct.pack_into('<Q', imports, 0x40 + index * 8, 0x3000 + offset)
        struct.pack_into('<Q', imports, 0x80 + index * 8, 0x3000 + offset)
        value = b'\0\0' + name.encode('ascii') + b'\0'
        imports[offset:offset + len(value)] = value
    imports[0xc0:0xc0 + 13] = b'kernel32.dll\0'
    sections = [(b'.text', 0x1000, text, 0x60000020),
                (b'.rdata', 0x2000, rodata, 0x40000040),
                (b'.idata', 0x3000, bytes(imports), 0xc0000040),
                (b'.data', 0x4000, data, 0xc0000040)]
    if any(not payload or len(payload) > 4096 for _, _, payload, _ in sections):
        raise ValueError('Fixture sections must be nonempty and at most one page')
    output = bytearray(1024)
    output[:2] = b'MZ'
    struct.pack_into('<I', output, 0x3c, 0x80)
    output[0x80:0x84] = b'PE\0\0'
    struct.pack_into('<HHIIIHH', output, 0x84, 0x8664, 4, 0, 0, 0, 240, 0x23)
    optional = 0x98
    struct.pack_into('<HBBIIIIIQII', output, optional,
                     0x20b, 1, 0, align(len(text)),
                     sum(align(len(p)) for _, _, p, _ in sections[1:]),
                     0, 0x1000, 0x1000, BASE, 4096, 512)
    struct.pack_into('<HHHHHHIIIIHHQQQQII', output, optional + 40,
                     6, 0, 0, 0, 6, 0, 0, 0x5000, 1024, 0,
                     3, 0x100, 65536, 4096, 1048576, 4096, 0, 16)
    struct.pack_into('<II', output, optional + 112 + 1 * 8, 0x3000, 40)
    struct.pack_into('<II', output, optional + 112 + 12 * 8, 0x3040, 32)
    table = optional + 240
    for index, (name, address, payload, flags) in enumerate(sections):
        raw_offset = len(output)
        raw_size = align(len(payload))
        struct.pack_into('<8sIIIIIIHHI', output, table + index * 40,
                         name, len(payload), address, raw_size, raw_offset,
                         0, 0, 0, 0, flags)
        output.extend(payload)
        output.extend(bytes(raw_size - len(payload)))
    (directory / (output_name + '.exe')).write_bytes(output)
    print(f'Original fixed PE32+ fixture built: {len(output)} bytes, 3 kernel32 imports')

if __name__ == '__main__':
    import sys
    name = sys.argv[1] if len(sys.argv) > 1 else 'hello'
    if name not in ('hello', 'hello-gate'):
        raise SystemExit('Unknown original fixture name')
    build(Path(__file__).resolve().parents[1] / 'build/compat/nt', name)
