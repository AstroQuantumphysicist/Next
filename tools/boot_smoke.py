#!/usr/bin/env python3
"""Exercise the actual development image in QEMU; never substitute host tests."""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import wave
import struct
from udp_responder import Responder
from dns_responder import Responder as DnsResponder
from tcp_responder import Responder as TcpResponder
from lapic_outcome import parse_outcome as parse_lapic_outcome, verify_continuation as verify_lapic_continuation

ROOT = Path(__file__).resolve().parents[1]
BOOT_FAILURE_CODES = {
    "panic": 0x4E580001,
    "deployment": 0x4E580101,
    "kernel": 0x4E580201,
    "initrd": 0x4E580301,
    "firmware-handoff": 0x4E580401,
    "boot-record": 0x4E580501,
}


def validate_boot_failure_record(serial_data, expected_stage=None):
    """Validate one stable loader failure record in an expected rejection log."""
    if expected_stage is not None and expected_stage not in BOOT_FAILURE_CODES:
        raise ValueError(f"unknown boot failure stage: {expected_stage}")
    matches = re.findall(
        rb"(?m)^NEXT BOOT FAILURE: code=0x([0-9a-f]{8}) stage=([a-z-]+) action=HALT\r?$",
        serial_data,
    )
    if len(matches) != 1:
        raise RuntimeError(f"expected exactly one stable loader failure record, found {len(matches)}")
    code_text, stage_bytes = matches[0]
    stage = stage_bytes.decode("ascii")
    code = int(code_text, 16)
    if stage not in BOOT_FAILURE_CODES or code != BOOT_FAILURE_CODES[stage]:
        raise RuntimeError(f"unknown or inconsistent loader failure record: code=0x{code:08x} stage={stage}")
    if expected_stage is not None and stage != expected_stage:
        raise RuntimeError(f"loader failure stage was {stage}, expected {expected_stage}")
    return f"NEXT BOOT FAILURE: code=0x{code:08x} stage={stage} action=HALT"


