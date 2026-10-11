#!/usr/bin/env python3
"""Extend the unchanged desktop smoke runner with eighth-wave UDP wait proofs.

Uses boot_smoke's actual QEMU, QMP, responders, serial checks and cleanup. The
existing portal hook provides a live guest session; no baseline implementation
is copied or modified. Use the same arguments as tools/boot_smoke.py.
"""
import hashlib
import json
from pathlib import Path
import re
import sys

import boot_smoke

ROOT = boot_smoke.ROOT


def source_digest():
    digest = hashlib.sha256()
    files = [ROOT / name for name in
             ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md")]
    for directory in ("crates", "tools", "docs"):
        files.extend(path for path in (ROOT / directory).rglob("*")
                     if path.is_file() and "__pycache__" not in path.parts
                     and path.suffix != ".pyc")
    for path in sorted(files):
        name = path.relative_to(ROOT).as_posix().encode()
        data = path.read_bytes()
        digest.update(len(name).to_bytes(8, "little"))
        digest.update(name)
        digest.update(len(data).to_bytes(8, "little"))
        digest.update(data)
    return digest.hexdigest()


def audit_boot_notifications(data):
    boundary = data.index(b"PRISM: READY")
    exits = [(int(match[1]), int(match[2])) for match in re.finditer(
        rb"HELIX: PROCESS EXIT pid=(\d+) status=(-?\d+) preemptions=\d+\r?\n",
        data[:boundary])]
    if len({pid for pid, _ in exits}) != len(exits):
        raise RuntimeError("process lifecycle emitted duplicate boot exits")
    expected = exits[-8:]
    notifications = [(int(match[1]), int(match[2])) for match in re.finditer(
        rb"PRISM: process notification pid=(\d+) exit=(-?\d+) accepted=true\r?\n",
        data[boundary:])]
    if notifications[:len(expected)] != expected:
        raise RuntimeError("boot notification drain lost or impersonated retained lifecycle records")
    loss = max(0, len(exits) - 8)
    overwritten = re.search(rb"PRISM: process notification queue overwritten=(\d+)\r?\n",
                            data[boundary:])
    if loss and (overwritten is None or int(overwritten[1]) != loss):
        raise RuntimeError("boot lifecycle queue loss count does not match bounded capacity")
    if not loss and overwritten is not None and int(overwritten[1]) != 0:
        raise RuntimeError("notification queue reported unexplained boot loss")
    return {"boot_exit_count": len(exits), "retained_pids": [pid for pid, _ in expected],
            "overwritten": loss}


def exercise_waits(serial, data, wait_text, wait_regex, wait_exit,
                   qmp_command, remaining, report):
    del qmp_command
    spawned = wait_regex(
        rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/udp-probe\.elf profile=Native")
    pid = int(spawned[1])
    exited = wait_exit(pid, 47, spawned.start())
    interval = data[spawned.start():exited.start()]
    for direction in (b"read", b"write"):
        marker = b"HELIX: DATAGRAM WAIT parked pid=" + str(pid).encode() + b" direction=" + direction
        if not re.search(re.escape(marker) + rb"\r?\n", interval):
            raise RuntimeError(f"UDP probe did not actually park for {direction.decode()} readiness")
    marker = b"UDP WAIT: delayed readable park, writable pressure wake, omitted deadline and duplicate/nonconsume authority passed"
    if marker not in interval:
        raise RuntimeError("UDP probe exited without eighth wait proof marker")
    report["checks"].append("actual delayed readable and pressure-blocked writable socket waits, timeout, authority and nonconsuming copyout")
    report["udp_wait_probe_pid"] = pid
    helpers = []
    helper_start = len(data)

    def command(text, expected):
        remaining()
        start = len(data)
        serial.sendall((text + "\r").encode())
        wait_text(expected, start)
        end = data.index(expected.encode(), start) + len(expected)
        wait_text("PRISM FRAME:", end)
        return start

    for _ in range(2):
        start = len(data)
        serial.sendall(b"run /system/udp-block.elf\r")
        spawned = wait_regex(
            rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/udp-block\.elf profile=Native", start)
        helper = int(spawned[1])
        if helper in helpers:
            raise RuntimeError("blocked helper reused an existing PID")
        helpers.append(helper)
        wait_text("UDP BLOCK: connected port 49207 acquired; parking readable for kill/reopen proof", start)
        wait_text(f"HELIX: DATAGRAM WAIT parked pid={helper} direction=read", start)
        command("ps", f"NEXT PROCESS: pid={helper} state=waiting")
        killed = command(f"kill {helper}", f"NEXT KILL: pid={helper}")
        wait_exit(helper, -13, killed)
    final = command("ps", "NEXT PROCESS: pid=1")
    states = {int(match[1]): match[2].decode() for match in re.finditer(
        rb"NEXT PROCESS: pid=(\d+) state=(\w+) preemptions=\d+ console=(true|false)\r?\n",
        data[final:])}
    if any(helper in states for helper in helpers):
        raise RuntimeError("killed blocked helper survived process collection")
    if states.get(1) not in ("runnable", "sleeping", "waiting"):
        raise RuntimeError("PID1 is not alive after blocked socket cleanup")
    if any(state == "exited" for state in states.values()):
        raise RuntimeError("unexpected unreaped process after blocked socket cleanup")
    if b"HELIX: PROCESS FAULT" in data[helper_start:]:
        raise RuntimeError("unexpected fault during final process verification")
    report["udp_blocked_killed_pids"] = helpers
    report["checks"].append("two actual blocked socket processes killed and collected; identical connected port49207 reopened after teardown")
    report["boot_notifications"] = audit_boot_notifications(data)
    report["checks"].append("trusted bootstrap exit notifications retain exact newest eight PIDs/statuses with counted queue loss")


def main():
    if "--help" in sys.argv or "-h" in sys.argv:
        boot_smoke.main()
        return
    if any(argument.split("=", 1)[0] in
           ("--persistent-data", "--persistence-phase", "--expect-boot-failure")
           for argument in sys.argv):
        raise SystemExit("UDP wait gate requires the default successful snapshot boot profile")
    if not any(argument == "--output-dir" or argument.startswith("--output-dir=")
               for argument in sys.argv):
        sys.argv.extend(["--output-dir", str(ROOT / "build/udp-wait")])
    before = source_digest()
    frozen = ROOT / "work/eighth-wave/source-manifest.json"
    frozen_before = hashlib.sha256(frozen.read_bytes()).hexdigest()
    frozen_files = len(json.loads(frozen.read_text()))
    original = boot_smoke.exercise_file_portal

    def extension(*args):
        exercise_waits(*args)
        return original(*args)

    boot_smoke.exercise_file_portal = extension
    try:
        result = boot_smoke.main()
    finally:
        boot_smoke.exercise_file_portal = original
        after = source_digest()
        arguments = sys.argv[1:]
        for index, argument in enumerate(arguments):
            if argument == "--output-dir":
                output = Path(arguments[index + 1])
                break
            if argument.startswith("--output-dir="):
                output = Path(argument.split("=", 1)[1])
                break
        report_path = output.resolve() / "boot-smoke.json"
        if report_path.exists():
            report = json.loads(report_path.read_text())
            report["source_before_sha256"] = before
            report["source_after_sha256"] = after
            report["source_unchanged"] = before == after
            report["workspace_source_drift"] = before != after
            report["source_drift_policy"] = "Informational: other agents may continue authorized source work; guest claims bind to immutable image and frozen compiled-source manifest."
            frozen_after = hashlib.sha256(frozen.read_bytes()).hexdigest()
            report["compiled_source_manifest"] = {
                "path": str(frozen.relative_to(ROOT)), "files": frozen_files,
                "before_sha256": frozen_before, "after_sha256": frozen_after,
                "unchanged": frozen_before == frozen_after,
            }
            if frozen_before != frozen_after:
                report["passed"] = False
                report["error"] = "frozen compiled-source manifest changed during UDP wait gate"
            report_path.write_text(json.dumps(report, indent=2) + "\n")
        if hashlib.sha256(frozen.read_bytes()).hexdigest() != frozen_before:
            raise SystemExit("frozen compiled-source manifest changed during UDP wait gate")
    return result


if __name__ == "__main__":
    sys.exit(main())
