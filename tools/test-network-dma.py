#!/usr/bin/env python3
"""Run host tests over the production virtio-network queue ownership code."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    environment = os.environ.copy()
    environment["CARGO_PROFILE_DEV_PANIC"] = "unwind"
    subprocess.run(
        ["cargo", "build", "--locked", "--offline", "-p", "helix-core"],
        cwd=ROOT,
        env=environment,
        check=True,
    )
    libraries = list((ROOT / "target/debug/deps").glob("libhelix_core-*.rlib"))
    if not libraries:
        raise RuntimeError("cargo did not produce the helix-core host library")
    library = max(libraries, key=lambda path: path.stat().st_mtime_ns)

    output = ROOT / "build/network-dma-tests"
    output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "rustc",
            "--edition=2021",
            "--test",
            "-A",
            "dead_code",
            "-D",
            "warnings",
            str(ROOT / "tools/network_dma_host.rs"),
            "-L",
            f"dependency={ROOT / 'target/debug/deps'}",
            "--extern",
            f"helix_core={library}",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        env=environment,
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
        raise RuntimeError(f"virtio-network DMA ownership tests failed: {result.returncode}")

    production = [
        ROOT / "crates/helix-kernel/src/network.rs",
        ROOT / "crates/helix-kernel/src/network_dhcpv6.rs",
        ROOT / "crates/helix-kernel/src/network_ipv6.rs",
        ROOT / "crates/helix-kernel/src/network_runtime.rs",
        ROOT / "crates/helix-kernel/src/network_dhcp.rs",
        ROOT / "crates/helix-kernel/src/network_tcp.rs",
        ROOT / "crates/helix-kernel/src/network_udp.rs",
        ROOT / "tools/network_dma_host.rs",
        ROOT / "tools/test-network-dma.py",
    ]
    report = {
        "passed": True,
        "mode": "host tests compile and exercise production network queue code; simulated used-ring completions",
        "production_code_exercised": [
            "Queue::publish descriptor publication and static-buffer ownership lease",
            "Queue::poll exact descriptor validation and used-ring ownership return",
            "Device::submit_transmit rejection before modifying device-owned static TX storage",
            "network runtime timeout retaining outstanding TX ownership",
            "network_ipv6::validate_ethernet_frame IPv6 and Neighbor Discovery ingress validation",
            "network_dhcpv6::parse_message and Transaction option/lease-candidate validation plus the bounded Client transaction/timer state machine (host-tested only; no guest packet, address, DAD, or route integration)",
        ],
        "qemu_guest_verification": "not performed by this host harness",
        "source_sha256": {
            str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in production
        },
        "test_output": result.stdout,
    }
    report_path = ROOT / "build/network-dma-host.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                "passed": report["passed"],
                "mode": report["mode"],
                "report": str(report_path.relative_to(ROOT)),
                "qemu_guest_verification": report["qemu_guest_verification"],
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
