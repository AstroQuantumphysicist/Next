#!/usr/bin/env python3
"""Compile the real Linux file adapter against bounded host device stubs."""
from pathlib import Path
import os
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def library(crate):
    candidates = list((ROOT / "target/debug/deps").glob(f"lib{crate}-*.rlib"))
    if not candidates:
        raise RuntimeError(f"cargo did not produce the {crate} host library")
    return max(candidates, key=lambda path: path.stat().st_mtime_ns)


def main():
    environment = os.environ.copy()
    environment["CARGO_PROFILE_DEV_PANIC"] = "unwind"
    subprocess.run(["cargo", "build", "--locked", "--offline", "-p", "helix-core", "-p", "next-abi"],
                   cwd=ROOT, env=environment, check=True)
    output = ROOT / "build/linux-fs-contract-tests"
    output.parent.mkdir(parents=True, exist_ok=True)
    environment["CARGO_MANIFEST_DIR"] = str(ROOT)
    subprocess.run([
        "rustc", "--edition=2021", "--test", "-A", "dead_code", "-D", "warnings",
        str(ROOT / "tools/linux_fs_contract_tests.rs"),
        "-L", f"dependency={ROOT / 'target/debug/deps'}",
        "--extern", f"helix_core={library('helix_core')}",
        "--extern", f"next_abi={library('next_abi')}",
        "-o", str(output),
    ], cwd=ROOT, env=environment, check=True)
    subprocess.run([str(output)], cwd=ROOT, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
