#!/usr/bin/env python3
"""Verify durable guest writes across two independent QEMU boots of a scratch image."""
import argparse
import datetime
import hashlib
import json
import math
from pathlib import Path
import os
import shutil
import signal
import subprocess
import sys
import tempfile
from cow_verify import verify as verify_cow
from cow_recover import export as export_cow, verify_export

ROOT = Path(__file__).resolve().parents[1]
ESP_BYTES = 64 * 1024 * 1024
STORE_BYTES = 132 * 512


def sha256(path, start=0, length=None):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        stream.seek(start)
        remaining = length
        while remaining is None or remaining > 0:
            chunk = stream.read(1024 * 1024 if remaining is None else min(1024 * 1024, remaining))
            if not chunk:
                if remaining is not None and remaining != 0:
                    raise ValueError("image truncated while hashing protected extent")
                break
            digest.update(chunk)
            if remaining is not None:
                remaining -= len(chunk)
    return digest.hexdigest()


def run_phase(image, output, phase, timeout):
    command = [sys.executable, str(ROOT / "tools/boot_smoke.py"),
               "--image", str(image), "--output-dir", str(output),
               "--timeout", str(timeout), "--persistent-data", "--persistence-phase", phase]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True, start_new_session=True, cwd=ROOT)
    try:
        transcript, _ = process.communicate(timeout=timeout + 10)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            transcript, _ = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            transcript, _ = process.communicate(timeout=5)
        output.mkdir(parents=True, exist_ok=True)
        (output / "runner.log").write_text(transcript)
        raise TimeoutError(f"{phase} boot did not finish within bounded timeout")
    output.mkdir(parents=True, exist_ok=True)
    (output / "runner.log").write_text(transcript)
    report = json.loads((output / "boot-smoke.json").read_text())
    serial = (output / "serial.log").read_bytes()
    markers = [b"NEXT DATA: Wrote 15 bytes to /persistent"] if phase == "create" else [
        b"NEXT DATA: survives-reboot", b"NEXT DATA: Removed /persistent"]
    confirmed = "persistence phase: " + phase in report.get("checks", []) and all(
        marker in serial for marker in markers)
    return {"phase": phase, "passed": process.returncode == 0 and report.get("passed") is True and confirmed,
            "guest_phase_confirmed": confirmed,
            "exit_status": process.returncode, "checks": report.get("checks", []),
            "artifact": str(output.relative_to(ROOT) / "serial.log"), "error": report.get("error")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=ROOT / "build/next-dev.img")
    parser.add_argument("--timeout", type=float, default=45.0, help="maximum seconds for each boot")
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be positive and finite")
    image = args.image.resolve()
    output = ROOT / "build/persistence"
    output.mkdir(parents=True, exist_ok=True)
    work = ROOT / "work"
    work.mkdir(exist_ok=True)
    report = {"passed": False,
              "scope": "Two independent QEMU/OVMF guest boots using real virtio writes and flushes on one scratch image; no snapshot overlay; protected FAT32/ESP unchanged. This is clean VM reboot persistence, not power-loss or real hardware certification.",
              "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "source_image": str(image), "phases": [], "checks": []}
    digest = None
    try:
        if image.stat().st_size < ESP_BYTES + STORE_BYTES:
            raise ValueError("image lacks the dedicated trailing 132-sector COW data extent")
        digest = sha256(image)
        report["source_image_sha256"] = digest
        protected_digest = sha256(image, length=ESP_BYTES)
        with tempfile.TemporaryDirectory(prefix="persistence-", dir=work) as temp_name:
            copy = Path(temp_name) / "persistent.img"
            shutil.copyfile(image, copy)
            initial_data = sha256(copy, start=ESP_BYTES)
            for phase in ["create", "verify"]:
                result = run_phase(copy, output / phase, phase, args.timeout)
                result["protected_esp_unchanged"] = sha256(copy, length=ESP_BYTES) == protected_digest
                result["data_sha256"] = sha256(copy, start=ESP_BYTES)
                result["passed"] = result["passed"] and result["protected_esp_unchanged"]
                report["phases"].append(result)
                print(f"persistence {phase}: {'PASS' if result['passed'] else 'FAIL'}", flush=True)
                if not result["passed"]:
                    raise RuntimeError(f"{phase} boot failed: {result.get('error') or 'see phase artifacts'}")
            journal = verify_cow(copy, ESP_BYTES // 512, STORE_BYTES // 512, [(0, ESP_BYTES // 512)])
            if journal.get("mount_read_only"):
                raise RuntimeError("clean persistence journal unexpectedly requires read-only fallback")
            report["journal_verification"] = journal
            (output / "cow-verification.json").write_text(json.dumps(journal, indent=2) + "\n")
            # VMs have stopped. Export only the verifier's committed snapshot,
            # never repair or write the still-live temporary scratch image.
            recovery_before = sha256(copy)
            recovery_parent = Path(tempfile.mkdtemp(prefix="recovery-", dir=output))
            recovery_destination = recovery_parent / "snapshot"
            exported = export_cow(copy, recovery_destination, ESP_BYTES // 512,
                                  STORE_BYTES // 512, [(0, ESP_BYTES // 512)])
            verified = verify_export(recovery_destination)
            recovery_after = sha256(copy)
            expected_files = [{"path": item["path"], "size": item["size"], "crc32": item["crc32"]}
                              for item in journal["files"]]
            recovered_files = [{"path": item["path"], "size": item["size"], "crc32": item["crc32"]}
                               for item in verified["files"]]
            recovery_checks = {
                "scratch_image_unchanged": recovery_before == recovery_after,
                "source_hash_matches_journal": verified["source_sha256"] == recovery_before == journal["source_sha256"],
                "generation_matches_journal": verified["generation"] == journal["selected_generation"],
                "bank_matches_journal": verified["bank"] == journal["selected_bank"],
                "files_match_journal": recovered_files == expected_files,
                "verified_manifest_matches_export": verified == exported,
            }
            recovery_result = {"passed": all(recovery_checks.values()), "checks": recovery_checks,
                               "scratch_sha256_before": recovery_before,
                               "scratch_sha256_after": recovery_after,
                               "manifest_sha256": sha256(recovery_destination / "manifest.json"),
                               "manifest": str((recovery_destination / "manifest.json").relative_to(ROOT)),
                               "scope": "Read-only export of bounded verified committed guest files; no repair, full fsck, encryption or backup authentication."}
            report["recovery_verification"] = recovery_result
            (output / "recovery-verification.json").write_text(json.dumps(recovery_result, indent=2) + "\n")
            if not recovery_result["passed"]:
                raise RuntimeError("guest recovery export disagrees with committed journal or changed media")
            if report["phases"][0]["data_sha256"] == initial_data:
                raise RuntimeError("create phase made no durable change to the trailing data extent")
            if report["phases"][1]["data_sha256"] == report["phases"][0]["data_sha256"]:
                raise RuntimeError("verify phase did not durably remove the persistence test file")
            report["checks"] = ["guest writes /persistent with survives-reboot and flushes",
                                "second fresh VM boot reads survives-reboot from the same disk copy",
                                "second boot removes /persistent with durable commit",
                                "both guest phases preserve every byte of the protected 64 MiB FAT32/ESP",
                                "original generated source image is never passed writable to QEMU",
                                "read-only verifier validates actual guest-written committed journal without changing media",
                                "read-only recovery exports and verifies the same generation/files while preserving every scratch-image byte"]
            report["passed"] = True
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    finally:
        try:
            report["source_image_unchanged"] = digest is not None and sha256(image) == digest
        except OSError:
            report["source_image_unchanged"] = False
        report["passed"] = report["passed"] and report["source_image_unchanged"]
        report["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (ROOT / "build/persistence.json").write_text(json.dumps(report, indent=2) + "\n")
    if "error" in report:
        print(report["error"], file=sys.stderr)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
