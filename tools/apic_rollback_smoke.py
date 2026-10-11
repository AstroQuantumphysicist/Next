#!/usr/bin/env python3
"""Boot an explicit signed diagnostic image and prove real APIC-to-PIC rollback.

The caller builds/signs the alternate kernel with NEXT_DEV_APIC_FAILURE_STAGE=
after-unmask. This runner never builds or edits an image. Snapshot writes remain
private to QEMU; both the alternate and default image hashes must stay unchanged.
This is a narrow fault gate, not the default full desktop certification suite.
"""
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
import socket
import subprocess
import tempfile
import time
from boot_smoke import ROOT, Responder, DnsResponder, TcpResponder
from service_pair_smoke import package_files


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify_routing_report(data):
    lines = re.findall(rb'HELIX IOAPIC IRQ RESULT Report \{[^\r\n]+\}\s*; PIT remains time source', data)
    if len(lines) != 1:
        raise ValueError('expected exactly one authoritative IRQ routing report')
    line = lines[0]
    def field(name):
        match = re.search(name.encode() + rb': (\d+|true|false)(?:,| \})', line)
        if not match:
            raise ValueError('missing routing field: ' + name)
        return match[1].decode('ascii')
    result = {name: int(field(name)) for name in ('gsi', 'apic_interrupts', 'pic_interrupts', 'rollback_pic_interrupts')}
    result.update({name: field(name) == 'true' for name in ('injected', 'restored_controls', 'owner_retained')})
    if (result['gsi'] != 2 or result['apic_interrupts'] < 1 or result['rollback_pic_interrupts'] < 3
            or not result['injected'] or not result['restored_controls'] or result['owner_retained']
            or b'result: Err(Hardware)' not in line):
        raise ValueError('report does not prove injected APIC delivery, restoration and three fresh PIC IRQs')
    result['result'] = 'Err(Hardware)'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, required=True, help='explicit alternate signed diagnostic image; read-only input')
    parser.add_argument('--output-dir', type=Path, default=ROOT/'build/apic-rollback')
    parser.add_argument('--cpu', choices=('qemu64', 'max'), default='qemu64')
    parser.add_argument('--timeout', type=float, default=60)
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or not 0 < args.timeout <= 300:
        parser.error('timeout must be positive, finite and <=300 seconds')
    image = args.image.resolve()
    default = (ROOT/'build/next-dev.img').resolve()
    if image == default:
        parser.error('requires an explicit alternate diagnostic image, not the default image')
    output = args.output_dir.resolve()
    if not output.is_relative_to(ROOT/'build'):
        parser.error('output directory must be below build/')
    if not image.is_file():
        parser.error('alternate image does not exist')
    output.mkdir(parents=True, exist_ok=True)
    for name in ('serial.log', 'qemu.log', 'audio.wav', 'screenshot.ppm', 'apic-rollback.json'):
        artifact = output/name
        if artifact.is_symlink() or (artifact.exists() and (
                os.path.samefile(artifact, image)
                or (default.exists() and os.path.samefile(artifact, default)))):
            parser.error('output artifact aliases an input image or is a symlink: ' + name)
    serial_log = output/'serial.log'
    serial_log.write_bytes(b'')
    report = {'passed': False, 'scope': 'q35/TCG BSP PIT IRQ0 after-unmask injection; actual ISR-observed APIC delivery, restored controls and PIC fallback plus PS/2 desktop action. No SMP or hardware certification.',
              'started_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'image': str(image), 'image_sha256': digest(image),
              'default_image_sha256': digest(default) if default.exists() else None, 'checks': []}
    sysroot = Path(os.environ.get('NEXT_SYSROOT', '/workspace/toolchains/sysroot'))
    env = os.environ.copy()
    qemu = sysroot/'usr/bin/qemu-system-x86_64'
    if qemu.exists():
        env['LD_LIBRARY_PATH'] = str(sysroot/'usr/lib/x86_64-linux-gnu') + ':' + env.get('LD_LIBRARY_PATH', '')
    else:
        binary = shutil.which('qemu-system-x86_64')
        if not binary:
            parser.error('QEMU missing; source tools/env.sh')
        qemu = Path(binary)
    firmware = sysroot/'usr/share/OVMF'
    if not firmware.exists():
        firmware = Path('/usr/share/OVMF')
    process = None
    channels = []
    peers = []
    serial_data = bytearray()
    deadline = time.monotonic() + args.timeout
    def remaining():
        value = deadline - time.monotonic()
        if value <= 0:
            raise TimeoutError('bounded APIC rollback VM deadline')
        if process is not None and process.poll() is not None:
            raise RuntimeError('QEMU exited before completing proof')
        return value
    def connect(path):
        while True:
            channel = socket.socket(socket.AF_UNIX)
            try:
                channel.connect(str(path))
                channels.append(channel)
                return channel
            except (FileNotFoundError, ConnectionRefusedError):
                channel.close()
                time.sleep(min(.03, remaining()))
    def consume():
        if select.select([serial], [], [], min(.1, remaining()))[0]:
            chunk = serial.recv(65536)
            if not chunk:
                raise RuntimeError('serial channel closed')
            serial_data.extend(chunk)
            serial_log.write_bytes(serial_data)
        if len(serial_data) > 8 << 20:
            raise RuntimeError('serial evidence exceeded bounded 8MiB')
        for marker in (b'HELIX PANIC:', b'HELIX FATAL:', b'NEXT BOOT FAILED:', b'HELIX IOAPIC IRQ ERROR', b'HELIX TIMER FAILED:'):
            if marker in serial_data:
                raise RuntimeError('guest fatal/refusal marker: ' + marker.decode())
    def wait_text(value, start=0):
        wanted = value.encode() if isinstance(value, str) else value
        while wanted not in serial_data[start:]:
            consume()
    def wait_regex(pattern, start=0):
        while True:
            match = re.search(pattern, serial_data[start:])
            if match:
                return match
            consume()
    try:
        with tempfile.TemporaryDirectory(prefix='next-apic-rollback-', dir='/tmp') as folder:
            temp = Path(folder)
            # Reuse the authenticated bundle reader only after real signature verification.
            bundle = temp/'SYSTEM.NXP'
            subprocess.run(['mcopy', '-i', str(image), '::/EFI/NEXT/SYSTEM.NXP', str(bundle)], check=True, capture_output=True, timeout=remaining(), env=env)
            tool = ROOT/'target/debug/next-package'
            public = Path(env.get('NEXT_DEV_PUBLIC_KEY', str(ROOT/'work/package-dev.pub')))
            verified = subprocess.run([str(tool), 'verify', str(public), str(bundle)], check=True, capture_output=True, timeout=remaining(), env=env)
            members = package_files(bundle.read_bytes())
            kernel = members['boot/helix.elf']
            report['kernel_sha256'] = hashlib.sha256(kernel).hexdigest()
            report['deployment_sha256'] = digest(bundle)
            report['package_verification'] = verified.stdout.decode(errors='replace').strip()
            peers.append(Responder())
            peers.append(DnsResponder(39099, drop_first=True))
            peers.append(TcpResponder())
            variables = temp/'vars.fd'
            shutil.copyfile(firmware/'OVMF_VARS_4M.fd', variables)
            command = [str(qemu), '-machine', 'q35,accel=tcg', '-cpu', args.cpu, '-m', '256M', '-smp', '1', '-vga', 'std',
                       '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE_4M.fd"}',
                       '-drive', f'if=pflash,format=raw,file={variables}',
                       '-drive', f'if=none,id=nextdisk,format=raw,snapshot=on,file={image}',
                       '-device', 'virtio-blk-pci,drive=nextdisk,disable-modern=on',
                       '-netdev', 'user,id=nextnet', '-device', 'virtio-net-pci,netdev=nextnet,disable-modern=on',
                       '-audiodev', f'wav,id=nextaudio,path={output/"audio.wav"},out.frequency=48000,out.channels=2,out.format=s16',
                       '-device', 'AC97,audiodev=nextaudio', '-display', 'none',
                       '-serial', f'unix:{temp/"serial.sock"},server=on,wait=off',
                       '-qmp', f'unix:{temp/"qmp.sock"},server=on,wait=off', '-no-reboot']
            for directory in ('usr/share/qemu', 'usr/share/seabios', 'usr/lib/ipxe/qemu'):
                if (sysroot/directory).exists():
                    command.extend(['-L', str(sysroot/directory)])
            with (output/'qemu.log').open('wb') as log:
                process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env)
                serial = connect(temp/'serial.sock')
                qmp = connect(temp/'qmp.sock')
                stream = qmp.makefile('rb'); channels.append(stream)
                qmp.settimeout(remaining())
                if 'QMP' not in json.loads(stream.readline()):
                    raise RuntimeError('invalid QMP greeting')
                request_id = 0
                def qmp_command(name, arguments=None):
                    nonlocal request_id
                    request_id += 1
                    request = {'execute': name, 'id': request_id}
                    if arguments:
                        request['arguments'] = arguments
                    qmp.sendall(json.dumps(request).encode() + b'\n')
                    while True:
                        qmp.settimeout(remaining())
                        answer = json.loads(stream.readline())
                        if answer.get('id') == request_id:
                            if 'error' in answer:
                                raise RuntimeError('QMP error: ' + str(answer['error']))
                            return answer.get('return')
                qmp_command('qmp_capabilities')
                wait_text('NEXT BOOT: SIGNATURE PASS')
                wait_text('HELIX IOAPIC IRQ RESULT Report')
                wait_text('; PIT remains time source')
                report['routing'] = verify_routing_report(serial_data)
                report['checks'].append('one genuine IOAPIC IRQ preceded injected Hardware failure; masked route/SVR/TPR/PIC controls restored and three fresh PIC ISR IRQs observed')
                wait_text('PRISM: READY')
                report['checks'].append('signed diagnostic deployment reached Prism after ordinary verified rollback')
                start = len(serial_data)
                for key in ('s', 't', 'a', 't', 'u', 's', 'ret'):
                    qmp_command('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 40})
                    time.sleep(min(.08, remaining()))
                wait_text('NEXT: kernel-control=independent boot-info=validated framebuffer=active input=polling', start)
                wait_text('PRISM FRAME:', start)
                report['checks'].append('actual QMP PS/2 status command and subsequent framebuffer presentation after PIC fallback')
                clocks = []
                for _ in range(2):
                    start = len(serial_data)
                    serial.sendall(b'uptime\r')
                    match = wait_regex(rb'NEXT CLOCK: ticks=(\d+) nanoseconds=(\d+) source=PIT divisor=11931', start)
                    clocks.append(tuple(map(int, match.groups())))
                    time.sleep(min(.04, remaining()))
                if clocks[0][0] < 3 or any(b <= a for a, b in zip(clocks[0], clocks[1])):
                    raise RuntimeError('PIT-backed fallback clock did not continue advancing')
                report['clock_samples'] = clocks
                report['checks'].append('PIT monotonic ticks/nanoseconds continue advancing under restored PIC IRQ delivery')
                report['lapic_after_rollback'] = qmp_command('human-monitor-command', {'command-line': 'info lapic'})
                qmp_command('screendump', {'filename': str(output/'screenshot.ppm'), 'format': 'ppm'})
                report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(timeout=5)
        for channel in reversed(channels):
            channel.close()
        for peer in reversed(peers):
            peer.close()
        report['image_sha256_after'] = digest(image)
        report['default_image_sha256_after'] = digest(default) if default.exists() else None
        if report['image_sha256_after'] != report['image_sha256'] or report['default_image_sha256_after'] != report['default_image_sha256']:
            report['passed'] = False
            report['error'] = 'alternate or default image changed during read-only proof'
        (output/'apic-rollback.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
