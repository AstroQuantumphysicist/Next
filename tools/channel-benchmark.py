#!/usr/bin/env python3
"""Build and run a reproducible host-only benchmark of helix-core channels."""
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tools/channel_benchmark.rs"
CHANNEL_SOURCE = ROOT / "crates/helix-core/src/channel.rs"
IPC_SOURCE = ROOT / "crates/helix-core/src/ipc.rs"
SAMPLES = 5
CASES = [
    ("payload_1B", 1, 20_000, 0),
    ("payload_64B", 64, 20_000, 0),
    ("payload_4KiB_fragmented", 4 * 1024, 1_024, 0),
    ("payload_64KiB_fragmented", 64 * 1024, 128, 0),
    ("payload_64B_plus_1_attachment", 64, 20_000, 1),
    ("payload_64B_plus_4_attachments", 64, 20_000, 4),
]


def build_and_run():
    environment = os.environ.copy()
    # Use an optimized host rlib and an unwind profile for the standalone host
    # executable; these choices do not alter the kernel build configuration.
    environment["CARGO_PROFILE_RELEASE_PANIC"] = "unwind"
    environment["CARGO_PROFILE_RELEASE_LTO"] = "false"
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "--offline", "-p", "helix-core"],
        cwd=ROOT,
        env=environment,
        check=True,
    )
    libraries = list((ROOT / "target/release/deps").glob("libhelix_core-*.rlib"))
    if not libraries:
        raise RuntimeError("cargo did not produce the helix-core host library")
    library = max(libraries, key=lambda path: path.stat().st_mtime_ns)

    output_dir = ROOT / "build"
    output_dir.mkdir(parents=True, exist_ok=True)
    executable = output_dir / "channel-host-benchmark"
    rustc_version = subprocess.run(
        ["rustc", "--version"],
        cwd=ROOT,
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    environment["NEXT_CHANNEL_RUSTC_VERSION"] = rustc_version
    subprocess.run(
        [
            "rustc",
            "--edition=2021",
            "-C",
            "opt-level=2",
            "-D",
            "warnings",
            str(SOURCE),
            "-L",
            f"dependency={ROOT / 'target/release/deps'}",
            "--extern",
            f"helix_core={library}",
            "-o",
            str(executable),
        ],
        cwd=ROOT,
        env=environment,
        check=True,
    )
    result = subprocess.run(
        [str(executable)], cwd=ROOT, env=environment, text=True, capture_output=True
    )
    if result.returncode:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise RuntimeError(f"channel host benchmark failed ({result.returncode})")
    return result.stdout, rustc_version


def main():
    output, rustc_version = build_and_run()
    samples = {name: [] for name, _, _, _ in CASES}
    observed = {}
    validation = False
    for line in output.splitlines():
        fields = line.split("\t")
        if fields[0] == "VALIDATION":
            validation = fields[1] == "passed"
            continue
        if fields[0] != "SAMPLE" or len(fields) != 8:
            raise RuntimeError(f"malformed benchmark output: {line!r}")
        _, name, payload_bytes, iterations, handles, fragments, sample, elapsed_ns = fields
        sample_info = tuple(map(int, (payload_bytes, iterations, handles, fragments)))
        observed.setdefault(name, sample_info)
        if observed[name] != sample_info:
            raise RuntimeError(f"workload metadata changed between samples: {name}")
        samples[name].append((int(sample), int(elapsed_ns)))

    if not validation:
        raise RuntimeError("host benchmark's payload/attachment validation failed")
    if set(samples) != set(observed):
        raise RuntimeError(f"missing or unexpected workloads: {set(samples) ^ set(observed)}")

    reports = []
    for name, payload_bytes, iterations, handles in CASES:
        actual_payload, actual_iterations, actual_handles, fragments = observed[name]
        if (actual_payload, actual_iterations, actual_handles) != (
            payload_bytes,
            iterations,
            handles,
        ):
            raise RuntimeError(f"unexpected workload shape for {name}")
        timing = samples[name]
        if len(timing) != SAMPLES or sorted(index for index, _ in timing) != list(range(SAMPLES)):
            raise RuntimeError(f"incomplete timing samples for {name}")
        elapsed_values = [elapsed for _, elapsed in timing]
        if any(elapsed <= 0 for elapsed in elapsed_values):
            raise RuntimeError(f"invalid elapsed time for {name}")
        frames = iterations * fragments
        logical_bytes = iterations * payload_bytes
        wire_bytes = logical_bytes + frames * (40 + handles * 8)
        median_ns = int(statistics.median(elapsed_values))
        seconds = median_ns / 1_000_000_000
        reports.append(
            {
                "name": name,
                "logical_payload_bytes": payload_bytes,
                "iterations": iterations,
                "fragments_per_payload": fragments,
                "frames": frames,
                "attachments_per_frame": handles,
                "elapsed_ns_samples": elapsed_values,
                "median_elapsed_ns": median_ns,
                "min_elapsed_ns": min(elapsed_values),
                "max_elapsed_ns": max(elapsed_values),
                "median_ns_per_frame": median_ns / frames,
                "logical_payload_mib_per_second": logical_bytes / seconds / (1024 * 1024),
                "framed_bytes_per_second": wire_bytes / seconds,
                "encoded_framed_bytes_per_frame_average": wire_bytes / frames,
            }
        )

    baseline_ns = next(
        workload["median_ns_per_frame"]
        for workload in reports
        if workload["name"] == "payload_64B"
    )
    for workload in reports:
        if workload["attachments_per_frame"]:
            overhead_ns = workload["median_ns_per_frame"] - baseline_ns
            workload["median_ns_per_frame_over_64B_unattached"] = overhead_ns
            workload["percent_over_64B_unattached"] = overhead_ns / baseline_ns * 100

    source_hashes = {
        str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in (CHANNEL_SOURCE, IPC_SOURCE, SOURCE)
    }
    report = {
        "schema": "next.helix-core.channel-host-benchmark.v1",
        "passed": True,
        "mode": "host model only: production helix-core framing and bounded queue APIs",
        "scope": "No kernel syscall, process-table authorization/retain-release, device, or guest latency is measured.",
        "host": {
            "system": platform.system(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "rustc": rustc_version,
            "optimization": "release opt-level=2; LTO disabled for standalone host linking",
        },
        "production_limits": {
            "queue_depth_frames": 4,
            "max_payload_bytes_per_frame": 256,
            "max_attached_references_per_frame": 4,
            "wire_header_bytes": 40,
        },
        "workload_notes": [
            "1 B and 64 B workloads use one frame per logical payload.",
            "4 KiB and 64 KiB are logical payload streams fragmented into 256 B maximum frames; a single channel frame cannot carry either size.",
            "Attachment cases measure copying and queueing generic owned-reference tokens through send_attached/peek/take. They do not measure kernel handle-table rights checks or object retain/release.",
            "Fixed deterministic payload bytes, iteration counts, five samples, and source hashes make runs comparable; timings remain host- and load-dependent.",
        ],
        "validation": {
            "passed": True,
            "payload_fragment_order_and_bytes_checked": True,
            "attachment_identity_and_count_checked": True,
            "samples_per_workload": SAMPLES,
        },
        "source_sha256": source_hashes,
        "workloads": reports,
        "raw_output": output,
    }
    report_path = ROOT / "build/channel-host-benchmark.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                "passed": report["passed"],
                "mode": report["mode"],
                "report": str(report_path.relative_to(ROOT)),
                "workloads": [
                    {
                        "name": workload["name"],
                        "median_ns_per_frame": round(workload["median_ns_per_frame"], 1),
                        "logical_payload_mib_per_second": round(
                            workload["logical_payload_mib_per_second"], 2
                        ),
                    }
                    for workload in reports
                ],
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
