#!/usr/bin/env python3
"""Compile the real Linux file adapter against bounded host device stubs."""
from pathlib import Path
import os
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def library(crate, dependencies):
    candidates = list(dependencies.glob(f"lib{crate}-*.rlib"))
    if not candidates:
        raise RuntimeError(f"cargo did not produce the {crate} host library")
    return max(candidates, key=lambda path: path.stat().st_mtime_ns)


def main():
    environment = os.environ.copy()
    environment["CARGO_PROFILE_DEV_PANIC"] = "unwind"
    # Keep the unwind libraries isolated from the workspace's ordinary dev
    # artifacts. Other test/build commands can leave newer panic=abort rlibs in
    # target/debug/deps, and selecting the newest there can link the wrong ABI.
    target_dir = ROOT / "work/linux-fs-contract-target"
    dependencies = target_dir / "debug/deps"
    # Cargo fingerprints can otherwise keep a stale panic=abort library after
    # the isolated target has previously been built with a different profile.
    subprocess.run(["cargo", "clean", "--target-dir", str(target_dir)],
                   cwd=ROOT, env=environment, check=True)
    subprocess.run(["cargo", "build", "--locked", "--offline", "-p", "helix-core", "-p", "next-abi",
                    "--target-dir", str(target_dir)],
                   cwd=ROOT, env=environment, check=True)
    output = ROOT / "build/linux-fs-contract-tests"
    output.parent.mkdir(parents=True, exist_ok=True)
    environment["CARGO_MANIFEST_DIR"] = str(ROOT)
    subprocess.run([
        "rustc", "--edition=2021", "--test", "-A", "dead_code", "-D", "warnings",
        str(ROOT / "tools/linux_fs_contract_tests.rs"),
        "-L", f"dependency={dependencies}",
        "--extern", f"helix_core={library('helix_core', dependencies)}",
        "--extern", f"next_abi={library('next_abi', dependencies)}",
        "-o", str(output),
    ], cwd=ROOT, env=environment, check=True)
    subprocess.run([str(output)], cwd=ROOT, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
