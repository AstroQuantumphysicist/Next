#!/usr/bin/env python3
"""Exercise ninth-wave signed native DNS capabilities in the real guest.

Reuses the desktop baseline's QEMU, local DNS peer, immutable image checks and
cleanup. This extension executes before the existing live file-portal gate.
"""
import re
import sys

import boot_smoke

ROOT = boot_smoke.ROOT
MARKER = "DNS PROBE: actual A/AAAA peer answers, cache, readonly/short copyout, retained duplicate results, stale handles, negative/unsupported and malformed/timeout passed"
DENIED = "SERVICE WORKER: NETWORK declaration without execution grant denied DNS lookup"


def exercise_dns(serial, data, wait_text, wait_regex, wait_exit,
                 qmp, remaining, report, original):
    remaining()
    wait_text(DENIED)
    start = len(data)
    serial.sendall(b"run /system/dns-probe.elf\r")
    spawned = wait_regex(
        rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/dns-probe\.elf profile=Native", start)
    pid = int(spawned[1])
    identity = wait_regex(rb"HELIX: APPLICATION IDENTITY pid=" + str(pid).encode()
                          + rb" package=([^ ]+) signer=", start)
    if identity[1] != b"org.next.development.native.dns-probe":
        raise RuntimeError("DNS probe did not execute under its authenticated package identity")
    # Match the kernel's authenticated process log prefix, rather than accepting
    # a marker emitted by an unrelated process or echoed console input.
    wait_text(f"NEXT APP pid={pid}: {MARKER}", start)
    exited = wait_exit(pid, 48, start)
    interval = data[start:exited.start()]
    parks = len(re.findall(rb"HELIX: DNS WAIT parked pid=" + str(pid).encode()
                           + rb"\r?\n", interval))
    if parks < 4:
        raise RuntimeError("DNS fixture did not suspend on actual asynchronous requests")
    if re.search(rb"HELIX: PROCESS FAULT pid=" + str(pid).encode() + rb" ", data[start:]):
        raise RuntimeError("DNS fixture faulted instead of completing its syscall checks")
    final = len(data)
    serial.sendall(b"ps\r")
    wait_text("NEXT PROCESS: pid=1", final)
    response_end = data.index(b"NEXT PROCESS: pid=1", final) + len(b"NEXT PROCESS: pid=1")
    wait_text("PRISM FRAME:", response_end)
    if re.search(rb"NEXT PROCESS: pid=" + str(pid).encode() + rb" ", data[final:]):
        raise RuntimeError("completed DNS fixture was not collected")
    report["dns_capability"] = {
        "probe_pid": pid, "package_id": identity[1].decode(), "exit_code": 48,
        "actual_park_count": parks, "marker": MARKER,
        "authority": "Worker signed NETWORK declaration is denied when actual execution grant is zero",
        "checks": ["A=10.0.2.2 and AAAA=::1 over fixed trusted resolver",
                   "cached answer and complete144-byte bounded wire output",
                   "READ/WAIT separation, short/readonly/null/guard output without result destruction",
                   "duplicate lifetime, repeated terminal results and stale generation checks",
                   "NXDOMAIN/TC/CNAME/referral explicit negative or unsupported results",
                   "omitted and malformed replies reach bounded timeout",
                   "four held requests exhaust broker; failed fifth allocation rolls back; close permits reuse"],
        "limits": "Development UDP resolver only; no recursive search, DNSSEC, TCP fallback, alias/referral completion or production network consent",
    }
    report["checks"].append("actual signed native DNS capability fixture passes answers/cache/copyout/rights/duplicates/terminal outcomes/timeouts/quota and cleanup")
    report["checks"].append("configured worker cannot turn a signed NETWORK request into a DNS execution grant")
    return original(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report)


def main():
    if any(argument.split("=", 1)[0] in ("--persistent-data", "--persistence-phase", "--expect-boot-failure")
           for argument in sys.argv):
        raise SystemExit("DNS capability gate requires a successful snapshot boot profile")
    if not any(arg == "--output-dir" or arg.startswith("--output-dir=") for arg in sys.argv):
        sys.argv.extend(["--output-dir", str(ROOT / "build/dns-capability")])
    original = boot_smoke.exercise_file_portal
    boot_smoke.exercise_file_portal = lambda *args: exercise_dns(*args, original)
    try:
        return boot_smoke.main()
    finally:
        boot_smoke.exercise_file_portal = original


if __name__ == "__main__":
    sys.exit(main())
