#!/usr/bin/env python3
"""Exercise bounded process exhaustion and negative runtime behavior in QEMU."""
import argparse
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
import tempfile
import time
from udp_responder import Responder

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, default=ROOT / 'build/next-dev.img')
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'build/runtime-failures')
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--cpu', choices=['qemu64', 'max'], default='qemu64')
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error('timeout must be finite and positive')
    image = args.image.resolve()
    out = args.output_dir.resolve()
    if not out.is_relative_to(ROOT / 'build'):
        parser.error('output must be below build/')
    if not image.is_file():
        parser.error('image does not exist')
    out.mkdir(parents=True, exist_ok=True)
    before = hashlib.sha256(image.read_bytes()).hexdigest()
    report = {'passed': False, 'image_sha256': before, 'checks': [],
              'scope': 'Single-CPU QEMU bounded process-slot exhaustion/reuse, non-yielding preemption, stale PID denial and user-fault survival; no SMP, resource-wide exhaustion or hardware certification.'}
    (out / 'runtime-failures.json').write_text(json.dumps(report, indent=2) + '\n')
    (ROOT / 'work').mkdir(exist_ok=True)
    env = os.environ.copy()
    sysroot = Path(env.get('NEXT_SYSROOT', '/workspace/toolchains/sysroot'))
    local = sysroot / 'usr/bin/qemu-system-x86_64'
    qemu = str(local) if local.exists() else shutil.which('qemu-system-x86_64')
    if local.exists():
        libs = str(sysroot / 'usr/lib/x86_64-linux-gnu')
        env['LD_LIBRARY_PATH'] = libs + (':' + env['LD_LIBRARY_PATH'] if env.get('LD_LIBRARY_PATH') else '')
    firmware = sysroot / 'usr/share/OVMF'
    if not firmware.exists():
        firmware = Path('/usr/share/OVMF')
    process = None
    channel = None
    peer = None
    transcript = bytearray()
    deadline = time.monotonic() + args.timeout

    def remaining():
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError('runtime negative test deadline exceeded')
        if process is not None and process.poll() is not None:
            raise RuntimeError('QEMU exited prematurely')
        return left

    def wait(pattern, start=0):
        pattern = re.compile(pattern)
        while True:
            match = pattern.search(transcript, start)
            if match:
                return match
            ready, _, _ = select.select([channel], [], [], min(0.1, remaining()))
            if ready:
                chunk = channel.recv(65536)
                if not chunk:
                    raise RuntimeError('serial channel closed')
                transcript.extend(chunk)
                (out / 'serial.log').write_bytes(transcript)
                if any(marker in transcript for marker in [b'HELIX PANIC:', b'HELIX FATAL:', b'NEXT BOOT FAILED:', b'HELIX: NATIVE STARTUP FAILED']):
                    raise RuntimeError('guest fatal/startup error')

    def command(text, pattern):
        start = len(transcript)
        channel.sendall((text + '\r').encode('ascii'))
        return wait(pattern, start)

    def stop(_signum, _frame):
        raise KeyboardInterrupt('termination requested')

    old_term = signal.signal(signal.SIGTERM, stop)
    try:
        if not qemu:
            raise RuntimeError('QEMU unavailable; source tools/env.sh')
        peer = Responder()
        (out / 'serial.log').write_bytes(b'')
        with tempfile.TemporaryDirectory(prefix='runtime-', dir=ROOT / 'work') as tmp, (out / 'qemu.log').open('wb') as log:
            tmp = Path(tmp)
            shutil.copyfile(firmware / 'OVMF_VARS_4M.fd', tmp / 'vars.fd')
            env['TMPDIR'] = str(tmp)
            cmd = [qemu, '-machine', 'q35,accel=tcg', '-cpu', args.cpu, '-m', '256M', '-smp', '1', '-vga', 'std',
                   '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE_4M.fd"}',
                   '-drive', f'if=pflash,format=raw,file={tmp / "vars.fd"}',
                   '-drive', f'if=none,id=nextdisk,format=raw,snapshot=on,file={image}',
                   '-device', 'virtio-blk-pci,drive=nextdisk,disable-modern=on',
                   '-netdev', 'user,id=nextnet', '-device', 'virtio-net-pci,netdev=nextnet,disable-modern=on',
                   '-audiodev', f'wav,id=nextaudio,path={out / "audio.wav"},out.frequency=48000,out.channels=2,out.format=s16',
                   '-device', 'AC97,audiodev=nextaudio', '-display', 'none',
                   '-serial', f'unix:{tmp / "serial.sock"},server=on,wait=off', '-no-reboot']
            for data in ['usr/share/qemu', 'usr/share/seabios', 'usr/lib/ipxe/qemu']:
                if (sysroot / data).exists():
                    cmd.extend(['-L', str(sysroot / data)])
            process = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            while channel is None:
                remaining()
                candidate = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                try:
                    candidate.connect(str(tmp / 'serial.sock'))
                    channel = candidate
                except (FileNotFoundError, ConnectionRefusedError):
                    candidate.close()
                    time.sleep(min(0.05, remaining()))
            wait(rb'HELIX: NATIVE SYSTEM READY')
            wait(rb'PRISM: READY')
            wait(rb'NEXTD: service crash restart, heartbeat watchdog recovery and PID1 survival passed')
            service_spawns = list(re.finditer(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/service-worker\.elf profile=Native', transcript))
            if len(service_spawns) < 3:
                raise RuntimeError('persistent service recovery was not established')
            worker_pid = int(service_spawns[-1][1])
            report['persistent_worker_pid'] = worker_pid
            command('ps', f'NEXT PROCESS: pid={worker_pid} state='.encode())
            command(f'kill {worker_pid}', rb'NEXT KILL FAILED: AccessDenied')
            report['checks'].append('persistent isolated worker is live and console cannot kill supervisor-owned service')
            pids = []
            for _ in range(2):
                match = command('run /system/spin.elf', rb'NEXT RUN: pid=(\d+)\r?\n')
                pid = int(match[1])
                if pid <= 1 or pid in pids:
                    raise RuntimeError('spawn reused a live/supervisor PID')
                pids.append(pid)
            command('run /system/spin.elf', rb'NEXT RUN FAILED: NoMemory')
            report['checks'].append('PID1 and persistent worker plus two console children fill four process slots; further spawn returns NoMemory')
            command('status', rb'NEXT: kernel-control=independent')
            command('uptime', rb'NEXT CLOCK: ticks=\d+ nanoseconds=\d+')
            command('ps', rb'NEXT PROCESS: pid=1 state=')
            command('kill 1', rb'NEXT KILL FAILED: AccessDenied')
            report['checks'].append('console, supervisor and persistent service remain responsive while two non-yielding executables run')
            for pid in pids:
                command(f'kill {pid}', f'NEXT KILL: pid={pid}'.encode())
                command(f'kill {pid}', rb'NEXT KILL FAILED: NotFound')
            report['checks'].append('kill frees slots and stale numeric PIDs cannot address dead processes')
            new = int(command('run /system/spin.elf', rb'NEXT RUN: pid=(\d+)\r?\n')[1])
            if new <= max(pids):
                raise RuntimeError('reused slot did not get a fresh monotonic PID')
            command(f'kill {new}', f'NEXT KILL: pid={new}'.encode())
            report['checks'].append('slot reuse allocates a fresh PID after exhaustion recovery')
            command('run /system/fault.elf', rb'HELIX: PROCESS FAULT pid=\d+ vector=14[^\r\n]*contained')
            command('status', rb'NEXT: kernel-control=independent')
            command('run /system/missing.elf', rb'NEXT RUN FAILED: NotFound')
            report['checks'].append('fresh user page fault is contained; console remains usable and missing executable rejects')
            report['passed'] = True
    except (Exception, KeyboardInterrupt) as error:
        report['error'] = str(error)
    finally:
        if process is not None:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
            else:
                process.wait()
        if channel is not None:
            channel.close()
        if peer is not None:
            peer.close()
        signal.signal(signal.SIGTERM, old_term)
        report['source_image_unchanged'] = hashlib.sha256(image.read_bytes()).hexdigest() == before
        if not report['source_image_unchanged']:
            report['passed'] = False
            report['error'] = 'source image unexpectedly changed'
        (out / 'runtime-failures.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
