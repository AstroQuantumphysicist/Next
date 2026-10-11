#!/usr/bin/env python3
"""Prove bounded read-only GPT/ESP discovery in an alternate generated VM image.

The kernel deliberately halts after this probe: writable GPT policy and full
GPT boot are pending. Neither the default image nor an existing disk is written.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid
import gpt_plan as gp

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def prepare(source, target, corrupt=False):
    before = digest(source)
    capacity = 70 << 20
    parts = gp.plan(capacity, esp=64 << 20, system=65536,
                    recovery=65536, minimum_data=65536)
    metadata = gp.encode(capacity, 512, uuid.uuid4(), parts)
    esp = next(p for p in parts if p.type_guid == gp.ESP)
    data = next(p for p in parts if p.type_guid == gp.TYPES['data'])
    if corrupt:
        last = capacity // 512 - 1
        changed = bytearray(metadata[last])
        changed[16] ^= 1
        metadata[last] = bytes(changed)
    with target.open('xb') as out, source.open('rb') as inp:
        out.truncate(capacity)
        for lba, content in metadata.items():
            out.seek(lba * 512)
            out.write(content)
        out.seek(esp.first * 512)
        remaining = 64 << 20
        while remaining:
            content = inp.read(min(1 << 20, remaining))
            if not content:
                raise ValueError('default ESP is truncated')
            out.write(content)
            remaining -= len(content)
        out.flush()
        os.fsync(out.fileno())
    if digest(source) != before:
        raise ValueError('default image changed during preparation')
    return {'source_image_sha256': before, 'alternate_image_sha256': digest(target),
            'esp_start': esp.first, 'esp_blocks': esp.last - esp.first + 1,
            'data_start': data.first, 'data_blocks': data.last - data.first + 1,
            'corrupt_backup': corrupt}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, default=ROOT/'build/next-dev.img')
    parser.add_argument('--timeout', type=float, default=45)
    args = parser.parse_args()
    if not 0 < args.timeout <= 300:
        parser.error('timeout must be positive and <=300 seconds')
    source = args.image.resolve()
    root = ROOT/'build/gpt-guest'
    root.mkdir(parents=True, exist_ok=True)
    work = ROOT/'work'
    work.mkdir(exist_ok=True)
    sysroot = Path(os.environ.get('NEXT_SYSROOT', '/workspace/toolchains/sysroot'))
    env = os.environ.copy()
    env['LD_LIBRARY_PATH'] = str(sysroot/'usr/lib/x86_64-linux-gnu') + ':' + env.get('LD_LIBRARY_PATH', '')
    qemu = sysroot/'usr/bin/qemu-system-x86_64'
    firmware = sysroot/'usr/share/OVMF'
    overall = {'passed': False, 'scope': 'QEMU readonly GPT/ESP discovery and corrupted-backup refusal; kernel halts before writable GPT policy/services. No installer, formatting, repair or full GPT boot.', 'cases': []}
    try:
        with tempfile.TemporaryDirectory(prefix='gpt-guest-', dir=work) as tmp:
            tmp = Path(tmp)
            for corrupt in (False, True):
                name = 'corrupt-backup' if corrupt else 'valid'
                output = root/name
                output.mkdir(exist_ok=True)
                image = tmp/(name+'.img')
                report = prepare(source, image, corrupt)
                variables = tmp/(name+'-vars.fd')
                shutil.copyfile(firmware/'OVMF_VARS_4M.fd', variables)
                serial = output/'serial.log'
                serial.unlink(missing_ok=True)
                command = [str(qemu), '-machine', 'q35,accel=tcg', '-cpu', 'qemu64', '-m', '256M', '-smp', '1', '-vga', 'std',
                           '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE_4M.fd"}',
                           '-drive', f'if=pflash,format=raw,file={variables}',
                           '-drive', f'if=none,id=nextdisk,format=raw,readonly=on,file={image}',
                           '-device', 'virtio-blk-pci,drive=nextdisk,disable-modern=on',
                           '-display', 'none', '-serial', f'file:{serial}', '-no-reboot']
                for directory in ('usr/share/qemu', 'usr/share/seabios', 'usr/lib/ipxe/qemu'):
                    if (sysroot/directory).exists(): command.extend(['-L', str(sysroot/directory)])
                with (output/'qemu.log').open('wb') as log:
                    process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
                    try:
                        deadline = time.monotonic()+args.timeout
                        expected = b'HELIX GPT DISCOVERY REJECT' if corrupt else b'HELIX GPT STORE REJECT'
                        while True:
                            text = serial.read_bytes() if serial.exists() else b''
                            if expected in text: break
                            if any(marker in text for marker in (b'HELIX PANIC:', b'HELIX FATAL:', b'NEXT BOOT FAILED:')):
                                raise RuntimeError('alternate GPT guest failed before expected discovery')
                            if time.monotonic() >= deadline or process.poll() is not None:
                                raise TimeoutError('alternate GPT discovery deadline')
                            time.sleep(0.05)
                        if not corrupt:
                            wanted = f"HELIX GPT DISCOVERY PASS esp_start={report['esp_start']} esp_blocks={report['esp_blocks']} data_start={report['data_start']} data_blocks={report['data_blocks']}".encode()
                            if wanted not in text or b'HELIX GPT ESP NOTE: Read by Helix from the Next development disk.' not in text:
                                raise RuntimeError('actual GPT extents or authoritative ESP note differ')
                        if b'HELIX STORAGE WRITE PASS' in text or b'HELIX COW STORE PASS' in text:
                            raise RuntimeError('readonly discovery entered writable profile')
                    finally:
                        process.terminate()
                        try: process.wait(timeout=3)
                        except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=3)
                report['alternate_unchanged'] = digest(image) == report['alternate_image_sha256']
                report['source_unchanged'] = digest(source) == report['source_image_sha256']
                report['passed'] = report['alternate_unchanged'] and report['source_unchanged']
                (output/'report.json').write_text(json.dumps(report, indent=2)+'\n')
                overall['cases'].append(report)
                print(name+': '+('PASS' if report['passed'] else 'FAIL'), flush=True)
        overall['passed'] = all(case['passed'] for case in overall['cases']) and len(overall['cases']) == 2
    except (OSError, ValueError, RuntimeError, TimeoutError) as error:
        overall['error'] = str(error)
    (root/'gpt-guest.json').write_text(json.dumps(overall, indent=2)+'\n')
    return 0 if overall['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
