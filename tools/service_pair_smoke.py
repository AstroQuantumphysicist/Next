#!/usr/bin/env python3
"""Build a disposable signed two-worker configuration and prove real guest ordering.

The default image is read-only input. The alternate keeps the exact authenticated
kernel and executable payloads, changing only the signed nextd configuration.
This narrow gate does not substitute for the default full desktop smoke suite.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import zlib
from initrd import pack
from udp_responder import Responder as UdpResponder
from dns_responder import Responder as DnsResponder
from tcp_responder import Responder as TcpResponder

ROOT = Path(__file__).resolve().parents[1]


def package_files(bundle):
    """Caller must first authenticate the complete bundle using next-package."""
    files = {}
    for index in range(int.from_bytes(bundle[32:36], 'little')):
        at = 256 + 128 * index
        path = bundle[at:at+64].split(b'\0')[0].decode('ascii')
        offset, size = struct.unpack_from('<QQ', bundle, at+64)
        data = bundle[offset:offset+size]
        if len(data) != size or hashlib.sha256(data).digest() != bundle[at+80:at+112]:
            raise ValueError('authenticated member bounds/digest mismatch')
        files[path] = data
    return files


def archive_files(archive):
    if archive[:8] != b'NEXTINIT' or len(archive) < 32:
        raise ValueError('invalid authenticated archive')
    count = int.from_bytes(archive[12:16], 'little')
    if not 1 <= count <= 64:
        raise ValueError('archive count')
    files = {}
    for index in range(count):
        at = 32 + 80 * index
        path = archive[at:at+64].split(b'\0')[0].decode('ascii')
        offset, size = struct.unpack_from('<QQ', archive, at+64)
        data = archive[offset:offset+size]
        if len(data) != size:
            raise ValueError('archive bounds')
        files[path] = data
    return files


def prepare(image, directory, env):
    if image.resolve() == (directory/'next-service-pair.img').resolve():
        raise ValueError('input image must differ from disposable alternate output')
    tool = ROOT/'target/debug/next-package'
    public = Path(env.get('NEXT_DEV_PUBLIC_KEY', str(ROOT/'work/package-dev.pub')))
    seed = Path(env.get('NEXT_SIGNING_SEED', str(ROOT/'work/package-dev.seed')))
    def run(args):
        subprocess.run([str(item) for item in args], check=True, env=env, capture_output=True)
    with tempfile.TemporaryDirectory(prefix='pair-stage-', dir=ROOT/'work') as stage_name:
        stage = Path(stage_name)
        deployment = stage/'base.nxp'
        run(['mcopy', '-i', image, '::/EFI/NEXT/SYSTEM.NXP', deployment])
        run([tool, 'verify', public, deployment])
        base = package_files(deployment.read_bytes())
        entries = archive_files(base['boot/initrd.nxi'])
        supervisor = stage/'supervisor.nxp'
        supervisor.write_bytes(entries['/system/nextd.elf'])
        run([tool, 'verify', public, supervisor])
        members = package_files(supervisor.read_bytes())
        config = bytearray(members['config/services.bin'])
        if len(config) != 544 or config[:8] != b'NXSVC001' or config[24] != 1:
            raise ValueError('base must carry the default authenticated two-record mask1 config')
        config[24] = 3
        config[20:24] = b'\0'*4
        struct.pack_into('<I', config, 20, zlib.crc32(config))
        (stage/'services.bin').write_bytes(config)
        (stage/'nextd.elf').write_bytes(members['bin/nextd.elf'])
        alternate_app = stage/'nextd.nxp'
        run([tool, 'pack', seed, 'org.next.development.native.nextd', '0.1.0', '3f', 'app',
             'bin/nextd.elf', alternate_app, f'bin/nextd.elf={stage}/nextd.elf',
             f'config/services.bin={stage}/services.bin'])
        run([tool, 'verify', public, alternate_app])
        entries['/system/nextd.elf'] = alternate_app.read_bytes()
        initrd = stage/'INITRD.NXI'
        initrd.write_bytes(pack(entries.items()))
        kernel = stage/'helix.elf'
        kernel.write_bytes(base['boot/helix.elf'])
        alternate_deployment = stage/'SYSTEM.NXP'
        run([tool, 'pack', seed, 'org.next.development', '0.1.0', '0', 'deployment',
             'boot/helix.elf', alternate_deployment, f'boot/helix.elf={kernel}',
             f'boot/initrd.nxi={initrd}'])
        run([tool, 'verify', public, alternate_deployment])
        alternate_image = stage/'next-service-pair.img'
        shutil.copyfile(image, alternate_image)
        run(['mcopy', '-o', '-i', alternate_image, alternate_deployment, '::/EFI/NEXT/SYSTEM.NXP'])
        run(['mcopy', '-o', '-i', alternate_image, initrd, '::/EFI/NEXT/INITRD.NXI'])
        destination = directory/'next-service-pair.img'
        alternate_image.replace(destination)
        return destination, {'base_image_sha256': hashlib.sha256(image.read_bytes()).hexdigest(),
                             'image_sha256': hashlib.sha256(destination.read_bytes()).hexdigest(),
                             'kernel_sha256': hashlib.sha256(base['boot/helix.elf']).hexdigest(),
                             'nextd_payload_sha256': hashlib.sha256(members['bin/nextd.elf']).hexdigest(),
                             'configuration_sha256': hashlib.sha256(config).hexdigest(),
                             'active_mask': 3}


def verify_trace(data):
    events = list(re.finditer(rb'NEXTD SERVICE: index=(\d+) name=([a-z-]+) phase=([a-z-]+) instance=(\d+) dependencies=(\d+) ceiling=0x([0-9a-f]+)\r?\n', data))
    starts = [event for event in events if event[3] == b'started']
    first = {index: next(event for event in starts if int(event[1]) == index) for index in (0, 1)}
    ready0 = next(event for event in events if event[1] == b'0' and event[3] == b'ready')
    ready1 = next(event for event in events if event[1] == b'1' and event[3] == b'ready')
    if not first[0].start() < ready0.start() < first[1].start() < ready1.start():
        raise ValueError('logs do not prove dependency ready before dependent launch')
    if any(int(event[6], 16) != 0x1b for event in events) or first[1][5] != b'1':
        raise ValueError('service grant/dependency mismatch')
    pids = {}
    for event in starts:
        preceding = list(re.finditer(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/service-worker\.elf profile=Native', data[:event.start()]))
        if not preceding:
            raise ValueError('service started without actual process creation')
        pids[(int(event[1]), int(event[4]))] = int(preceding[-1][1])
    if len(set(pids.values())) != len(starts):
        raise ValueError('service starts reused a PID or claimed a shared instance')
    for index in (0, 1):
        if sum(int(event[1]) == index for event in starts) < 2:
            raise ValueError('both actual isolated services must restart')
    initial_primary = pids[(0, int(first[0][4]))]
    exit0 = re.search(rb'HELIX: PROCESS EXIT pid=' + str(initial_primary).encode() + rb' status=81 preemptions=\d+\r?\n', data)
    stop1 = next(event for event in events if event[1] == b'1' and event[3] == b'watchdog-or-dependency-stop')
    if not exit0 or not ready1.start() < exit0.start() < stop1.start():
        raise ValueError('dependency loss did not stop the actual dependent process')
    final = {}
    for index in (0, 1):
        ready = [event for event in events if int(event[1]) == index and event[3] == b'ready'][-1]
        pid = pids[(index, int(ready[4]))]
        if re.search(rb'HELIX: PROCESS EXIT pid=' + str(pid).encode() + rb' status=', data[ready.end():]):
            raise ValueError('final service exited after readiness')
        final[index] = pid
    return final, ['authenticated alternate mask3 boots exact default kernel and executable payloads',
                   'primary ready precedes dependent process creation and readiness',
                   'both services have distinct process PIDs across actual restarts',
                   'primary exit stops dependent before dependency recovery',
                   'both final services remain alive alongside PID1']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, default=ROOT/'build/next-dev.img')
    parser.add_argument('--output-dir', type=Path, default=ROOT/'work/service-pair')
    parser.add_argument('--cpu', choices=['qemu64', 'max'], default='qemu64')
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--prepare-only', action='store_true')
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error('timeout must be finite and positive')
    directory = args.output_dir.resolve()
    if not directory.is_relative_to(ROOT/'work'):
        parser.error('disposable alternate output must be below work/')
    directory.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    image, report = prepare(args.image.resolve(), directory, env)
    report.update(passed=False, scope='signed two-worker alternate configuration; q35 single-CPU development profile', checks=[])
    if args.prepare_only:
        (directory/'service-pair.json').write_text(json.dumps(report, indent=2)+'\n')
        print(f'Prepared {image}; guest execution not performed')
        return
    sysroot = Path(env.get('NEXT_SYSROOT', '/workspace/toolchains/sysroot'))
    qemu = sysroot/'usr/bin/qemu-system-x86_64'
    if not qemu.exists():
        qemu = Path(shutil.which('qemu-system-x86_64') or '')
    if (sysroot/'usr/lib/x86_64-linux-gnu').exists():
        env['LD_LIBRARY_PATH'] = str(sysroot/'usr/lib/x86_64-linux-gnu') + ':' + env.get('LD_LIBRARY_PATH', '')
    firmware = sysroot/'usr/share/OVMF'
    if not firmware.exists(): firmware = Path('/usr/share/OVMF')
    data = bytearray()
    process = None
    peers = []
    deadline = time.monotonic() + args.timeout
    try:
        peers = [UdpResponder(), DnsResponder(39099, drop_first=True), TcpResponder()]
        with tempfile.TemporaryDirectory(prefix='pair-vm-', dir=ROOT/'work') as temp_name:
            temp = Path(temp_name)
            shutil.copyfile(firmware/'OVMF_VARS_4M.fd', temp/'vars.fd')
            command = [str(qemu), '-machine', 'q35,accel=tcg', '-cpu', args.cpu, '-m', '256M', '-smp', '1', '-vga', 'std',
                       '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}/OVMF_CODE_4M.fd',
                       '-drive', f'if=pflash,format=raw,file={temp}/vars.fd',
                       '-drive', f'if=none,id=nextdisk,format=raw,snapshot=on,file={image}',
                       '-device', 'virtio-blk-pci,drive=nextdisk,disable-modern=on',
                       '-netdev', 'user,id=nextnet', '-device', 'virtio-net-pci,netdev=nextnet,disable-modern=on',
                       '-audiodev', f'wav,id=nextaudio,path={directory}/audio.wav,out.frequency=48000,out.channels=2,out.format=s16',
                       '-device', 'AC97,audiodev=nextaudio', '-display', 'none', '-serial', f'unix:{temp}/serial.sock,server=on,wait=off', '-no-reboot']
            for path in ['usr/share/qemu', 'usr/share/seabios', 'usr/lib/ipxe/qemu']:
                if (sysroot/path).exists(): command.extend(['-L', str(sysroot/path)])
            with (directory/'qemu.log').open('wb') as log:
                process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env)
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as serial:
                    while True:
                        try: serial.connect(str(temp/'serial.sock')); break
                        except (FileNotFoundError, ConnectionRefusedError):
                            if time.monotonic() >= deadline or process.poll() is not None: raise TimeoutError('QEMU serial unavailable')
                            time.sleep(0.05)
                    def wait(marker, start=0):
                        while not (marker.search(data, start) if hasattr(marker, 'search') else marker in data[start:]):
                            if time.monotonic() >= deadline or process.poll() is not None: raise TimeoutError('two-worker guest gate timeout')
                            if select.select([serial], [], [], 0.1)[0]:
                                chunk = serial.recv(65536)
                                if not chunk: raise RuntimeError('serial disconnected')
                                data.extend(chunk)
                                (directory/'serial.log').write_bytes(data)
                                if any(item in data for item in [b'HELIX FATAL:', b'HELIX PANIC:', b'NEXT BOOT FAILED:', b'NATIVE STARTUP FAILED']):
                                    raise RuntimeError('guest startup failed')
                                if re.search(rb'HELIX: PROCESS EXIT pid=1 status=', data): raise RuntimeError('PID1 exited')
                    wait(b'NEXTD: signed two-service dependency readiness and isolated recovery passed')
                    wait(b'PRISM: READY')
                    start = len(data)
                    serial.sendall(b'ps\r')
                    final, checks = verify_trace(data)
                    for pid in [1, *final.values()]:
                        wait(re.compile(rb'NEXT PROCESS: pid=' + str(pid).encode() + rb' state=(runnable|sleeping)'), start)
                        if not re.search(rb'NEXT PROCESS: pid=' + str(pid).encode() + rb' state=(runnable|sleeping)', data[start:]):
                            raise ValueError(f'console did not report live process {pid}')
                    wait(b'PRISM FRAME:', start)
                    # Recheck after the console loop actually scheduled them.
                    final, checks = verify_trace(data)
                    report.update(passed=True, checks=checks, final_worker_pids=final, cpu=args.cpu)
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        if process is not None:
            process.terminate()
            try: process.wait(timeout=3)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
        for peer in peers: peer.close()
        (directory/'serial.log').write_bytes(data)
        (directory/'service-pair.json').write_text(json.dumps(report, indent=2)+'\n')
    print(f'Signed two-service guest gate PASS ({len(report["checks"])} checks)')


if __name__ == '__main__':
    main()
