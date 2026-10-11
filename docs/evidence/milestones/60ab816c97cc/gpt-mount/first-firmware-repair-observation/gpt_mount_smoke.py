#!/usr/bin/env python3
"""Prove authenticated GPT journal boot/persistence using only fresh owned VM images."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import time
import uuid
import zlib
import gpt_plan as gp
from initrd import pack as pack_initrd
from cow_verify import verify as verify_cow
from persistence_smoke import sha256

ROOT = Path(__file__).resolve().parents[1]
CAPACITY = 70 << 20
PREFIX_SECTORS = 132
CASES = ('valid', 'corrupt-backup', 'unsigned-supervisor', 'tampered-policy',
         'mismatched-policy', 'blank-journal', 'readonly-policy', 'readonly-attribute')


def run(command, env, timeout=30):
    result = subprocess.run([str(v) for v in command], env=env, cwd=ROOT,
                            capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError('host preparation failed: ' + str(command[0]) + ': ' +
                           result.stderr.decode(errors='replace')[-1000:])
    return result.stdout


def package_members(data):
    """Decode only after the real signature verifier has accepted this package."""
    if len(data) < 320:
        raise ValueError('truncated verified package')
    count, entry = struct.unpack_from('<II', data, 32)
    if not 1 <= count <= 32 or entry >= count or len(data) < 320 + count*128:
        raise ValueError('invalid verified package table')
    members = []
    for index in range(count):
        at = 256 + index * 128
        name = data[at:at+64].split(b'\0')[0].decode('ascii')
        offset, size = struct.unpack_from('<QQ', data, at+64)
        payload = data[offset:offset+size]
        if len(payload) != size or hashlib.sha256(payload).digest() != data[at+80:at+112]:
            raise ValueError('verified package digest changed')
        members.append((name, payload))
    identity = data[96:160].split(b'\0')[0].decode('ascii')
    version = '.'.join(str(v) for v in struct.unpack_from('<III', data, 48))
    return members, identity, version, struct.unpack_from('<Q', data, 40)[0], members[entry][0]


def archive_members(data):
    if len(data) < 32 or data[:8] != b'NEXTINIT':
        raise ValueError('source initrd header')
    count = struct.unpack_from('<I', data, 12)[0]
    if not 1 <= count <= 64 or len(data) < 32+count*80:
        raise ValueError('source initrd count')
    members = []
    for index in range(count):
        at = 32 + index * 80
        raw = data[at:at+64]
        name = raw.split(b'\0')[0].decode('ascii')
        offset, size = struct.unpack_from('<QQ', data, at+64)
        if offset + size > len(data):
            raise ValueError('source initrd range')
        members.append((name, data[offset:offset+size]))
    if pack_initrd(members) != data:
        raise ValueError('source initrd is not canonical/CRC valid')
    return members


def policy_bytes(disk, data, writable=True):
    return struct.pack('<8sHHI16s16sQQQII', b'NXTSTOR1', 1, 80, int(writable),
                       disk.bytes_le, data.unique_guid.bytes_le, data.first,
                       data.last-data.first+1, CAPACITY//512, 512, 0)


def sign_members(members, identity, version, permissions, kind, entry, directory, env):
    directory.mkdir()
    arguments = []
    for index, (name, payload) in enumerate(members):
        member = directory / f'member-{index}.bin'
        member.write_bytes(payload)
        arguments.append(f'{name}={member}')
    destination = directory / 'signed.nxp'
    run([ROOT/'target/debug/next-package', 'pack',
         env.get('NEXT_SIGNING_SEED', str(ROOT/'work/package-dev.seed')), identity,
         version, f'{permissions:x}', kind, entry, destination, *arguments], env)
    run([ROOT/'target/debug/next-package', 'verify',
         env.get('NEXT_DEV_PUBLIC_KEY', str(ROOT/'work/package-dev.pub')), destination], env)
    return destination.read_bytes()


def prepare(source, target, directory, case, env):
    """All mutations are to an exclusive new regular file under private work/."""
    if case not in CASES or not target.resolve().is_relative_to(ROOT/'work'):
        raise ValueError('only named cases in owned work/ scratch paths are supported')
    info = source.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_size != 65 << 20:
        raise ValueError('source must be the regular65MiB development image')
    source_hash = sha256(source)
    with source.open('rb') as stream:
        bpb = stream.read(512)
    if bpb[71:82] != b'NEXTDEV    ' or any(bpb[450+slot*16] == 0xee for slot in range(4)):
        raise ValueError('source is not the tagged superfloppy development profile')
    parts = gp.plan(CAPACITY, esp=64 << 20, system=65536, recovery=65536, minimum_data=65536)
    if case == 'readonly-attribute':
        parts = [gp.Partition(p.name,p.type_guid,p.unique_guid,p.first,p.last,
                             p.attributes | ((1 << 60) if p.type_guid == gp.TYPES['data'] else 0)) for p in parts]
    disk = uuid.uuid4()
    metadata = gp.encode(CAPACITY, 512, disk, parts)
    esp = next(p for p in parts if p.type_guid == gp.ESP)
    data = next(p for p in parts if p.type_guid == gp.TYPES['data'])
    with target.open('xb') as output, source.open('rb') as stream:
        output.truncate(CAPACITY)
        for lba, content in metadata.items():
            output.seek(lba*512); output.write(content)
        output.seek(esp.first*512)
        remaining = 64 << 20
        while remaining:
            chunk = stream.read(min(1 << 20, remaining))
            if not chunk: raise ValueError('truncated source ESP')
            output.write(chunk); remaining -= len(chunk)
    extracted = {}
    for name in ('SYSTEM.NXP','HELIX.ELF','INITRD.NXI'):
        destination = directory/name
        run(['mcopy','-i',source,f'::/EFI/NEXT/{name}',destination],env)
        extracted[name] = destination.read_bytes()
    public = env.get('NEXT_DEV_PUBLIC_KEY', str(ROOT/'work/package-dev.pub'))
    run([ROOT/'target/debug/next-package','verify',public,directory/'SYSTEM.NXP'],env)
    deployment, identity, version, permissions, entry = package_members(extracted['SYSTEM.NXP'])
    if identity != 'org.next.development' or permissions != 0 or entry != 'boot/helix.elf' or struct.unpack_from('<H',extracted['SYSTEM.NXP'],164)[0] != 2 or dict(deployment).get('boot/helix.elf') != extracted['HELIX.ELF'] or dict(deployment).get('boot/initrd.nxi') != extracted['INITRD.NXI']:
        raise ValueError('actual source boot artifacts differ from signed deployment')
    members = archive_members(extracted['INITRD.NXI'])
    supervisor = dict(members).get('/system/nextd.elf')
    if supervisor is None: raise ValueError('source supervisor missing')
    original = directory/'nextd.nxp'; original.write_bytes(supervisor)
    run([ROOT/'target/debug/next-package','verify',public,original],env)
    app, app_id, app_version, app_permissions, app_entry = package_members(supervisor)
    if app_id != 'org.next.development.native.nextd' or app_entry != 'bin/nextd.elf' or struct.unpack_from('<H',supervisor,164)[0] != 1:
        raise ValueError('source supervisor identity/entry differs')
    policy = policy_bytes(uuid.uuid4() if case == 'mismatched-policy' else disk,
                          data, case != 'readonly-policy')
    app = [(name,payload) for name,payload in app if name != 'config/storage.bin'] + [('config/storage.bin',policy)]
    signed = sign_members(app,app_id,app_version,app_permissions,'app',app_entry,directory/'app-sign',env)
    if case == 'unsigned-supervisor': signed = dict(app)[app_entry]
    if case == 'tampered-policy':
        changed = bytearray(signed)
        at = signed.index(policy); changed[at+12] ^= 1
        signed = bytes(changed)
    updated = [(name,signed if name == '/system/nextd.elf' else payload) for name,payload in members]
    archive = pack_initrd(updated)
    if any(dict(updated)[name] != payload for name,payload in members if name != '/system/nextd.elf'):
        raise ValueError('unrelated source application changed')
    deployment = [(name,archive if name == 'boot/initrd.nxi' else payload) for name,payload in deployment]
    bundle = sign_members(deployment,identity,version,permissions,'deployment',entry,directory/'deployment-sign',env)
    for name,payload in [('INITRD.NXI',archive),('SYSTEM.NXP',bundle)]:
        updated_file = directory/('updated-'+name); updated_file.write_bytes(payload)
        run(['mcopy','-o','-i',f'{target}@@{esp.first*512}',updated_file,f'::/EFI/NEXT/{name}'],env)
    for name,payload in [('HELIX.ELF',extracted['HELIX.ELF']),('INITRD.NXI',archive),('SYSTEM.NXP',bundle)]:
        readback=directory/('readback-'+name)
        run(['mcopy','-i',f'{target}@@{esp.first*512}',f'::/EFI/NEXT/{name}',readback],env)
        if readback.read_bytes()!=payload: raise ValueError('actual alternate ESP artifact differs: '+name)
    if case != 'blank-journal':
        blob = directory/'journal.bin'
        run([ROOT/'work/journal-fixture',data.first,CAPACITY//512,blob],env)
        with target.open('r+b') as output:
            output.seek(data.first*512); output.write(blob.read_bytes())
    if case == 'corrupt-backup':
        with target.open('r+b') as output:
            output.seek(CAPACITY-512+16); old = output.read(1)
            output.seek(-1,1); output.write(bytes([old[0]^1]))
    with target.open('r+b') as output: output.flush(); os.fsync(output.fileno())
    if sha256(source) != source_hash: raise ValueError('source image changed during preparation')
    return {'case':case,'source_image_sha256':source_hash,'prepared_sha256':sha256(target),
            'disk_guid':str(disk),'data_guid':str(data.unique_guid),'esp_start':esp.first,
            'data_start':data.first,'data_blocks':data.last-data.first+1,
            'journal_sectors':PREFIX_SECTORS,'kernel_sha256':hashlib.sha256(extracted['HELIX.ELF']).hexdigest()}


def protected_hash(image, start):
    return [sha256(image,length=start*512),
            sha256(image,start=(start+PREFIX_SECTORS)*512)]


def full_phase(image, output, phase, timeout, env):
    command = [sys.executable,ROOT/'tools/boot_smoke.py','--image',image,'--output-dir',output,
               '--timeout',str(timeout),'--persistent-data','--gpt-data-profile','--persistence-phase',phase]
    process = subprocess.Popen([str(v) for v in command],env=env,cwd=ROOT,
                               stdout=subprocess.PIPE,stderr=subprocess.STDOUT,start_new_session=True)
    try:
        transcript,_ = process.communicate(timeout=timeout+10)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid,signal.SIGTERM)
        try: transcript,_ = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid,signal.SIGKILL); transcript,_=process.communicate(timeout=5)
        raise TimeoutError('bounded GPT full-boot phase expired')
    (output/'runner.log').write_bytes(transcript)
    report = json.loads((output/'boot-smoke.json').read_text())
    serial = (output/'serial.log').read_bytes()
    wanted = b'PRISM EDITOR SAVE: path=/native/persistent bytes=16' if phase == 'create' else b'NEXT DATA: survives-rebootx'
    if process.returncode or not report.get('passed') or wanted not in serial:
        raise RuntimeError('GPT full-boot phase failed: '+str(report.get('error','see artifacts')))
    if b'formatted=true' in serial or b'HELIX STORAGE WRITE PASS' in serial:
        raise RuntimeError('GPT path entered formatting or legacy write probe')
    return {'phase':phase,'checks':report['checks'],'passed':True,
            'artifact':str((output/'serial.log').relative_to(ROOT))}


def gate_phase(image, output, case, timeout, env):
    output.mkdir(parents=True,exist_ok=True)
    sysroot = Path(env.get('NEXT_SYSROOT','/workspace/toolchains/sysroot'))
    firmware = sysroot/'usr/share/OVMF'
    variables = output/'vars.fd'; shutil.copyfile(firmware/'OVMF_VARS_4M.fd',variables)
    serial = output/'serial.log'; serial.write_bytes(b'')
    command = [sysroot/'usr/bin/qemu-system-x86_64','-machine','q35,accel=tcg','-cpu','qemu64','-m','256M','-smp','1','-vga','std',
               '-drive',f'if=pflash,format=raw,readonly=on,file={firmware/"OVMF_CODE_4M.fd"}',
               '-drive',f'if=pflash,format=raw,file={variables}',
               '-drive',f'if=none,id=nextdisk,format=raw,file={image}',
               '-device','virtio-blk-pci,drive=nextdisk,disable-modern=on',
               '-display','none','-serial',f'file:{serial}','-no-reboot']
    for directory in ('usr/share/qemu','usr/share/seabios','usr/lib/ipxe/qemu'):
        if (sysroot/directory).exists(): command.extend(['-L',str(sysroot/directory)])
    readonly = case.startswith('readonly-')
    expected = {
        'corrupt-backup': b'HELIX GPT DISCOVERY REJECT Gpt(HeaderCrc)',
        'unsigned-supervisor': b'HELIX GPT STORE REJECT Package(Header)',
        'tampered-policy': b'HELIX GPT STORE REJECT Package(Digest)',
        'mismatched-policy': b'HELIX GPT STORE REJECT Policy(Binding)',
        'blank-journal': b'HELIX GPT STORE REJECT InvalidFormat',
        'readonly-policy': b'read_only=true formatted=false selftest=false',
        'readonly-attribute': b'read_only=true formatted=false selftest=false',
    }[case]
    with (output/'qemu.log').open('wb') as log:
        process = subprocess.Popen([str(v) for v in command],env=env,stdout=log,stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic()+timeout
            while True:
                text = serial.read_bytes()
                if expected in text: break
                if any(marker in text for marker in (b'HELIX PANIC:',b'HELIX FATAL:',b'NEXT BOOT FAILED:')):
                    raise RuntimeError('GPT gate failed before expected kernel decision')
                if time.monotonic()>=deadline or process.poll() is not None:
                    raise TimeoutError('GPT gate deadline: '+case)
                time.sleep(0.05)
            if readonly and b'read_only=true formatted=false selftest=false' not in text:
                raise RuntimeError('GPT readonly attenuation missing')
            if not readonly and b'HELIX GPT STORE PASS' in text:
                raise RuntimeError('rejected policy/data mounted')
            if b'formatted=true' in text or b'HELIX STORAGE WRITE PASS' in text:
                raise RuntimeError('rejection/readonly gate invoked formatting/write probe')
            # Observe rejected halted guests briefly: a later entry must not
            # bypass the decision. Readonly cases are policy tests, not full boot.
            if not readonly:
                time.sleep(0.25)
                if b'PRISM:' in serial.read_bytes(): raise RuntimeError('rejected guest entered desktop')
        finally:
            process.terminate()
            try: process.wait(timeout=3)
            except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=3)
    return {'passed':True,'expected':expected.decode(),'artifact':str(serial.relative_to(ROOT))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image',type=Path,default=ROOT/'build/next-dev.img')
    parser.add_argument('--timeout',type=float,default=60)
    parser.add_argument('--case',choices=CASES,action='append',help='default: all cases')
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or not 0 < args.timeout <= 300: parser.error('timeout must be finite, positive and <=300')
    source = args.image.resolve(); output = ROOT/'build/gpt-mount'; output.mkdir(parents=True,exist_ok=True)
    env = os.environ.copy(); sysroot = Path(env.get('NEXT_SYSROOT','/workspace/toolchains/sysroot'))
    env['PATH'] = str(sysroot/'usr/bin')+':'+env.get('PATH','')
    env['LD_LIBRARY_PATH'] = str(sysroot/'usr/lib/x86_64-linux-gnu')+':'+env.get('LD_LIBRARY_PATH','')
    report = {'passed':False,'requested_cases':list(args.case or CASES),
              'scope':'Fresh owned GPT images; owner-signed exact-layout132-sector journal mount, actual Prism editor and clean VM reboot persistence, readonly/refusal gates. No installer, unknown-disk formatting, power-loss or hardware certification.','cases':[]}
    before = None
    try:
        before = sha256(source); run(['bash','tools/build-journal-fixture.sh'],env)
        with tempfile.TemporaryDirectory(prefix='gpt-mount-',dir=ROOT/'work') as temporary:
            temporary = Path(temporary)
            for case in args.case or CASES:
                case_dir=temporary/case; case_dir.mkdir()
                image=case_dir/'owned.img'
                item=prepare(source,image,case_dir,case,env)
                case_output=output/case; case_output.mkdir(parents=True,exist_ok=True)
                protected=protected_hash(image,item['data_start'])
                if case == 'valid':
                    item['phases']=[]
                    for phase in ('create','verify'):
                        item['phases'].append(full_phase(image,case_output/phase,phase,args.timeout,env))
                        if protected_hash(image,item['data_start']) != protected: raise RuntimeError('GPT guest changed bytes outside journal prefix')
                        if phase == 'create':
                            created=verify_cow(image,item['data_start'],PREFIX_SECTORS,[(0,item['data_start'])])
                            saved=next((file for file in created['files'] if file['path']=='/native/persistent'),None)
                            wanted=b'survives-rebootx'
                            if saved is None or saved['size']!=len(wanted) or saved['crc32']!=zlib.crc32(wanted):
                                raise RuntimeError('actual Editor commit differs from durable journal metadata')
                            at=(item['data_start']+2+created['selected_bank']*65+saved['slot']*8)*512
                            with image.open('rb') as stream:
                                stream.seek(at)
                                if stream.read(len(wanted))!=wanted: raise RuntimeError('actual Editor bytes were not durably committed')
                            item['editor_saved_content_sha256']=hashlib.sha256(wanted).hexdigest()
                    journal=verify_cow(image,item['data_start'],PREFIX_SECTORS,[(0,item['data_start']),
                        (item['data_start']+PREFIX_SECTORS,CAPACITY//512-item['data_start']-PREFIX_SECTORS)])
                    if journal['mount_read_only'] or journal['selected_generation']<=1:
                        raise RuntimeError('GPT durable journal did not remain valid/writable')
                    item['journal_verification']=journal
                    item['image_changed']=sha256(image)!=item['prepared_sha256']
                    if not item['image_changed']: raise RuntimeError('actual GPT writes were not durable')
                else:
                    item['gate']=gate_phase(image,case_output,case,args.timeout,env)
                    item['whole_image_unchanged']=sha256(image)==item['prepared_sha256']
                    if not item['whole_image_unchanged']: raise RuntimeError('refused/readonly GPT image changed')
                item['protected_outside_prefix_unchanged']=protected_hash(image,item['data_start'])==protected
                item['source_unchanged']=sha256(source)==before
                item['passed']=item['protected_outside_prefix_unchanged'] and item['source_unchanged']
                (case_output/'report.json').write_text(json.dumps(item,indent=2)+'\n')
                report['cases'].append(item); print(case+': PASS',flush=True)
        report['passed']=bool(report['cases']) and all(item['passed'] for item in report['cases'])
    except (OSError,ValueError,RuntimeError,TimeoutError,subprocess.SubprocessError) as error:
        report['error']=str(error); print(str(error),file=sys.stderr)
    finally:
        try: report['source_image_unchanged']=before is not None and sha256(source)==before
        except OSError: report['source_image_unchanged']=False
        report['passed']=report['passed'] and report['source_image_unchanged']
        (output/'gpt-mount.json').write_text(json.dumps(report,indent=2)+'\n')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
