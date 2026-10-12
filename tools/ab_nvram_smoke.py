#!/usr/bin/env python3
"""Exercise OVMF A/B trial attempts across separate QEMU process boots."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BOOT = b"NEXT BOOT A/B READY slot="
INITRD = b"NEXT BOOT: INITRD LOAD PASS"
EXPECTED = [
    (b"slot=B mode=Trial sequence=3 attempts_left=2", b"state=initialized"),
    (b"slot=B mode=Trial sequence=4 attempts_left=1", b"state=persistent"),
    (b"slot=B mode=Trial sequence=5 attempts_left=0", b"state=persistent"),
    (b"slot=A mode=Rollback sequence=6 attempts_left=0", b"state=persistent"),
    (b"slot=A mode=Confirmed sequence=6 attempts_left=0", b"state=persistent"),
]


def recv_json(stream):
    while True:
        line = stream.readline()
        if not line:
            raise RuntimeError("QMP closed before replying")
        item = json.loads(line)
        if isinstance(item, dict):
            return item


def qmp_command(sock, stream, serial, name):
    identifier = 1
    sock.sendall(json.dumps({"execute": name, "id": identifier}).encode() + b"\n")
    while True:
        item = recv_json(stream)
        if item.get("id") == identifier:
            if "error" in item:
                raise RuntimeError(f"QMP {name} failed: {item['error']}")
            return item.get("return")


def wait_socket(path, process, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"QEMU exited early with status {process.returncode}")
        try:
            channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            channel.connect(str(path))
            return channel
        except (FileNotFoundError, ConnectionRefusedError):
            channel.close()
            time.sleep(0.05)
    raise TimeoutError(f"QEMU socket did not appear: {path}")


def run_one(qemu, firmware, variables, image, work, index, env, timeout=75):
    temp = Path(tempfile.mkdtemp(prefix=f"ab-boot-{index}-", dir=work))
    serial_path, qmp_path = temp / "serial.sock", temp / "qmp.sock"
    log_path, qemu_log = temp / "serial.log", temp / "qemu.log"
    command = [
        qemu,
        "-machine", "q35,accel=tcg",
        "-cpu", "qemu64",
        "-m", "256M",
        "-smp", "1",
        "-vga", "std",
        "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE_4M.fd'}",
        "-drive", f"if=pflash,format=raw,file={variables}",
        "-drive", f"if=none,id=nextdisk,format=raw,snapshot=on,file={image}",
        "-device", "virtio-blk-pci,drive=nextdisk,disable-modern=on",
        "-display", "none",
        "-serial", f"unix:{serial_path},server=on,wait=off",
        "-qmp", f"unix:{qmp_path},server=on,wait=off",
        "-no-reboot",
    ]
    sysroot = Path(env.get("NEXT_SYSROOT", "/workspace/toolchains/sysroot"))
    for data in ("usr/share/qemu", "usr/share/seabios", "usr/lib/ipxe/qemu"):
        if (sysroot / data).exists():
            command.extend(["-L", str(sysroot / data)])
    with qemu_log.open("wb") as stderr:
        process = subprocess.Popen(command, stdout=stderr, stderr=subprocess.STDOUT, env=env)
    serial = qmp = qmp_stream = None
    captured = bytearray()
    try:
        serial = wait_socket(serial_path, process, timeout)
        qmp = wait_socket(qmp_path, process, timeout)
        qmp_stream = qmp.makefile("rb")
        if "QMP" not in recv_json(qmp_stream):
            raise RuntimeError("QEMU returned an invalid QMP greeting")
        qmp_command(qmp, qmp_stream, serial, "qmp_capabilities")
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            serial.settimeout(0.25)
            try:
                chunk = serial.recv(65536)
                if not chunk:
                    raise RuntimeError("QEMU closed serial before A/B marker")
                captured.extend(chunk)
            except TimeoutError:
                pass
            log_path.write_bytes(captured)
            ready = captured.splitlines()
            ready = [line for line in ready if BOOT in line]
            loaded = captured.count(INITRD)
            if ready and loaded >= len(ready):
                match = ready[-1]
                need = EXPECTED[index]
                if not all(part in match for part in need):
                    raise RuntimeError(
                        f"boot {index + 1} chose unexpected state: {match.decode(errors='replace')}"
                    )
                break
            if process.poll() is not None:
                raise RuntimeError(
                    f"QEMU exited before loading selected slot; see {log_path} and {qemu_log}"
                )
        else:
            raise TimeoutError(f"boot {index + 1} did not load the selected A/B slot")
        qmp_command(qmp, qmp_stream, serial, "quit")
        process.wait(timeout=10)
        return {
            "boot": index + 1,
            "serial_log": str(log_path.relative_to(ROOT)),
            "ready_line": match.decode(errors="replace"),
            "selected_slot_artifacts_loaded": True,
            "qemu_exit": process.returncode,
        }
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if serial is not None:
            serial.close()
        if qmp_stream is not None:
            qmp_stream.close()
        if qmp is not None:
            qmp.close()


def main():
    work = ROOT / "work"
    build = ROOT / "build"
    work.mkdir(exist_ok=True)
    build.mkdir(exist_ok=True)
    env = os.environ.copy()
    env["NEXT_DEV_PUBLIC_KEY"] = str(Path(env.get("NEXT_DEV_PUBLIC_KEY", work / "package-dev.pub")))
    if not Path(env["NEXT_DEV_PUBLIC_KEY"]).is_file():
        raise SystemExit("Missing development public key; run tools/build.sh first")
    if not (build / "next-dev.img").is_file():
        raise SystemExit("Missing build/next-dev.img; run tools/build.sh first")
    sysroot = Path(env.get("NEXT_SYSROOT", "/workspace/toolchains/sysroot"))
    firmware = sysroot / "usr/share/OVMF"
    if not firmware.exists():
        firmware = Path("/usr/share/OVMF")
    qemu_path = sysroot / "usr/bin/qemu-system-x86_64"
    qemu = str(qemu_path) if qemu_path.exists() else shutil.which("qemu-system-x86_64")
    if not qemu:
        raise SystemExit("qemu-system-x86_64 unavailable; source tools/env.sh")
    env["LD_LIBRARY_PATH"] = str(sysroot / "usr/lib/x86_64-linux-gnu") + (
        ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else ""
    )
    cargo = shutil.which("cargo")
    mcopy = shutil.which("mcopy")
    if not cargo or not mcopy:
        raise SystemExit("cargo and mcopy are required; source tools/env.sh")
    build_command = [
        cargo, "build", "--locked", "--offline", "--release", "-p", "next-boot",
        "--features", "ab-smoke", "--target", "x86_64-unknown-uefi",
    ]
    subprocess.run(build_command, cwd=ROOT, env=env, check=True)
    smoke_image = work / "ab-nvram-smoke.img"
    shutil.copyfile(build / "next-dev.img", smoke_image)
    test_loader = ROOT / "target/x86_64-unknown-uefi/release/next-boot.efi"
    subprocess.run(
        [mcopy, "-o", "-i", str(smoke_image), str(test_loader), "::/EFI/BOOT/BOOTX64.EFI"],
        cwd=ROOT,
        check=True,
    )
    variables = build / "ab-nvram-vars.fd"
    shutil.copyfile(firmware / "OVMF_VARS_4M.fd", variables)
    runs = [run_one(qemu, firmware, variables, smoke_image, build, i, env) for i in range(5)]
    report = {
        "passed": True,
        "scope": (
            "Five independent QEMU process boots shared one OVMF variable-store file. "
            "Readback-checked EFI NV+BS+RT variables selected signed slot B for three "
            "trial starts, then rolled back to confirmed slot A; each selected kernel "
            "and initrd loaded. Smoke-only build feature staged B. No protected anti-rollback anchor."
        ),
        "image_sha256": hashlib.sha256(smoke_image.read_bytes()).hexdigest(),
        "variables_sha256": hashlib.sha256(variables.read_bytes()).hexdigest(),
        "boots": runs,
    }
    (build / "ab-nvram-smoke.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
