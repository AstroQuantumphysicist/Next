#!/usr/bin/env python3
"""Require actual EFI rejection of altered signed deployment media in disposable copies."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import zlib

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_PATH = "::/EFI/NEXT/A/SYSTEM.NXP"
ARTIFACT_DIGEST = "boot artifact SHA-256 differs from signed deployment"


def bounded_boot(command, timeout):
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True, start_new_session=True, cwd=ROOT)
    try:
        transcript, _ = process.communicate(timeout=timeout + 10)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)
        raise TimeoutError("deployment rejection boot exceeded bounded timeout")
    return process.returncode, transcript


def cases(bundle, kernel, initrd):
    yield "pkg-missing", PACKAGE_PATH, None, "open \\EFI\\NEXT\\SYSTEM.NXP:"
    yield "pkg-truncated-header", PACKAGE_PATH, bundle[:255], "signed deployment rejected: Header"
    for name, offset, fmt, value, error in [
        ("pkg-version", 8, "H", 2, "Version"),
        ("pkg-unknown-signer", 64, "B", bundle[64] ^ 1, "UnknownSigner"),
        ("pkg-permissions", 40, "Q", 1 << 63, "Permissions"),
    ]:
        altered = bytearray(bundle)
        struct.pack_into("<" + fmt, altered, offset, value)
        yield name, PACKAGE_PATH, altered, "signed deployment rejected: " + error
    altered = bytearray(bundle)
    altered[-1] ^= 1
    yield "pkg-signature", PACKAGE_PATH, altered, "signed deployment rejected: Signature"
    count = struct.unpack_from("<I", bundle, 32)[0]
    table_end = 256 + count * 128
    if not 0 < count <= 32 or table_end >= len(bundle) - 64:
        raise ValueError("source deployment package has invalid table geometry")
    altered = bytearray(bundle)
    altered[table_end] ^= 1
    yield "pkg-payload-digest", PACKAGE_PATH, altered, "signed deployment rejected: Digest"

    ph_offset = struct.unpack_from("<Q", kernel, 32)[0]
    ph_size, ph_count = struct.unpack_from("<HH", kernel, 54)
    for i in range(ph_count):
        at = ph_offset + i * ph_size
        kind, flags = struct.unpack_from("<II", kernel, at)
        offset = struct.unpack_from("<Q", kernel, at + 8)[0]
        size = struct.unpack_from("<Q", kernel, at + 32)[0]
        if kind == 1 and flags == 4 and size and offset >= ph_offset + ph_size * ph_count:
            altered = bytearray(kernel)
            altered[offset] ^= 1
            yield "pkg-kernel-binding", "::/EFI/NEXT/A/HELIX.ELF", altered, ARTIFACT_DIGEST
            break
    else:
        raise ValueError("source kernel has no suitable file-backed read-only segment")
    altered = bytearray(initrd)
    altered[-1] ^= 1
    altered[24:28] = bytes(4)
    struct.pack_into("<I", altered, 24, zlib.crc32(altered))
    yield "pkg-initrd-binding", "::/EFI/NEXT/A/INITRD.NXI", altered, ARTIFACT_DIGEST
    yield "pkg-oversized", PACKAGE_PATH, bytes(16 * 1024 * 1024 + 1), "signed deployment exceeds bounded 16 MiB profile"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=45.0, help="maximum seconds for each rejected boot")
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be positive and finite")
    image = ROOT / "build/next-dev.img"
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    directory = ROOT / "build/esp/EFI/NEXT"
    source_cases = list(cases((directory / "A/SYSTEM.NXP").read_bytes(),
                              (directory / "A/HELIX.ELF").read_bytes(),
                              (directory / "A/INITRD.NXI").read_bytes()))
    work = ROOT / "work"
    work.mkdir(exist_ok=True)
    results = []
    error = None
    try:
        with tempfile.TemporaryDirectory(prefix="package-failures-", dir=work) as temp_name:
            temp = Path(temp_name)
            for name, destination, payload, diagnostic in source_cases:
                copy = temp / "mutated.img"
                shutil.copyfile(image, copy)
                package_mutation = name.startswith("pkg-") and name not in {
                    "pkg-kernel-binding", "pkg-initrd-binding"
                }
                destinations = (
                    ["::/EFI/NEXT/A/SYSTEM.NXP", "::/EFI/NEXT/B/SYSTEM.NXP"]
                    if package_mutation else [destination]
                )
                for target in destinations:
                    if payload is None:
                        subprocess.run(["mdel", "-i", str(copy), target], check=True, timeout=15)
                    else:
                        changed = temp / "payload.bin"
                        changed.write_bytes(payload)
                        subprocess.run(["mcopy", "-o", "-i", str(copy), str(changed), target], check=True, timeout=15)
                output = ROOT / "build/failures" / name
                expected_stage = {
                    "pkg-kernel-binding": "kernel",
                    "pkg-initrd-binding": "initrd",
                }.get(name, "deployment")
                command = [sys.executable, str(ROOT / "tools/boot_smoke.py"),
                           "--image", str(copy), "--output-dir", str(output),
                           "--timeout", str(args.timeout), "--expect-boot-failure", diagnostic,
                           "--expect-failure-stage", expected_stage]
                status, transcript = bounded_boot(command, args.timeout)
                boot = json.loads((output / "boot-smoke.json").read_text())
                serial = (output / "serial.log").read_bytes()
                observed = diagnostic.encode() in serial and b"HELIX:" not in serial
                failure_record_by_case = {
                    "pkg-kernel-binding": b"NEXT BOOT FAILURE: code=0x4e580201 stage=kernel action=HALT",
                    "pkg-initrd-binding": b"NEXT BOOT FAILURE: code=0x4e580301 stage=initrd action=HALT",
                }
                failure_record = failure_record_by_case.get(
                    name, b"NEXT BOOT FAILURE: code=0x4e580101 stage=deployment action=HALT")
                failure_record_present = failure_record in serial
                passed = status == 0 and boot.get("passed") is True and observed and failure_record_present
                (output / "runner.log").write_text(transcript)
                results.append({"case": name, "passed": passed,
                                "diagnostic": diagnostic, "checks": boot.get("checks", []),
                                "stable_failure_record_present": failure_record_present,
                                "artifact": str(output.relative_to(ROOT) / "serial.log"), "error": boot.get("error")})
                print(f"{name}: {'PASS' if passed else 'FAIL'}", flush=True)
    except (OSError, ValueError, TimeoutError, subprocess.SubprocessError) as exception:
        error = str(exception)
    unchanged = hashlib.sha256(image.read_bytes()).hexdigest() == digest
    report = {"passed": error is None and unchanged and len(results) == len(source_cases)
                        and all(item["passed"] for item in results),
              "source_image_sha256": digest, "source_image_unchanged": unchanged,
              "scope": "QEMU/OVMF rejection of mutated Ed25519/SHA256 development deployments and bound kernel/init-image bytes. Uses no private signing key; EFI loader remains unsigned. No Secure Boot, TPM, production key lifecycle or anti-rollback certification.",
              "cases": results}
    if error:
        report["error"] = error
        print(error, file=sys.stderr)
    (ROOT / "build/package-failures.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
