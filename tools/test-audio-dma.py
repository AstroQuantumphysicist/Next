#!/usr/bin/env python3
"""Compile and run the production AC97 DMA lifetime tests on the host."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    output = ROOT / "build/ac97-dma-tests"
    output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "rustc",
            "--edition=2021",
            "--test",
            "-D",
            "warnings",
            str(ROOT / "crates/helix-kernel/src/audio.rs"),
            "-o",
            str(output),
        ],
        cwd=ROOT,
        check=True,
    )
    result = subprocess.run(
        [str(output), "--test-threads=1"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    print(result.stdout, end="")
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    if result.returncode:
        raise RuntimeError(f"AC97 DMA lifetime tests failed: {result.returncode}")

    sources = [
        ROOT / "crates/helix-kernel/src/audio.rs",
        ROOT / "tools/test-audio-dma.py",
    ]
    report = {
        "passed": True,
        "mode": "host tests compile the production AC97 module; no port I/O or DMA device is invoked",
        "production_code_exercised": [
            "DmaOwnership::publish before BDL address and RUN register publication",
            "DmaOwnership::observe retaining ownership on no completion, FIFO error, and malformed completion",
            "DmaOwnership::observe returning ownership after DCH + LVBCI + zero remaining samples",
            "DmaOwnership::reset_confirmed as the safe recovery after an engine reset",
        ],
        "hardware_verification": "not performed by this host harness",
        "source_sha256": {
            str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sources
        },
        "test_output": result.stdout,
    }
    report_path = ROOT / "build/ac97-dma-host.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                "passed": report["passed"],
                "mode": report["mode"],
                "report": str(report_path.relative_to(ROOT)),
                "hardware_verification": report["hardware_verification"],
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
