#!/usr/bin/env python3
"""Prepare a signed after-unmask APIC fault image without changing the base.

Builds only helix-kernel in work/apic-rollback-target, passing the existing seed
path to next-package CLI without reading its contents. Preserves authenticated
initrd/application bytes and the original bootloader. Never boots a VM.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
from service_pair_smoke import ROOT, package_files


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def sources():
    files = [ROOT/'Cargo.toml', ROOT/'Cargo.lock', ROOT/'tools/env.sh', ROOT/'tools/kernel-env.sh']
    for name in ('helix-kernel', 'helix-core', 'next-abi', 'next-fs', 'next-package', 'prism'):
        directory = ROOT/'crates'/name
        files.extend(p for p in directory.rglob('*') if p.is_file() and p.suffix in ('.rs', '.toml', '.ld'))
    if (ROOT/'.cargo/config.toml').exists():
        files.append(ROOT/'.cargo/config.toml')
    return {str(p.relative_to(ROOT)): digest(p) for p in sorted(set(files))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, default=ROOT/'build/next-dev.img')
    parser.add_argument('--output-dir', type=Path, required=True, help='new explicit output directory below work/')
    parser.add_argument('--expected-base-sha256', help='refuse if the frozen base image differs')
    args = parser.parse_args()
    image = args.image.resolve()
    output = args.output_dir.resolve()
    target = ROOT/'work/apic-rollback-target'
    if not output.is_relative_to(ROOT/'work') or output == ROOT/'work' or output == target or output.is_relative_to(target):
        parser.error('output must be an explicit directory below work/, outside the Cargo target')
    if output.exists():
        parser.error('output directory must not already exist')
    if not image.is_file():
        parser.error('base image missing')
    before = digest(image)
    if args.expected_base_sha256 and before != args.expected_base_sha256:
        parser.error('base image differs from expected frozen hash')
    output.mkdir(parents=True)
    env = os.environ.copy()
    public = Path(env.get('NEXT_DEV_PUBLIC_KEY', str(ROOT/'work/package-dev.pub'))).resolve()
    seed = Path(env.get('NEXT_SIGNING_SEED', str(ROOT/'work/package-dev.seed'))).resolve()
    tool = ROOT/'target/debug/next-package'
    if not public.is_file() or not seed.is_file() or not tool.is_file():
        parser.error('existing signing paths/package CLI are required; this tool creates no keys')
    source_before = sources()
    source_hash = hashlib.sha256(json.dumps(source_before, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    report = {'prepared': False, 'scope': 'disposable signed q35 APIC after-unmask diagnostic; kernel payload only, no VM execution',
              'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'base_image': str(image), 'base_image_sha256': before,
              'source_sha256': source_hash, 'source_files': source_before,
              'builder_sha256': digest(Path(__file__)),
              'public_key_sha256': digest(public), 'compile_policy': 'NEXT_DEV_APIC_FAILURE_STAGE=after-unmask',
              'cargo_target_dir': str(target)}
    def run(arguments, *, timeout=120):
        # Never log the signing command or private key path/content.
        return subprocess.run([str(a) for a in arguments], cwd=ROOT, env=env,
                              check=True, capture_output=True, timeout=timeout)
    def extract(source, name, destination):
        run(['mcopy', '-i', source, '::/'+name, destination])
    try:
        with tempfile.TemporaryDirectory(prefix='apic-prepare-', dir=ROOT/'work') as directory:
            stage = Path(directory)
            bundle = stage/'original.nxp'
            extract(image, 'EFI/NEXT/SYSTEM.NXP', bundle)
            run([tool, 'verify', public, bundle])
            original = bundle.read_bytes()
            members = package_files(original)
            if set(members) != {'boot/helix.elf', 'boot/initrd.nxi'}:
                raise ValueError('frozen deployment must have exactly kernel/initrd members')
            bootloader = stage/'BOOTX64.EFI'
            mirror_initrd = stage/'INITRD.NXI'
            mirror_kernel = stage/'original.elf'
            extract(image, 'EFI/BOOT/BOOTX64.EFI', bootloader)
            extract(image, 'EFI/NEXT/INITRD.NXI', mirror_initrd)
            extract(image, 'EFI/NEXT/HELIX.ELF', mirror_kernel)
            if mirror_initrd.read_bytes() != members['boot/initrd.nxi'] or mirror_kernel.read_bytes() != members['boot/helix.elf']:
                raise ValueError('base mirrors differ from authenticated deployment')
            report.update({'base_deployment_sha256': digest(bundle),
                           'base_kernel_sha256': digest(mirror_kernel),
                           'bootloader_sha256': digest(bootloader), 'initrd_sha256': digest(mirror_initrd)})
            env['CARGO_TARGET_DIR'] = str(target)
            env['NEXT_DEV_APIC_FAILURE_STAGE'] = 'after-unmask'
            env['NEXT_DEV_PUBLIC_KEY'] = str(public)
            build = subprocess.run(['bash', '-c', 'source tools/env.sh\nsource tools/kernel-env.sh\nRUSTFLAGS="$NEXT_KERNEL_FLAGS" cargo build --locked --offline --release -p helix-kernel --target x86_64-unknown-none'],
                                   cwd=ROOT, env=env, capture_output=True, timeout=300, check=False)
            (output/'kernel-build.log').write_bytes(build.stdout + build.stderr)
            if build.returncode:
                raise RuntimeError('isolated kernel build failed; see kernel-build.log')
            if sources() != source_before or digest(public) != report['public_key_sha256']:
                raise RuntimeError('frozen source or public anchor changed during isolated build')
            kernel = output/'HELIX.ELF'
            shutil.copyfile(target/'x86_64-unknown-none/release/helix-kernel', kernel)
            if digest(kernel) == report['base_kernel_sha256']:
                raise RuntimeError('diagnostic kernel is identical to normal kernel; injection not compiled')
            identity = original[96:160].split(b'\0')[0].decode('ascii')
            version = '.'.join(str(value) for value in struct.unpack_from('<III', original, 48))
            rights = format(int.from_bytes(original[40:48], 'little'), 'x')
            if int.from_bytes(original[164:166], 'little') != 2:
                raise ValueError('authenticated source is not a deployment')
            alternate_bundle = output/'SYSTEM.NXP'
            run([tool, 'pack', seed, identity, version, rights, 'deployment', 'boot/helix.elf', alternate_bundle,
                 f'boot/helix.elf={kernel}', f'boot/initrd.nxi={mirror_initrd}'])
            verified = run([tool, 'verify', public, alternate_bundle])
            changed = package_files(alternate_bundle.read_bytes())
            if changed['boot/initrd.nxi'] != members['boot/initrd.nxi'] or changed['boot/helix.elf'] != kernel.read_bytes():
                raise RuntimeError('signed alternate changed preserved bytes or kernel differs')
            alternate = output/'next-apic-rollback.img'
            shutil.copyfile(image, alternate)
            run(['mcopy', '-o', '-i', alternate, alternate_bundle, '::/EFI/NEXT/SYSTEM.NXP'])
            run(['mcopy', '-o', '-i', alternate, kernel, '::/EFI/NEXT/HELIX.ELF'])
            for path, expected, name in [('EFI/BOOT/BOOTX64.EFI', bootloader.read_bytes(), 'check-boot.efi'),
                                         ('EFI/NEXT/INITRD.NXI', mirror_initrd.read_bytes(), 'check-initrd.nxi'),
                                         ('EFI/NEXT/HELIX.ELF', kernel.read_bytes(), 'check-kernel.elf'),
                                         ('EFI/NEXT/SYSTEM.NXP', alternate_bundle.read_bytes(), 'check-deployment.nxp')]:
                recovered = stage/name
                extract(alternate, path, recovered)
                if recovered.read_bytes() != expected:
                    raise RuntimeError('alternate disk member differs: ' + path)
            run([tool, 'verify', public, stage/'check-deployment.nxp'])
            report.update({'image': str(alternate), 'image_sha256': digest(alternate),
                           'kernel_sha256': digest(kernel), 'deployment_sha256': digest(alternate_bundle),
                           'package_verification': verified.stdout.decode(errors='replace').strip(),
                           'bootloader_preserved': True, 'initrd_preserved': True})
            if sources() != source_before or digest(public) != report['public_key_sha256']:
                raise RuntimeError('frozen source or public anchor changed during signing')
            report['prepared'] = True
    except Exception as error:
        # CalledProcessError text contains command/key path; keep diagnostics
        # concise and never serialize signing command arguments.
        report['error'] = type(error).__name__ if isinstance(error, subprocess.CalledProcessError) else str(error)
    finally:
        report['base_image_sha256_after'] = digest(image)
        if report['base_image_sha256_after'] != before:
            report['prepared'] = False
            report['error'] = 'base image changed during preparation'
        (output/'prepare.json').write_text(json.dumps(report, indent=2) + '\n')
    # Detailed source map stays in the artifact; display only reviewable results.
    print(json.dumps({key: value for key, value in report.items() if key != 'source_files'}, indent=2))
    return 0 if report['prepared'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
