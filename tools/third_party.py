#!/usr/bin/env python3
"""Create pinned Cargo dependency inventory and copy actual license notices offline."""
from pathlib import Path
import hashlib
import json
import shutil
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    metadata = json.loads((ROOT / 'work/metadata.json').read_text())
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    checksums = {(p['name'], p['version']): p['checksum'] for p in lock['package'] if 'checksum' in p}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    packages = sorted((p for p in metadata['packages'] if p['source'] is not None), key=lambda p: (p['name'], p['version']))
    destination = ROOT / 'docs/third-party'
    destination.mkdir(parents=True, exist_ok=True)
    components = []
    dependencies = []
    rows = []
    notices = []
    for package in packages:
        name, version = package['name'], package['version']
        source = Path(package['manifest_path']).parent
        archive = source.parent.parent.parent / 'cache' / source.parent.name / f'{name}-{version}.crate'
        archive_hash = hashlib.sha256(archive.read_bytes()).hexdigest()
        if archive_hash != checksums[(name, version)]:
            raise SystemExit(f'Registry archive checksum mismatch for {name} {version}')
        license_files = sorted(p for p in source.iterdir() if p.is_file() and p.name.lower().startswith(('license', 'copying', 'copyright', 'notice')))
        if package['license_file']:
            custom = source / package['license_file']
            if custom.is_file() and custom not in license_files:
                license_files.append(custom)
        if not license_files:
            raise SystemExit(f'Missing license notice for {name} {version}')
        copied = []
        for license_file in license_files:
            relative = Path('licenses') / f'{name}-{version}' / license_file.name
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            # Preserve notices from the checksum-authenticated archive, checking
            # that the local extracted source has not drifted before copying.
            with tarfile.open(archive) as package_archive:
                member = package_archive.extractfile(f'{name}-{version}/{license_file.name}')
                if member is None or member.read() != license_file.read_bytes():
                    raise SystemExit(f'License notice differs from pinned archive: {name}/{license_file.name}')
            shutil.copyfile(license_file, target)
            notices.append({'path': str(relative), 'sha256': hashlib.sha256(target.read_bytes()).hexdigest()})
            copied.append(str(relative))
        purl = f'pkg:cargo/{name}@{version}'
        # Older crates use Cargo's historical slash syntax for alternative licenses.
        expression = package['license'].replace('MIT/Apache-2.0', 'MIT OR Apache-2.0')
        components.append({
            'type': 'library', 'bom-ref': package['id'], 'name': name, 'version': version,
            'purl': purl, 'licenses': [{'expression': expression}],
            'hashes': [{'alg': 'SHA-256', 'content': checksums[(name, version)]}],
            'externalReferences': [{'type': 'distribution', 'url': f'https://crates.io/api/v1/crates/{name}/{version}/download'}],
            'properties': [{'name': 'next:license-notices', 'value': ','.join(copied)},
                           {'name': 'next:resolved-features', 'value': ','.join(nodes[package['id']]['features'])}],
        })
        dependencies.append({'ref': package['id'], 'dependsOn': nodes[package['id']]['dependencies']})
        links = ', '.join(f'[{Path(file).name}]({file})' for file in copied)
        rows.append(f'| {name} | {version} | {expression} | {links} |')
    workspace = [p for p in metadata['packages'] if p['source'] is None]
    for package in workspace:
        components.append({'type': 'application', 'bom-ref': package['id'], 'name': package['name'], 'version': package['version'], 'licenses': [{'expression': package['license']}]})
        dependencies.append({'ref': package['id'], 'dependsOn': nodes[package['id']]['dependencies']})
    sbom = {
        'bomFormat': 'CycloneDX', 'specVersion': '1.5', 'version': 1,
        'metadata': {'component': {'type': 'application', 'name': 'Next development operating system', 'version': '0.1.0'},
                     'properties': [{'name': 'next:scope', 'value': 'Cargo.lock and cargo metadata dependency graph; includes host/build and target-specific packages, not a target-specific runtime inventory'}]},
        'components': components, 'dependencies': dependencies,
    }
    (destination / 'sbom.cdx.json').write_text(json.dumps(sbom, indent=2) + '\n')
    (destination / 'license-manifest.json').write_text(json.dumps(notices, indent=2) + '\n')
    readme = """# Third-party Rust dependencies

This inventory records the exact Cargo dependency graph, registry archive
SHA-256 checksums, resolved features, and original license notices. It includes
host build tools and conditional packages; inclusion does not imply that every
component is linked into the kernel or EFI binary. These are libraries and
compiler tools, not a foreign operating-system kernel or hypervisor.

The signed-package verifier uses `sha2` 0.10.9 and `ed25519-dalek` 2.2.0 with
default features disabled. The project does not claim a separate security audit
of these versions. Their upstream license/copyright notices are preserved here.
Toolchains, QEMU, OVMF, and host utilities are outside this Cargo-only inventory.

Regenerate offline after dependency changes:

```
source tools/env.sh
cargo metadata --locked --offline --format-version 1 > work/metadata.json
python3 tools/third_party.py
```

The [CycloneDX SBOM](sbom.cdx.json) records the full graph and archive hashes.
The [license manifest](license-manifest.json) hashes the copied notice files.

| Crate | Version | SPDX license expression | Original notices |
| --- | --- | --- | --- |
"""
    (destination / 'README.md').write_text(readme + '\n'.join(rows) + '\n')
    print(f'Inventoried {len(packages)} registry crates; copied {len(notices)} license/copyright notices')


if __name__ == '__main__':
    main()
