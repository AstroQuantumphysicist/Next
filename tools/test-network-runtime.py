#!/usr/bin/env python3
"""Run the production DHCP/runtime host acceptance tests with fake device queues."""
import hashlib
import json
from pathlib import Path
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    environment = os.environ.copy()
    # The workspace builds kernel dependencies with panic=abort, while rustc's
    # host test harness uses unwind. Build helix-core with a compatible test
    # profile so its actual allocation-free datagram model can be linked.
    environment["CARGO_PROFILE_DEV_PANIC"] = "unwind"
    subprocess.run(["cargo", "build", "--locked", "--offline", "-p", "helix-core"],
                   cwd=ROOT, env=environment, check=True)
    libraries = list((ROOT / "target/debug/deps").glob("libhelix_core-*.rlib"))
    if not libraries:
        raise RuntimeError("cargo did not produce the helix-core host library")
    library = max(libraries, key=lambda path: path.stat().st_mtime_ns)

    runtime_path = ROOT / "crates/helix-kernel/src/network_runtime.rs"
    runtime_lines = runtime_path.read_text().splitlines()
    # Include the actual source as items within the harness module. Rust's
    # include! macro cannot place inner module-doc comments after an invocation,
    # so normalize only its leading //! lines to ordinary comments.
    for index, line in enumerate(runtime_lines):
        if not line.startswith("//!"):
            break
        runtime_lines[index] = "//" + line[3:]
    runtime_source = "\n".join(runtime_lines) + "\n"
    acceptance_source = (ROOT / "tools/dhcp_runtime_acceptance.rs").read_text()
    generated = runtime_source + "\n" + acceptance_source
    generated_path = ROOT / "work/network_runtime_host_generated.rs"
    generated_path.parent.mkdir(parents=True, exist_ok=True)
    generated_path.write_text(generated)

    output = ROOT / "build/network-runtime-tests"
    output.parent.mkdir(parents=True, exist_ok=True)
    environment["CARGO_MANIFEST_DIR"] = str(ROOT)
    subprocess.run([
        "rustc", "--edition=2021", "--test", "-A", "dead_code", "-D", "warnings",
        str(ROOT / "tools/network_runtime_host.rs"),
        "-L", f"dependency={ROOT / 'target/debug/deps'}",
        "--extern", f"helix_core={library}", "-o", str(output),
    ], cwd=ROOT, env=environment, check=True)
    result = subprocess.run([str(output), "--test-threads=1"], cwd=ROOT,
                            text=True, capture_output=True, check=False)
    print(result.stdout, end="")
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    if result.returncode:
        raise RuntimeError(f"host DHCP/runtime tests failed with exit status {result.returncode}")

    sources = {
        str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in [runtime_path, ROOT / "crates/helix-kernel/src/network_dhcp.rs",
                     ROOT / "tools/network_runtime_host.rs",
                     ROOT / "tools/dhcp_runtime_acceptance.rs"]
    }
    report = {
        "passed": True,
        "mode": "host-modeled with deterministic injected clock and fake bounded device queues",
        "production_code_exercised": [
            "network_dhcp::Client request/accept/lease state machine",
            "network_runtime::Runtime::run and pump",
            "network_runtime::accept_dhcp_reply and restart_dhcp",
        ],
        "qemu_guest_verification": "not performed by this host harness",
        "source_sha256": sources,
        "test_output": result.stdout,
    }
    report_path = ROOT / "build/network-runtime-host.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({
        "passed": report["passed"],
        "mode": report["mode"],
        "report": str(report_path.relative_to(ROOT)),
        "qemu_guest_verification": report["qemu_guest_verification"],
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
