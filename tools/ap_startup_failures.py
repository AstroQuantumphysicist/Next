#!/usr/bin/env python3
"""Mutate actual retained AP ELF bytes and require the host audit to reject them.
No image build, signing, mapping or AP startup is performed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys

import ap_startup_audit as checker


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    elf = args.elf.resolve()
    out = args.output_dir.resolve()
    if not out.is_relative_to((checker.ROOT / "work").resolve()) or out.exists():
        parser.error("output must be a new directory under work/")
    if not elf.is_file() or elf.stat().st_size > checker.MAX_ELF:
        parser.error("ELF must be an existing bounded file")
    original = elf.read_bytes()
    blob, _, info = checker.audit(elf)
    start = info["symbols"]["next_ap_blob_start"]
    offset = None
    flags_offset = None
    phoff = struct.unpack_from("<Q", original, 32)[0]
    size, count = struct.unpack_from("<HH", original, 54)
    for i in range(count):
        at = phoff + i * size
        kind, flags, file, virtual, physical, files, memory, align = struct.unpack_from("<IIQQQQQQ", original, at)
        if kind == 1 and virtual <= start < virtual + files:
            offset, flags_offset = file + start - virtual, at + 4
    assert offset is not None
    pm = info["symbols"]["next_ap_pm32"] - start
    far = blob[:pm].index(b"\x66\xea")
    gdt = info["symbols"]["next_ap_gdt"] - start
    real_bridge = blob.index(bytes.fromhex("0f 01 1e 58 0d"))
    protected_bridge = blob.index(bytes.fromhex("0f 01 1d 58 2d 08 00"))
    ready_cas = blob.index(b"\xf0\x41\x0f\xb1\x14\x24")
    cases = [
        ("real-bridge-wrong-metadata-offset", offset + real_bridge + 3, 0x59),
        ("protected-bridge-wrong-metadata-offset", offset + protected_bridge + 3, 0x59),
        ("missing-16bit-far-offset-prefix", offset + far, 0x90),
        ("altered-private-gdt", offset + gdt + 24 + 6, 0),
        ("gdt-code-accessed-clear", offset + gdt + 8 + 5, 0x9a),
        ("gdt-data-accessed-clear", offset + gdt + 16 + 5, 0x92),
        ("gdt-long-accessed-clear", offset + gdt + 24 + 5, 0x9a),
        ("writable-source-load", flags_offset, 6),
        ("entry-enables-interrupts", offset, 0xFB),
        ("ready-cas-not-locked", offset + ready_cas, 0x90),
    ]
    out.mkdir(parents=True)
    tool = checker.ROOT / "tools/ap_startup_audit.py"
    records = []
    baseline = subprocess.run([sys.executable, str(tool), "--elf", str(elf), "--output-dir", str(out / "baseline")], capture_output=True, text=True)
    if baseline.returncode:
        raise RuntimeError("baseline compiled image failed audit: " + baseline.stderr[-1000:])
    for name, at, byte in cases:
        changed = bytearray(original)
        if changed[at] == byte:
            raise RuntimeError("mutation did not change a byte")
        changed[at] = byte
        fixture = out / f"{name}.elf"
        fixture.write_bytes(changed)
        result = subprocess.run([sys.executable, str(tool), "--elf", str(fixture), "--output-dir", str(out / name)], capture_output=True, text=True)
        if result.returncode == 0:
            raise RuntimeError("hostile compiled image unexpectedly accepted: " + name)
        records.append({"case": name, "offset": at, "changed_sha256": hashlib.sha256(changed).hexdigest(), "rejected": True, "reason": result.stderr.strip().splitlines()[-1]})
    if elf.read_bytes() != original:
        raise RuntimeError("input ELF changed during read-only audit")
    manifest = {"scope": "host compiled-byte mutation rejection; no guest AP execution", "input_sha256": hashlib.sha256(original).hexdigest(), "cases": records}
    (out / "failures.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"result": "PASS", "rejections": len(records), "manifest": str(out / "failures.json")}))


if __name__ == "__main__":
    main()
