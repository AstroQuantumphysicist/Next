#!/usr/bin/env python3
"""Boot mutated copies of generated media; require fail-closed loader behavior."""
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def cases(original):
    yield "missing-kernel", None, "open \\EFI\\NEXT\\HELIX.ELF:"
    yield "truncated-header", original[:63], "Helix ELF rejected: Truncated"
    changes = [
        ("wrong-architecture", 18, "H", 183, "Architecture"),
        ("writable-executable", 68, "I", 7, "WritableExecutable"),
        ("overlapping-segments", 64 + 56 + 16, "Q", 0x04000000, "Overlap"),
        ("file-offset-overflow", 72, "Q", 2**64 - 1, "Overflow"),
        ("unsupported-dynamic-kernel", 16, "H", 3, "Type"),
        ("invalid-entry", 24, "Q", 0, "Entry"),
        ("wrong-physical-address", 88, "Q", 0x05000000, "Address"),
    ]
    for name, offset, fmt, value, diagnostic in changes:
        mutated = bytearray(original)
        struct.pack_into("<" + fmt, mutated, offset, value)
        yield name, mutated, "Helix ELF rejected: " + diagnostic
    yield "oversized-kernel", bytes(8 * 1024 * 1024 + 1), "Helix ELF exceeds bounded 8 MiB"


def main():
    image = ROOT / "build/next-dev.img"
    original = (ROOT / "build/esp/EFI/NEXT/HELIX.ELF").read_bytes()
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    work = ROOT / "work"
    work.mkdir(exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix="boot-failures-", dir=work) as temp_name:
        temp = Path(temp_name)
        for name, kernel, diagnostic in cases(original):
            copy = temp / "mutated.img"
            shutil.copyfile(image, copy)
            if kernel is None:
                subprocess.run(["mdel", "-i", str(copy), "::/EFI/NEXT/HELIX.ELF"], check=True)
            else:
                payload = temp / "HELIX.ELF"
                payload.write_bytes(kernel)
                subprocess.run(["mcopy", "-o", "-i", str(copy), str(payload), "::/EFI/NEXT/HELIX.ELF"], check=True)
            output = ROOT / "build/failures" / name
            command = [sys.executable, str(ROOT / "tools/boot_smoke.py"), "--image", str(copy),
                       "--output-dir", str(output), "--expect-boot-failure", diagnostic]
            result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            report = json.loads((output / "boot-smoke.json").read_text())
            results.append({"case": name, "passed": result.returncode == 0 and report["passed"],
                            "checks": report["checks"], "artifact": str(output.relative_to(ROOT) / "serial.log"),
                            "error": report.get("error")})
            print(f"{name}: {'PASS' if results[-1]['passed'] else 'FAIL'}", flush=True)
    unchanged = hashlib.sha256(image.read_bytes()).hexdigest() == digest
    report = {"passed": unchanged and all(r["passed"] for r in results), "source_image_sha256": digest,
              "source_image_unchanged": unchanged, "scope": "QEMU/OVMF rejection of mutated unsigned development ELF media; no signature validation or real hardware certification",
              "cases": results}
    (ROOT / "build/boot-failures.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