def exercise_file_portal(serial, serial_data, wait_text, wait_regex, wait_exit,
                         qmp_command, remaining, report):
    """Approve an actual scoped file through the trusted modal using PS/2 keys."""
    def command(text, expected):
        mark = len(serial_data)
        serial.sendall((text + "\r").encode())
        wait_text(expected, mark)
        end = serial_data.index(expected.encode("ascii"), mark) + len(expected)
        wait_text("PRISM FRAME:", end)

    def key(code):
        qmp_command("send-key", {
            "keys": [{"type": "qcode", "data": code}], "hold-time": 40})
        time.sleep(min(0.08, remaining()))

    command("write /native/portal-demo portal-data", "NEXT DATA: Wrote 11 bytes")
    started = len(serial_data)
    serial.sendall(b"run /system/portal-probe.elf\r")
    spawned = wait_regex(
        rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/portal-probe\.elf profile=Native",
        started)
    pid = int(spawned[1])
    wait_text("PORTAL-PROBE: waiting for readonly file consent", started)
    modal = wait_regex(rb"PRISM PORTAL: request=(\d+) pid=(\d+) access=(\d+)\r?\n", started)
    if int(modal[2]) != pid or int(modal[3]) != 1:
        raise RuntimeError("readonly chooser identity/access differs from the actual native caller")
    initial = wait_regex(rb"PRISM PORTAL SELECTED: path=([^\r\n ]+)\r?\n", modal.end())
    wait_regex(rb"PRISM (?:OUTPUT )?FRAME: generation=\d+\r?\n", initial.end())

    # The file list comes from the trusted backend. Do not assume an inode
    # ordering; cycle through the bounded list using actual navigation events.
    for _ in range(64):
        mark = len(serial_data)
        key("down")
        selected = wait_regex(rb"PRISM PORTAL SELECTED: path=([^\r\n ]+)\r?\n", mark)
        wait_regex(rb"PRISM (?:OUTPUT )?FRAME: generation=\d+\r?\n", selected.end())
        if selected[1] == b"/native/portal-demo":
            break
    else:
        raise RuntimeError("trusted chooser did not expose the existing portal-demo file")
    approval = len(serial_data)
    key("tab")
    key("ret")
    wait_text("PORTAL-PROBE: waiting for cancelled readwrite request", approval)
    second = wait_regex(rb"PRISM PORTAL: request=(\d+) pid=(\d+) access=(\d+)", approval)
    if int(second[2]) != pid or int(second[3]) != 3 or second[1] == modal[1]:
        raise RuntimeError("readwrite chooser reused a request or changed authenticated caller/access")
    cancelled = len(serial_data)
    key("esc")
    wait_text("PORTAL-PROBE: approved readonly file, denied write, cancel and expiry passed", cancelled)
    wait_exit(pid, 0, started)
    command("cat /native/portal-demo", "NEXT DATA: portal-data")
    command("rm /native/portal-demo", "NEXT DATA: Removed /native/portal-demo")
    report["portal_probe_pid"] = pid
    report["checks"].append(
        "trusted PS/2 file chooser grants scoped readonly authority to the authenticated caller; "
        "write denial, UI rejection, unattended deadline, cancellation and stale requests pass")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpu", choices=["qemu64","max"],default="qemu64")
    parser.add_argument("--smp", type=int, choices=[1, 2], default=1,
                        help="configured QEMU vCPUs; --smp 2 is a BSP-only topology/scheduler regression profile, not SMP scheduling")
    parser.add_argument("--require-lapic-calibration", action="store_true", help="require successful optional LAPIC calibration rather than a verified PIT fallback")
    parser.add_argument("--require-guarded-emergency-stack", action="store_true", help="require both actual CPU page-fault probes for the guarded emergency IST stack")
    parser.add_argument("--trace-exceptions", action="store_true", help="save QEMU CPU exception diagnostics below the output directory")
    parser.add_argument("--timeout", type=float, default=45.0)
    parser.add_argument("--image", type=Path, default=ROOT / "build/next-dev.img")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build")
    parser.add_argument("--expect-boot-failure", help="require this exact loader diagnostic and no kernel entry")
    parser.add_argument("--expect-failure-stage", choices=sorted(BOOT_FAILURE_CODES),
                        help="require the corresponding stable loader failure code in serial output")
    parser.add_argument("--capture-prism-gallery", action="store_true",
                        help="capture actual guest Home, Files, Settings and Terminal screens")
    parser.add_argument("--persistent-data", action="store_true", help="write only to a disposable image below work/")
    parser.add_argument("--gpt-data-profile", action="store_true", help="require owner-authenticated GPT journal mount instead of legacy superfloppy storage markers")
    parser.add_argument("--persistence-phase", choices=["create", "verify"])
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be positive and finite")
    if args.expect_failure_stage and not args.expect_boot_failure:
        parser.error("--expect-failure-stage requires --expect-boot-failure")
    build = args.output_dir.resolve()
    if not build.is_relative_to(ROOT / "build"):
        parser.error("output directory must be below build/")
    build.mkdir(parents=True, exist_ok=True)
    image = args.image.resolve()
    if args.persistent_data and not args.image.resolve().is_relative_to(ROOT / "work"):
        parser.error("persistent writes require a disposable image below work/")
    if args.persistence_phase and not args.persistent_data:
        parser.error("persistence phase requires --persistent-data")
    screenshot = build / "screenshot.ppm"
    screenshot.unlink(missing_ok=True)
    report = {"passed": False, "scope": ("QEMU q35/TCG with two configured vCPUs, validated ACPI topology, APs intentionally stopped, and scheduler workload exercised only on the BSP; legacy PS/2, software framebuffer, PIT/PIC timer interrupts, legacy virtio read/write/flush in dedicated trailing data region, FAT32 reads, native processes and IPC; no SMP scheduling or hardware certification" if args.smp == 2 else "QEMU q35/TCG, single CPU, legacy PS/2, software framebuffer, PIT/PIC timer interrupts, legacy virtio read/write/flush in dedicated trailing data region, FAT32 reads, native processes and IPC; no hardware certification"),
              "configured_vcpus": args.smp,
              "ap_startup_enabled": False,
              "scheduler_cpu_scope": "bootstrap CPU only",
              "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(), "checks": []}
    serial_data = bytearray()
    (build / "serial.log").write_bytes(b"")
    (build / "qemu-smoke.log").write_bytes(b"")
    (build / "boot-smoke.json").write_text(json.dumps(report, indent=2) + "\n")
    process = None
    serial = None
    qmp = None
    qmp_stream = None
    qmp_id = 0
    interrupted = False
    udp_peer = None
    dns_peer = None
    tcp_peer = None
    deadline = time.monotonic() + args.timeout
    env = os.environ.copy()
    sysroot = Path(env.get("NEXT_SYSROOT", "/workspace/toolchains/sysroot"))
    local_qemu = sysroot / "usr/bin/qemu-system-x86_64"
    qemu = str(local_qemu) if local_qemu.exists() else shutil.which("qemu-system-x86_64")
    if local_qemu.exists():
        lib = str(sysroot / "usr/lib/x86_64-linux-gnu")
        env["LD_LIBRARY_PATH"] = lib + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    firmware = sysroot / "usr/share/OVMF"
    if not firmware.exists():
        firmware = Path("/usr/share/OVMF")

    def remaining():
        seconds = deadline - time.monotonic()
        if seconds <= 0:
            raise TimeoutError(f"boot smoke exceeded {args.timeout:g} seconds")
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"QEMU exited with status {process.returncode}; see build/qemu-smoke.log")
        return seconds

    def connect(path):
        while True:
            remaining()
            channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                channel.connect(str(path))
                return channel
            except (FileNotFoundError, ConnectionRefusedError):
                channel.close()
                time.sleep(min(0.05, remaining()))

    def receive_serial():
        chunk = serial.recv(65536)
        if not chunk:
            raise RuntimeError("QEMU closed serial channel")
        serial_data.extend(chunk)
        (build / "serial.log").write_bytes(serial_data)
        if args.expect_boot_failure and b"HELIX:" in serial_data:
            raise RuntimeError("rejected image unexpectedly entered kernel")
        fatal_markers = [b"HELIX PANIC:", b"HELIX FATAL:",
                b"HELIX TIMER FAILED:",
                b"HELIX USER FAILED:", b"HELIX DISK FAILED:", b"HELIX FS FAILED:",
                b"HELIX: NATIVE STARTUP FAILED", b"HELIX NETWORK FAILED", b"HELIX AUDIO FAILED",
                b"HELIX: invalid ", b"PRISM: invalid "]
        if not args.expect_boot_failure:
            fatal_markers.append(b"NEXT BOOT FAILED:")
        if any(fatal in serial_data for fatal in fatal_markers):
            raise RuntimeError("kernel or firmware failure in serial log")

    def wait_text(text, start=0):
        marker = text.encode("ascii")
        while marker not in serial_data[start:]:
            seconds = remaining()
            ready, _, _ = select.select([serial], [], [], min(0.1, seconds))
            if ready:
                receive_serial()

    def wait_regex(pattern, start=0):
        expression = re.compile(pattern)
        while True:
            match = expression.search(serial_data, start)
            if match:
                return match
            ready, _, _ = select.select([serial], [], [], min(0.1, remaining()))
            if ready:
                receive_serial()

    def wait_exit(pid, status, start):
        match = wait_regex(rb"HELIX: PROCESS EXIT pid=" + str(pid).encode() + rb" status=(-?\d+) preemptions=(\d+)\r?\n", start)
        if int(match[1]) != status:
            raise RuntimeError(f"process {pid} exited {int(match[1])}, expected {status}")
        return match

    def qmp_read():
        qmp.settimeout(remaining())
        line = qmp_stream.readline()
        if not line:
            raise RuntimeError("QEMU closed QMP channel")
        return json.loads(line)

    def qmp_command(command, arguments=None):
        nonlocal qmp_id
        qmp_id += 1
        request = {"execute": command, "id": qmp_id}
        if arguments is not None:
            request["arguments"] = arguments
        qmp.sendall(json.dumps(request).encode() + b"\n")
        while True:
            response = qmp_read()
            if response.get("id") == qmp_id:
                if "error" in response:
                    raise RuntimeError(f"QMP {command}: {response['error']}")
                return response.get("return")

    def terminate_signal(_signum, _frame):
        raise KeyboardInterrupt("termination requested")

    previous_term = signal.signal(signal.SIGTERM, terminate_signal)
    try:
        if not args.expect_boot_failure:
            udp_peer = Responder()
            dns_peer = DnsResponder(39099, drop_first=True)
            tcp_peer = TcpResponder()
        if not qemu:
            raise RuntimeError("qemu-system-x86_64 is missing; source tools/env.sh")
        if not image.is_file():
            raise RuntimeError("build/next-dev.img is missing; run tools/build.sh")
        report["image_sha256"] = hashlib.sha256(image.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory(prefix="next-smoke-", dir="/tmp") as temp_name, (build / "qemu-smoke.log").open("wb") as qemu_log:
            temp = Path(temp_name)
            env["TMPDIR"] = str(temp)
            protected_digest = hashlib.sha256(image.read_bytes()[:64 * 1024 * 1024]).hexdigest()
            variables = temp / "vars.fd"
            shutil.copyfile(firmware / "OVMF_VARS_4M.fd", variables)
            command = [qemu, "-machine", "q35,accel=tcg", "-cpu",args.cpu, "-m", "256M", "-smp", str(args.smp), "-vga", "std",
                       "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE_4M.fd'}",
                       "-drive", f"if=pflash,format=raw,file={variables}",
                       "-drive", f"if=none,id=nextdisk,format=raw,{'' if args.persistent_data else 'snapshot=on,'}file={image}",
                       "-device", "virtio-blk-pci,drive=nextdisk,disable-modern=on",
                       "-netdev", "user,id=nextnet", "-device", "virtio-net-pci,netdev=nextnet,disable-modern=on",
                       "-audiodev", f"wav,id=nextaudio,path={build / 'audio.wav'},out.frequency=48000,out.channels=2,out.format=s16",
                       "-device", "AC97,audiodev=nextaudio", "-display", "none",
                       "-serial", f"unix:{temp / 'serial.sock'},server=on,wait=off",
                       "-qmp", f"unix:{temp / 'qmp.sock'},server=on,wait=off", "-no-reboot"]
            if args.trace_exceptions:
                command.extend(["-d","int,cpu_reset","-D",str(build / "qemu-exceptions.log")])
            for data in ["usr/share/qemu", "usr/share/seabios", "usr/lib/ipxe/qemu"]:
                if (sysroot / data).exists():
                    command.extend(["-L", str(sysroot / data)])
            report["qemu_profile"] = f"q35,accel=tcg; cpu={args.cpu}; 256M; smp={args.smp}; vga=std"
            process = subprocess.Popen(command, stdout=qemu_log, stderr=subprocess.STDOUT, env=env)
            serial = connect(temp / "serial.sock")
            qmp = connect(temp / "qmp.sock")
            qmp_stream = qmp.makefile("rb")
            if "QMP" not in qmp_read():
                raise RuntimeError("invalid QMP greeting")
            qmp_command("qmp_capabilities")
            configured_cpus = qmp_command("query-cpus-fast")
            if not isinstance(configured_cpus, list) or len(configured_cpus) != args.smp:
                raise RuntimeError(f"QMP reports {len(configured_cpus) if isinstance(configured_cpus, list) else 'invalid'} CPUs, expected {args.smp}")
            report["qmp_cpus"] = [{"cpu_index": cpu.get("cpu-index"), "thread_id": cpu.get("thread-id"),
                                    "apic_id": cpu.get("props", {}).get("apic-id")}
                                   for cpu in configured_cpus]
            report["checks"].append(f"QMP reports exactly {args.smp} configured QEMU CPU object(s)")
            if args.expect_boot_failure:
                wait_text("NEXT BOOT FAILED: " + args.expect_boot_failure)
                failure_record = validate_boot_failure_record(
                    bytes(serial_data), args.expect_failure_stage
                )
                report["loader_failure_record"] = failure_record
                report["checks"].append("stable loader failure record: " + failure_record)
                # Keep observing a halted guest: no kernel/desktop entry and no
                # firmware reboot after the diagnostic. Serial polling is bounded.
                end_observation = time.monotonic() + 0.5
                while time.monotonic() < end_observation:
                    ready, _, _ = select.select([serial], [], [], min(0.05, remaining()))
                    if ready:
                        receive_serial()
                if serial_data.count(b"NEXT BOOT development") != 1:
                    raise RuntimeError("loader repeated boot rather than halting")
                report["checks"].append("expected loader rejection: " + args.expect_boot_failure)
                report["checks"].append("no kernel entry or reboot during bounded halt observation")
            else:
                wait_text("NEXT BOOT: SIGNATURE PASS")
                report["checks"].append("Ed25519 deployment signature and kernel/init-image SHA256 verified against workspace development anchor")
                wait_text("NEXT BOOT: ELF LOAD PASS")
                report["checks"].append("separate validated ELF loaded from EFI boot-device filesystem")
                wait_text("HELIX: ExitBootServices succeeded")
                report["checks"].append("firmware boot services exited")
                wait_text("HELIX: BOOTINFO SELFTEST PASS")
                report["checks"].append("versioned checksummed BootInfo with kernel/loader/map reservations")
                wait_text("HELIX: KERNEL MAPPINGS PASS")
                report["checks"].append("supervisor RX text, RO NX data, RW NX memory and guarded stacks")
                for protection in ["RX text write", "RO data write", "user code physical alias write", "NX data execution", "kernel stack guard write", "interrupt stack guard write"]:
                    wait_text("HELIX: KERNEL PROTECTION PASS " + protection + " denied by CPU page fault")
                    report["checks"].append("CPU kernel protection: " + protection)
                report["guarded_emergency_stack_verified"] = False
                if args.require_guarded_emergency_stack:
                    for protection in ["emergency IST lower guard write", "emergency IST upper guard write"]:
                        wait_text("HELIX: KERNEL PROTECTION PASS " + protection + " denied by CPU page fault")
                        report["checks"].append("CPU kernel protection: " + protection)
                    report["guarded_emergency_stack_verified"] = True
                wait_text("HELIX: SELFTEST PASS")
                report["checks"].append("kernel physical-page and handle selftest")
                wait_text("HELIX: USER SELFTEST PASS")
                report["checks"].append("isolated ring3 native code, bounded syscall, pointer denial and exit")
                for protection in ["supervisor read", "RX code write", "NX stack execution"]:
                    wait_text("HELIX: USER PROTECTION PASS " + protection + " denied by CPU page fault")
                    report["checks"].append("CPU page fault isolation: " + protection)
                wait_text("HELIX: TIMER SELFTEST PASS")
                report["checks"].append("PIT timer interrupts preserve tested integer registers and XMM0 sentinel")
                wait_text("HELIX DISK SELFTEST PASS")
                report["checks"].append("actual read-only legacy virtio block device")
                if args.gpt_data_profile:
                    wait_text("HELIX GPT DISCOVERY PASS")
                    wait_text("HELIX GPT ESP NOTE: Read by Helix from the Next development disk.")
                    report["checks"].append("validated GPT GUID selection and ESP-relative FAT32 /NEXT.TXT read")
                else:
                    wait_text("HELIX FS SELFTEST PASS /NEXT.TXT")
                    report["checks"].append("actual read-only FAT32 file access: /NEXT.TXT")
                for marker, check in [
                    ("HELIX: CPU CONTEXT PROFILE PASS", "kernel readback verifies only saved FX512 state is enabled; unsaved optional user controls and inherited TLS bases disabled"),
                    ("NEXT BOOT: INITRD LOAD PASS", "bounded boot-device init image loaded and retained"),
                    ("PROBE: SDK native TLS FS/private/yield/sleep/timer checks passed", "native SDK TLS setters/getters preserve private FS memory across yield, sleep and timer switches and reject invalid bases/copyout"),
                    ("PROBE: bounded user-copy and capability checks passed", "native executable rejects invalid pointers and stale capability handles"),
                    ("PROBE: anonymous memory zeroing/range/W^X checks passed", "native anonymous memory supports zeroed multi-page mappings and enforces range/permissions"),
                    ("NEXTD: authenticated service configuration and copyout checks passed", "PID1 reads authenticated signed service declarations with no partial writes on short, null, readonly or guard-crossing outputs"),
                    ("NEXTD: manual/auto event lifecycle and rights checks passed", "actual manual/auto events enforce wait/signal rights and copyout-before-consumption"),
                    ("NEXTD: blocking event timeout, signal-reset auto wake, manual broadcast, process wait and kill cancellation passed", "actual blocking waits enforce deadlines, preserve a delivered auto-event completion across reset, broadcast manual events and clean up killed waiters"),
                    ("EVENT WAITER: atomic wait-many timeout, poll, signal/reset and selected index passed", "signed guest fixture validates ObjectWaitMany timeout cleanup, nonblocking WouldBlock, blocked wake, selected index and reset-stable completion across two transferred event handles"),
                    ("NEXTD: native file read/write/seek/stat and copyout checks passed", "capability-backed COW file streams preserve offsets on denied copyout and enforce attenuated rights"),
                    ("PROBE: transferred readonly file rights enforced", "cross-process file capability transfer grants read and denies write"),
                    ("NEXTD: transferred log authority written", "cross-process capability move produces usable attenuated recipient authority"),
                    ("NEXTD: isolated-process IPC request/reply passed", "framed IPC request/reply between distinct native address spaces"),
                    ("NEXTD: supervisor/RX/NX/guard/UD faults contained", "CPU faults for supervisor access, RX write, NX execute, stack guard and invalid opcode terminate only offending child"),
                    ("NEXTD: IRQ integer/XMM/x87 preservation passed", "native integer, all XMM and x87 sentinels survive noncooperative timer context switches"),
                    ("HELIX HPET PASS", "validated q35 HPET resource uses owned UC mapping and advancing hardware counter; PIT remains scheduler clock"),
                    ("HELIX LAPIC PASS id=0", "read-only BSP LAPIC identity and version discovery with owned UC mapping"),
                    ("HELIX IOAPIC PASS controllers=1", "actual IOAPIC pin discovery and bounded masked ISA plan before the explicit routing transition"),
                    ("HELIX IOAPIC IRQ RESULT Report { gsi: 2", "actual PIT IRQ0 routing transition reports ISR-observed IOAPIC delivery or verified PIC rollback"),
                    ("SERVICE WORKER: NETWORK declaration without execution grant denied UDP socket", "independently attenuated worker NETWORK grant blocks socket creation despite signed declaration"),
                    ("NEXTD: recipient NETWORK capability import denial acknowledged", "moving a socket to an unprivileged recipient cannot bypass NETWORK authority; denied import preserves ownership until teardown"),
                    ("HELIX ACPI PASS cpus=1", "validated ACPI tables discover BSP, IOAPIC and interrupt overrides; no SMP startup"),
                    ("HELIX COW STORE PASS", "COW data store commits file write/rename/delete and remounts without touching ESP"),
                    ("HELIX AUDIO PASS", "actual AC97 DMA playback completes 12000 stereo frames at 48 kHz"),
                    ("HELIX DHCP PASS address=[10, 0, 2, 15]", "actual DHCP Discover/Offer/Request/Ack obtains bounded QEMU lease"),
                    ("UDP=true; QEMU development profile", "actual virtio Ethernet ARP, ICMP echo and checksummed UDP request/reply with local host peer"),
                    ("HELIX DNS PASS A/AAAA=true cache=true", "actual local UDP A/AAAA lookups validate framed replies, TTL cache hits, real monotonic expiry and wire refresh"),
                    ("HELIX TCP PASS HTTP=true", "actual TCP handshake, bounded HTTP response and connection teardown with local host peer"),
                    ("PRISM: PS2 MOUSE ready=true", "PS/2 auxiliary mouse initialized"),
                    ("HELIX STORAGE WRITE PASS", "dedicated data-sector write/read/flush and original contents restored"),
                    ("NEXTD: child fault contained; supervisor survived", "native child fault terminates offender while supervisor survives"),
                    ("NEXTD: non-yielding child preempted and killed", "timer preempts non-yielding child and supervisor kills it"),
                    ("NEXTD: retained waits, duplicate ownership, negative exit and copyout checks passed", "process results survive repeated and duplicated waits; signed exit -7 is distinct from WouldBlock and bad copyout cannot consume result"),
                    ("NEXTD: native lifecycle selftest passed", "native process exit, wait, stale handles and slot reuse"),
                    ("VFS-PROBE: independent/shared offsets, rename replacement, unlink lifetime and recreate isolation passed", "isolated native privileged VFS fixture checks independent/shared offsets, rename replacement, open unlink lifetime and recreate isolation"),
                    ("NEXTD: service crash restart, heartbeat watchdog recovery and PID1 survival passed", "native PID1 executes real isolated-worker crash restart and heartbeat timeout recovery with bounded policy"),
                    ("HELIX: NATIVE SYSTEM READY", "live native nextd supervisor signals readiness"),
                    ("HELIX: NATIVE SELFTEST PASS", "kernel completes bounded native startup"),
                ]:
                    if args.gpt_data_profile and marker == "HELIX STORAGE WRITE PASS":
                        continue
                    if args.gpt_data_profile and marker == "HELIX COW STORE PASS":
                        wait_regex(rb"HELIX GPT STORE PASS generation=(\d+) read_only=false formatted=false selftest=false")
                        report["checks"].append("owner-signed exact GPT binding mounts existing132-sector journal without formatting or repair")
                        continue
                    if marker == "HELIX ACPI PASS cpus=1":
                        topology = wait_regex(rb"HELIX ACPI PASS cpus=(\d+) io_apics=(\d+) overrides=(\d+) tables=(\d+) bsp=(\d+); discovery only")
                        discovered = int(topology[1])
                        if discovered != args.smp:
                            raise RuntimeError(f"ACPI discovered {discovered} enabled CPUs, expected {args.smp}")
                        report["acpi_topology"] = {"enabled_cpus": discovered, "io_apics": int(topology[2]),
                                                    "overrides": int(topology[3]), "validated_tables": int(topology[4]),
                                                    "bootstrap_apic_id": int(topology[5])}
                        if args.smp == 2:
                            report["checks"].append("MADT discovery finds both configured CPU records; this does not start either AP")
                        else:
                            report["checks"].append(check)
                    else:
                        wait_text(marker)
                        report["checks"].append(check)
                routing = wait_regex(rb"HELIX IOAPIC IRQ RESULT Report \{ gsi: (\d+), apic_interrupts: (\d+), pic_interrupts: (\d+), owner_retained: (true|false), result: Ok", 0)
                if int(routing[2]) < 3 or routing[4] != b"true":
                    raise RuntimeError("IOAPIC routing did not prove actual ISR-observed delivery")
                report["ioapic_delivery"] = {"gsi": int(routing[1]), "apic_interrupts": int(routing[2]), "pic_interrupts": int(routing[3])}
                if args.smp == 2:
                    if bytes(serial_data).count(b"APs remain stopped") < 2:
                        raise RuntimeError("kernel did not explicitly retain BSP-only AP reservation/scratch posture")
                    if re.search(rb"(?m)^HELIX AP (?:ONLINE|STARTED|SCHEDULER)\b", serial_data):
                        raise RuntimeError("unexpected AP activation marker in a BSP-only profile")
                    report["checks"].append("AP trampoline and scratch remain reserved with APs stopped; no AP execution or scheduler claim")
                wait_regex(rb"HELIX LAPIC TIMER (?:PASS|UNAVAILABLE)[^\r\n]*\r?\n")
                lapic_outcome = parse_lapic_outcome(bytes(serial_data), args.require_lapic_calibration)
                report["lapic_calibration"] = lapic_outcome.report()
                report["lapic_calibration"]["continuation"] = verify_lapic_continuation(bytes(serial_data))
                if lapic_outcome.calibration_verified:
                    report["checks"].append("actual masked LAPIC countdown calibrated against two bounded HPET windows; masked firmware timer adopted/stopped, LVT/divider restored and PIT retained")
                else:
                    report["checks"].append("optional LAPIC calibration refused " + lapic_outcome.reason + "; actual PIT timer, owned IOAPIC delivery and noncooperative preemption verified")
                wait_text("PRISM: READY")
                utc_boot = wait_regex(rb"HELIX UTC PASS seconds=(\d+) source=q35-CMOS/PIT", 0)
                if abs(int(utc_boot[1]) - time.time()) > 20:
                    raise RuntimeError("guest RTC UTC anchor differs from host UTC by more than 20 seconds")
                report["checks"].append("actual q35 CMOS UTC anchor agrees with host wall clock")
                report["checks"].append("framebuffer shell ready")
                service_spawns = list(re.finditer(rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/service-worker\.elf profile=Native", serial_data))
                if len(service_spawns) < 3:
                    raise RuntimeError("service recovery did not create three isolated instances")
                service_pids = [int(match[1]) for match in service_spawns[:3]]
                if len(set(service_pids)) != 3 or any(pid <= 1 for pid in service_pids):
                    raise RuntimeError("service restart reused an instance PID")
                service_order = [service_spawns[0].start()]
                first_exit = wait_exit(service_pids[0], 81, service_spawns[0].start())
                service_order.extend([first_exit.start(), service_spawns[1].start()])
                stalled = serial_data.find(b"SERVICE WORKER: intentional heartbeat stall", service_spawns[1].start())
                watchdog = serial_data.find(b"NEXTD: service heartbeat deadline expired", service_spawns[1].start())
                second_exit = wait_exit(service_pids[1], 81, service_spawns[1].start())
                service_order.extend([stalled, watchdog, second_exit.start(), service_spawns[2].start()])
                if any(index < 0 for index in service_order) or service_order != sorted(service_order):
                    raise RuntimeError("service logs do not prove crash/restart/stall/watchdog/recovery order")
                report["service_instance_pids"] = service_pids
                report["checks"].append("distinct worker PIDs and exit statuses prove crash restart and watchdog recovery ordering")
                start = len(serial_data)
                serial.sendall(b"ps\r")
                wait_text("PRISM FRAME:", start)
                wait_regex(rb"NEXT PROCESS: pid=" + str(service_pids[2]).encode() + rb" state=(sleeping|runnable)", start)
                report["checks"].append("recovered isolated service remains live alongside PID1 after desktop readiness")
                commands = [("status", "NEXT: kernel-control=independent boot-info=validated framebuffer=active input=polling"),
                            ("about", "NEXT: Helix + Prism original Rust development kernel; not Next 1.0"),
                            ("help", "native processes and IPC active; bounded COW store and VM Ethernet; development build"),
                            ("cat /NEXT.TXT", "NEXT FILE: Read by Helix from the Next development disk."),
                            ("write /smoke hello world", "NEXT DATA: Wrote 11 bytes to /smoke"),
                            ("cat /smoke", "NEXT DATA: hello world"),
                            ("mv /smoke /renamed", "NEXT DATA: Renamed /smoke to /renamed"),
                            ("ls", "/renamed 11 bytes"),
                            ("rm /renamed", "NEXT DATA: Removed /renamed"),
                            ("cat /renamed", "Data store error: NotFound"),
                            ("ps", "NEXT PROCESS: pid=1"),
                            ("kill 1", "NEXT KILL FAILED: AccessDenied"),
                            ("run /system/missing.elf", "NEXT RUN FAILED: NotFound"),
                            ("run /system/untrusted.elf", "NEXT RUN FAILED: AccessDenied"),
                            ("trace", "TRACE retained="),
                            ("calc (2 + 3) * 4", "= 20"),
                            ("calc 1 / 0", "Cannot divide by zero."),
                            ("nextunknown", "NEXT: unknown command")]
                for text, expected in commands:
                    start = len(serial_data)
                    serial.sendall((text + "\r").encode())
                    wait_text(expected, start)
                    response_end = serial_data.index(expected.encode("ascii"), start) + len(expected)
                    wait_text("PRISM FRAME:", response_end)
                    report["checks"].append("serial command: " + text)
                if b"APPLICATION SIGNATURE REJECT path=/system/untrusted.elf" not in serial_data:
                    raise RuntimeError("invalid per-application signature was not refused")
                if re.search(rb"PROCESS SPAWN[^\r\n]+path=/system/untrusted\.elf", serial_data):
                    raise RuntimeError("untrusted app received a process slot")
                if b"id=100" not in serial_data or b"id=101" not in serial_data:
                    raise RuntimeError("structured trace did not contain actual process lifecycle events")
                report["checks"].append("signed application authentication rejects tampered archive member before process creation")
                report["checks"].append("structured trace captures actual numeric process and namespace outcomes")
                start = len(serial_data)
                serial.sendall(b"run /system/spin.elf\r")
                wait_text("NEXT RUN: pid=", start)
                # Serial delivery can split the decimal PID between reads.
                wait_text("PRISM FRAME:", start)
                match = wait_regex(rb"NEXT RUN: pid=(\d+)\r?\n", start)
                if not match:
                    raise RuntimeError("console spawn returned no PID")
                spin_pid = int(match[1])
                if spin_pid <= 1:
                    raise RuntimeError("console spawn reused supervisor PID")
                start = len(serial_data)
                serial.sendall(("kill " + str(spin_pid) + "\r").encode())
                wait_text("NEXT KILL: pid=" + str(spin_pid), start)
                report["checks"].append("interactive spawn and kill of non-yielding native child; supervisor protected")
                start = len(serial_data)
                serial.sendall(b"run /compat/linux/hello.elf\r")
                wait_text("LINUX STATIC ELF: original syscall compatibility PASS", start)
                wait_text("status=37", start)
                report["checks"].append("same static Linux ELF executes Linux SYSCALL subset and exits 37")
                start=len(serial_data)
                serial.sendall(b"run /compat/linux/hello-gate.elf\r")
                wait_text("HELIX: LINUX PROFILE native syscall gate rejected number=15",start)
                wait_text("status=37",start)
                report["checks"].append("Linux profile cannot invoke native readiness capability syscall")
                for path in ["/compat/linux/clock.elf", "/compat/linux/clock-gate.elf"]:
                    start = len(serial_data)
                    serial.sendall(("run " + path + "\r").encode())
                    wait_text("LINUX CLOCK ELF: monotonic/sleep/bounds compatibility PASS",start)
                    wait_text("status=43",start)
                    report["checks"].append("original Linux clock fixture: " + path)
                start = len(serial_data)
                serial.sendall(b"run /system/clock.elf\r")
                wait_text("Next Clock / native ABI",start)
                wait_text("CLOCK: native realtime copyout checks passed",start)
                realtime = wait_regex(rb"UTC realtime: (\d+)\.(\d{9}) Unix seconds", start)
                if abs(int(realtime[1]) - time.time()) > 20 or int(realtime[2]) >= 1_000_000_000:
                    raise RuntimeError("invalid native UTC realtime output")
                report["checks"].append("native realtime clock normalizes UTC and rejects null, readonly and guard-crossing copyout")
                wait_text("status=0",start)
                report["checks"].append("native clock utility uses actual monotonic time")
                wait_text("PRISM OUTPUT FRAME:",start)
                report["checks"].append("console-owned program output reaches Prism terminal through bounded queue")
                for path in ["/compat/linux/memory.elf", "/compat/linux/memory-gate.elf"]:
                    start = len(serial_data)
                    serial.sendall(("run " + path + "\r").encode())
                    wait_text("LINUX MEMORY ELF:",start)
                    wait_text("status=44",start)
                    report["checks"].append("original Linux brk/mmap/munmap fixture: " + path)
                for path in ["/compat/linux/namespace.elf", "/compat/linux/namespace-gate.elf"]:
                    start = len(serial_data)
                    serial.sendall(("run " + path + "\r").encode())
                    wait_text("LINUX NAMESPACE ELF:", start)
                    wait_text("status=48", start)
                    report["checks"].append("original Linux shared-open, rename/unlink lifetime fixture: " + path)
                for path in ["/compat/linux/tls.elf", "/compat/linux/tls-gate.elf"]:
                    start = len(serial_data)
                    serial.sendall(("run " + path + "\r").encode())
                    spawned = wait_regex(rb"NEXT RUN: pid=(\d+)\r?\n", start)
                    pid = int(spawned[1])
                    wait_text("LINUX TLS ELF: FS/GS ownership/yield/bounds compatibility PASS", start)
                    exited = wait_exit(pid, 47, start)
                    if int(exited[2]) == 0:
                        raise RuntimeError("TLS fixture never exercised timer preemption")
                    report["checks"].append("Linux FS/GS bases survive gate, yield, preemption and bounds checks: " + path)
                start = len(serial_data)
                serial.sendall(b"run /compat/linux/tls-gate.elf\rrun /compat/linux/tls-gate.elf\r")
                first = wait_regex(rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/compat/linux/tls-gate\.elf profile=Linux", start)
                second = wait_regex(rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/compat/linux/tls-gate\.elf profile=Linux", first.end())
                tls_pids = [int(first[1]), int(second[1])]
                if tls_pids[0] == tls_pids[1]:
                    raise RuntimeError("concurrent TLS fixtures reused a live PID")
                exits = [wait_exit(pid, 47, start) for pid in tls_pids]
                if any(exit_match.start() < second.end() for exit_match in exits):
                    raise RuntimeError("TLS fixtures completed before both were running; isolation overlap unproven")
                if any(int(exit_match[2]) == 0 for exit_match in exits):
                    raise RuntimeError("concurrent TLS fixtures did not undergo timer preemption")
                if serial_data[start:].count(b"LINUX TLS ELF: FS/GS ownership/yield/bounds compatibility PASS") < 2:
                    raise RuntimeError("both concurrent TLS instances did not validate private FS/GS data")
                report["concurrent_tls_pids"] = tls_pids
                report["checks"].append("two overlapping Linux TLS instances preserve PID-specific private FS/GS contents across timer switches")
                for path in ["/compat/linux/file.elf", "/compat/linux/file-gate.elf"]:
                    start = len(serial_data)
                    serial.sendall(("run " + path + "\r").encode())
                    wait_text("LINUX FILE ELF: open/read/write/seek/stat/dup/close compatibility PASS",start)
                    wait_text("status=46",start)
                    report["checks"].append("original Linux common file-object fixture: " + path)
                for path in ["/compat/nt/hello.exe", "/compat/nt/hello-gate.exe"]:
                    start = len(serial_data)
                    serial.sendall(("run " + path + "\r").encode())
                    wait_text("NT PE32+ ELF-FREE: original Win64 imports/GPR/XMM compatibility PASS",start)
                    wait_text("status=45",start)
                    report["checks"].append("original NT PE32+ Win64 import fixture: " + path)
                start = len(serial_data)
                serial.sendall(b"run /system/udp-probe.elf\r")
                udp_spawn = wait_regex(rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/udp-probe\.elf profile=Native", start)
                wait_text("UDP PROBE: actual nonce echo fixed trusted peer 10.0.2.2:39097, authority/copyout/nonconsume/stale/reuse passed", start)
                wait_exit(int(udp_spawn[1]), 47, start)
                report["checks"].append("native connected UDP capability sends actual nonce/empty datagrams through retained virtio device with copyout-safe receive, level wait, rights and stale-handle checks")
                # Exercise authoritative Editor load/save through real PS/2 input
                # and journal I/O, including preservation after an external edit.
                def editor_key(*keys):
                    qmp_command("send-key", {"keys": [{"type": "qcode", "data": key} for key in keys], "hold-time": 40})
                    time.sleep(min(0.08, remaining()))

                def editor_command(command, expected):
                    mark = len(serial_data)
                    serial.sendall((command + "\r").encode())
                    wait_text(expected, mark)
                    response_end = serial_data.index(expected.encode("ascii"), mark) + len(expected)
                    wait_text("PRISM FRAME:", response_end)
                    return mark

                editor_command("write /native/editor hello", "NEXT DATA: Wrote 5 bytes")
                editor_command("edit /native/editor", "PRISM EDITOR OPEN: path=/native/editor bytes=5 exists=true")
                editor_key("end")
                editor_key("x")
                mark = len(serial_data)
                editor_key("ctrl", "s")
                wait_text("PRISM EDITOR SAVE: path=/native/editor bytes=6", mark)
                editor_command("cat /native/editor", "NEXT DATA: hellox")
                editor_command("edit /native/editor", "PRISM EDITOR OPEN: path=/native/editor bytes=6 exists=true")
                editor_key("end")
                editor_key("y")
                editor_command("write /native/editor external", "NEXT DATA: Wrote 8 bytes")
                # A dirty edit request focuses Editor and opens its protection
                # dialog. Escape cancels the replacement without discarding text.
                mark = len(serial_data)
                serial.sendall(b"edit /native/editor\r")
                wait_text("PRISM FRAME:", mark)
                editor_key("esc")
                wait_text("PRISM DESKTOP: focused=Some(Editor) home=false", mark)
                mark = len(serial_data)
                editor_key("ctrl", "s")
                wait_text("PRISM EDITOR CONFLICT: path=/native/editor", mark)
                editor_command("cat /native/editor", "NEXT DATA: external")
                editor_command("write /native/editor hellox", "NEXT DATA: Wrote 6 bytes")
                mark = len(serial_data)
                serial.sendall(b"edit /native/editor\r")
                wait_text("PRISM FRAME:", mark)
                editor_key("esc")
                wait_text("PRISM DESKTOP: focused=Some(Editor) home=false", mark)
                mark = len(serial_data)
                editor_key("ctrl", "s")
                wait_text("PRISM EDITOR SAVE: path=/native/editor bytes=7", mark)
                editor_command("cat /native/editor", "NEXT DATA: helloxy")
                editor_command("rm /native/editor", "NEXT DATA: Removed /native/editor")
                editor_command("date", "NEXT UTC:")
                utc_console = wait_regex(rb"NEXT UTC: seconds=(\d+) nanoseconds=(\d+) source=q35-CMOS/PIT", 0)
                if abs(int(utc_console[1]) - time.time()) > 20 or int(utc_console[2]) >= 1_000_000_000:
                    raise RuntimeError("invalid console UTC realtime output")
                report["checks"].append("console date reports normalized UTC from the same RTC/PIT clock")
                editor_command("write /native/search-demo search-content", "NEXT DATA: Wrote 14 bytes")
                editor_key("f10")
                for character in "search":
                    editor_key(character)
                search_mark = len(serial_data)
                editor_key("ret")
                wait_text("PRISM EDITOR OPEN: path=/native/search-demo bytes=14 exists=true", search_mark)
                report["checks"].append("Home search matches actual file metadata and opens its authoritative content in Editor")
                editor_command("cat /native/search-demo", "NEXT DATA: search-content")
                editor_command("rm /native/search-demo", "NEXT DATA: Removed /native/search-demo")
                exercise_file_portal(serial, serial_data, wait_text, wait_regex, wait_exit,
                                     qmp_command, remaining, report)
                # Preview is modal: Enter applies layout without executing the
                # terminal's old command, and Escape restores prior placement.
                snap_pattern = rb"PRISM SNAP: kind=([^ ]+) app=Terminal zone=([^ ]+) x=(\d+) y=(\d+) width=(\d+) height=(\d+)"
                def snap_key(expected_kind, *keys):
                    mark = len(serial_data)
                    editor_key(*keys)
                    event = wait_regex(snap_pattern, mark)
                    if event[1].decode() != expected_kind:
                        raise RuntimeError("unexpected live window snap event")
                    wait_text("PRISM FRAME:", event.end())
                    return tuple(map(int, event.groups()[2:]))
                left = snap_key("Preview", "meta_l", "left")
                left_applied = snap_key("Applied", "ret")
                if left != left_applied:
                    raise RuntimeError("snap commit differs from selected preview")
                right = snap_key("Preview", "meta_l", "right")
                restored_left = snap_key("Cancelled", "esc")
                if restored_left != left or right[0] != left[0] + left[2] or right[1:] != left[1:]:
                    raise RuntimeError("snap cancellation or equal split bounds differ")
                full = snap_key("Preview", "meta_l", "up")
                if full[:2] != left[:2] or full[2] != left[2] + right[2] or full[3] != left[3]:
                    raise RuntimeError("maximized preview differs from work area")
                snap_key("Applied", "ret")
                original = snap_key("Restored", "meta_l", "down")
                if original == left or original == full:
                    raise RuntimeError("snap restore lost original floating geometry")
                report["checks"].append("actual PS/2 Super-arrow snap preview, apply, cancel and restore preserve floating geometry and work-area bounds")
                report["checks"].append("Editor actual load, PS/2 editing, Ctrl-S journal save, external-change conflict and unsaved-text preservation")
                if args.persistence_phase:
                    persistent_path = "/native/persistent" if args.gpt_data_profile else "/persistent"
                    phases = [(f"write {persistent_path} survives-reboot", f"NEXT DATA: Wrote 15 bytes to {persistent_path}")] if args.persistence_phase == "create" else [(f"cat {persistent_path}", "NEXT DATA: survives-reboot"), (f"rm {persistent_path}", f"NEXT DATA: Removed {persistent_path}")]
                    for text, expected in phases:
                        start = len(serial_data)
                        serial.sendall((text + "\r").encode())
                        wait_text(expected,start)
                    report["checks"].append("persistence phase: " + args.persistence_phase)
                    if args.gpt_data_profile and args.persistence_phase == "create":
                        editor_command(f"edit {persistent_path}", f"PRISM EDITOR OPEN: path={persistent_path} bytes=15 exists=true")
                        editor_key("end")
                        editor_key("x")
                        mark = len(serial_data)
                        editor_key("ctrl", "s")
                        wait_text(f"PRISM EDITOR SAVE: path={persistent_path} bytes=16", mark)
                        report["checks"].append("actual GPT Editor save commits the persisted file before reboot")
                clock_samples = []
                for _ in range(2):
                    start = len(serial_data)
                    serial.sendall(b"uptime\r")
                    wait_text("source=PIT divisor=11931", start)
                    match = re.search(rb"NEXT CLOCK: ticks=(\d+) nanoseconds=(\d+) source=PIT divisor=11931", serial_data[start:])
                    if not match:
                        raise RuntimeError("invalid monotonic clock response")
                    clock_samples.append(tuple(map(int, match.groups())))
                    time.sleep(min(0.04, remaining()))
                if clock_samples[0][0] < 3 or clock_samples[1][0] <= clock_samples[0][0] or clock_samples[1][1] <= clock_samples[0][1]:
                    raise RuntimeError("IRQ clock failed to advance between console commands")
                report["clock_samples"] = clock_samples
                report["checks"].append("monotonic PIT clock advances while serial console remains responsive")
                start = len(serial_data)
                for key in ["s", "t", "a", "t", "u", "s", "ret"]:
                    qmp_command("send-key", {"keys": [{"type": "qcode", "data": key}], "hold-time": 40})
                    time.sleep(min(0.08, remaining()))
                wait_text(commands[0][1], start)
                report["checks"].append("PS/2 keyboard command through QMP")
                start = len(serial_data)
                for key in ["s", "t", "a", "t", "u", "z", "backspace", "s", "ret"]:
                    qmp_command("send-key", {"keys": [{"type": "qcode", "data": key}], "hold-time": 40})
                    time.sleep(min(0.08, remaining()))
                wait_text(commands[0][1], start)
                report["checks"].append("PS/2 backspace corrects statuz to status")
                start = len(serial_data)
                for key in ["f10", "tab", "ret"]:
                    qmp_command("send-key", {"keys": [{"type":"qcode","data":key}],"hold-time":40})
                    time.sleep(min(0.08,remaining()))
                wait_text("PRISM DESKTOP: focused=Some(Files) home=false",start)
                report["checks"].append("keyboard Home menu opens actual Files window without executing terminal input")
                start = len(serial_data)
                qmp_command("input-send-event", {"events":[{"type":"rel","data":{"axis":"x","value":20}},{"type":"rel","data":{"axis":"y","value":10}}]})
                time.sleep(min(0.08,remaining()))
                qmp_command("input-send-event", {"events":[{"type":"btn","data":{"down":True,"button":"left"}}]})
                wait_text("PRISM POINTER:",start)
                release_start=len(serial_data)
                qmp_command("input-send-event", {"events":[{"type":"btn","data":{"down":False,"button":"left"}}]})
                wait_text("buttons=0",release_start)
                release_end=serial_data.index(b"buttons=0",release_start)+len(b"buttons=0")
                wait_text("PRISM FRAME:",release_end)
                report["checks"].append("actual PS/2 mouse motion and left-button event reaches desktop hit testing")
                start = len(serial_data)
                serial.sendall(b"status\r")
                wait_text(commands[0][1],start)
                report["checks"].append("serial console restores Terminal focus from Files panel")
                response_end=serial_data.index(commands[0][1].encode(),start)+len(commands[0][1])
                wait_text("PRISM FRAME:",response_end)
                mark = len(serial_data)
                serial.sendall(b"uart\r")
                uart = wait_regex(rb"NEXT UART: accepted=(\d+) transmitted=(\d+) dropped=(\d+) stalled=(\d+) absent=(\d+) pending=(\d+)", mark)
                accepted, transmitted, dropped, stalled, absent, pending = map(int, uart.groups())
                if dropped != 0 or absent != 0 or accepted != transmitted + pending:
                    raise RuntimeError("UART rejected output, became absent or reported inconsistent queue metrics")
                report["uart_metrics"] = dict(zip(("accepted", "transmitted", "dropped", "stalled", "absent", "pending"), (accepted, transmitted, dropped, stalled, absent, pending)))
                wait_text("PRISM FRAME:", uart.end())
                report["checks"].append("bounded UART transmit FIFO preserves queued output across stalls and reports zero rejected/absent bytes")
                report["checks"].append("screenshot taken after completed software back-buffer presentation")
                qmp_command("screendump", {"filename": str(screenshot), "format": "ppm"})
                ppm = screenshot.read_bytes()
                header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", ppm)
                if not header:
                    raise RuntimeError("invalid framebuffer screenshot format")
                width, height = int(header[1]), int(header[2])
                pixels = ppm[header.end():]
                if width <= 0 or height <= 0 or len(pixels) != width * height * 3:
                    raise RuntimeError("invalid framebuffer screenshot dimensions")
                colors = {pixels[at:at + 3] for at in range(0, len(pixels), 3)}
                if len(colors) < 4:
                    raise RuntimeError("framebuffer screenshot is blank or lacks rendered UI")
                report["screenshot"] = {"path": "build/screenshot.ppm", "width": width, "height": height, "distinct_colors": len(colors),
                                        "validation": "PPM dimensions and at least four colors; no visual layout or accessibility verification"}
                report["checks"].append("nonblank framebuffer screenshot")
                if args.capture_prism_gallery:
                    gallery_dir = build / "prism-gallery"
                    gallery_dir.mkdir(parents=True, exist_ok=True)
                    gallery = {}

                    def gallery_key(*keys, expected=None):
                        mark = len(serial_data)
                        qmp_command("input-send-event", {"events": [
                            {"type": "key", "data": {"down": True,
                             "key": {"type": "qcode", "data": key}}}
                            for key in keys
                        ]})
                        time.sleep(min(0.08, remaining()))
                        qmp_command("input-send-event", {"events": [
                            {"type": "key", "data": {"down": False,
                             "key": {"type": "qcode", "data": key}}}
                            for key in reversed(keys)
                        ]})
                        if expected is None:
                            transition = wait_regex(rb"PRISM FRAME: generation=\d+", mark)
                        else:
                            transition = wait_regex(expected, mark)
                            wait_text("PRISM FRAME:", transition.end())
                        return transition

                    def capture_gallery(name):
                        path = gallery_dir / (name + ".ppm")
                        qmp_command("screendump", {"filename": str(path), "format": "ppm"})
                        data = path.read_bytes()
                        match = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
                        if not match:
                            raise RuntimeError(f"invalid Prism gallery screenshot: {name}")
                        shot_width, shot_height = int(match[1]), int(match[2])
                        if (shot_width, shot_height) != (width, height) or len(data[match.end():]) != shot_width * shot_height * 3:
                            raise RuntimeError(f"Prism gallery screenshot dimensions differ: {name}")
                        gallery[name] = {"path": str(path.relative_to(ROOT)),
                                         "width": shot_width, "height": shot_height}

                    # The main smoke leaves several built-in windows open. Close
                    # them one at a time so each published gallery frame has one
                    # frontmost panel instead of a stack of test windows.
                    focus_pattern = rb"PRISM DESKTOP: focused=(None|Some\([^\r\n]*?\)) home=false"
                    for _ in range(8):
                        closed = gallery_key("alt", "f4", expected=focus_pattern)
                        if closed[1] == b"None":
                            break
                    gallery_key("f10", expected=rb"PRISM DESKTOP: focused=None home=true")
                    capture_gallery("home")
                    gallery_key("esc", expected=rb"PRISM DESKTOP: focused=None home=false")
                    apps = ("terminal", "files", "monitor", "settings", "editor", "calculator")
                    for index, name in enumerate(apps):
                        gallery_key("f10", expected=rb"PRISM DESKTOP: focused=None home=true")
                        for _ in range(index):
                            gallery_key("tab", expected=rb"PRISM DESKTOP: focused=None home=true")
                        app_title = {"terminal": "Terminal", "files": "Files", "monitor": "Monitor",
                                     "settings": "Settings", "editor": "Editor", "calculator": "Calculator"}[name]
                        gallery_key("ret", expected=(rb"PRISM DESKTOP: focused=Some\(" + app_title.encode()
                                                       + rb"\) home=false"))
                        capture_gallery(name)
                        if name == "settings":
                            # Appearance is the initial category (index 2);
                            # Accessibility is index 10 in the catalog. Open
                            # its active focus-outline controls, choose Strong,
                            # and save through the same keyboard path a user
                            # takes. This records a real guest frame after the
                            # atomic preference commit completed.
                            for _ in range(8):
                                gallery_key("down")
                            gallery_key("ret")
                            gallery_key("right")
                            gallery_key("tab")
                            gallery_key("ret", expected=rb"PRISM APPEARANCE SAVE: request=\d+ .* result=Ok\(\(\)\) accepted=true")
                            capture_gallery("settings-accessibility")
                        for _ in range(8):
                            closed = gallery_key("alt", "f4", expected=focus_pattern)
                            if closed[1] == b"None":
                                break
                    gallery_key("f10", expected=rb"PRISM DESKTOP: focused=None home=true")
                    for _ in range(12):
                        gallery_key("tab", expected=rb"PRISM DESKTOP: focused=None home=true")
                    gallery_key("ret", expected=rb"PRISM DESKTOP: focused=None home=true")
                    capture_gallery("shortcuts")
                    gallery_key("esc")
                    gallery_key("esc", expected=rb"PRISM DESKTOP: focused=None home=false")
                    gallery_key("f8", expected=rb"PRISM DESKTOP: focused=None home=false")
                    capture_gallery("notifications")
                    gallery_key("f8", expected=rb"PRISM DESKTOP: focused=None home=false")
                    report["visual_gallery"] = gallery
                    report["checks"].append("actual QMP keyboard launched and captured completed guest frames for Home, six first-party panels, saved Accessibility settings, shortcut guide and notifications")
            if dns_peer is not None and not args.expect_boot_failure:
                boot_dropped = dns_peer.stats.get("boot_dropped", dns_peer.stats["dropped"])
                boot_answers = dns_peer.stats.get("boot_answers", dns_peer.stats["answers"])
                if boot_dropped != 1 or boot_answers < 3:
                    raise RuntimeError("DNS wire scenario did not prove retry after loss and TTL refresh")
                report["dns_peer"] = {"dropped": dns_peer.stats["dropped"], "answers": dns_peer.stats["answers"], "requests": dns_peer.stats["requests"]}
                report["dns_boot_flow"] = {"dropped": boot_dropped, "answers": boot_answers,
                                           "validation": "First valid bootstrap UDP source flow, independently scoped from native lookup source ports"}
            while select.select([serial], [], [], 0)[0]:
                receive_serial()
            remaining()
            current_digest = hashlib.sha256(image.read_bytes()).hexdigest()
            if args.persistent_data:
                if hashlib.sha256(image.read_bytes()[:64 * 1024 * 1024]).hexdigest() != protected_digest:
                    raise RuntimeError("persistent test modified the protected EFI volume")
                report["image_after_sha256"] = current_digest
                report["protected_esp_unchanged"] = True
            elif current_digest != report["image_sha256"]:
                raise RuntimeError("development image changed during smoke; rebuild and rerun")
            remaining()
            report["passed"] = True
    except KeyboardInterrupt:
        interrupted = True
        report["passed"] = False
        report["error"] = "boot smoke interrupted"
    except (OSError, RuntimeError, TimeoutError, ValueError) as error:
        report["error"] = str(error)
    finally:
        if udp_peer is not None:
            udp_peer.close()
        if dns_peer is not None:
            dns_peer.close()
        if tcp_peer is not None:
            tcp_peer.close()
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        for channel in [qmp_stream, qmp, serial]:
            if channel is not None:
                channel.close()
        if report["passed"] and not args.expect_boot_failure:
            try:
                with wave.open(str(build / "audio.wav"), "rb") as pcm:
                    if pcm.getnchannels()!=2 or pcm.getsampwidth()!=2 or pcm.getframerate()!=48000:
                        raise RuntimeError("AC97 capture has wrong PCM format")
                    samples=struct.unpack('<'+'h'*(pcm.getnframes()*2),pcm.readframes(pcm.getnframes()))
                active=[index for index in range(0,len(samples),2) if samples[index] or samples[index+1]]
                if len(active)<10000 or any(samples[index]!=samples[index+1] for index in active):
                    raise RuntimeError("AC97 stereo playback missing or corrupted")
                start=active[0]
                # The generated 1 kHz triangle repeats every 48 stereo frames.
                if any(samples[index]!=samples[index+96] for index in active[:9000]):
                    raise RuntimeError("AC97 waveform does not retain the expected 1 kHz period")
                report["audio"]={"path":str((build/'audio.wav').relative_to(ROOT)),"sample_rate":48000,"channels":2,"active_frames":len(active)}
                report["checks"].append("captured AC97 PCM contains matching stereo channels and a 1 kHz waveform")
            except (OSError,EOFError,wave.Error,RuntimeError,struct.error,IndexError) as error:
                report["passed"]=False
                report["error"]=str(error)
        (build / "serial.log").write_bytes(serial_data)
        report["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (build / "boot-smoke.json").write_text(json.dumps(report, indent=2) + "\n")
        signal.signal(signal.SIGTERM, previous_term)
    print(json.dumps(report, indent=2))
    return 130 if interrupted else (0 if report["passed"] else 1)


if __name__ == "__main__":
    sys.exit(main())
