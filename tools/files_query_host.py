#!/usr/bin/env python3
"""Read-only, retained-source Files query oracle for a paused guest disk.

Compiles the authenticated build's actual Store, Query model and kernel query
backend. This is a host oracle, not evidence of guest UI execution. Caller must
pause the VM while reading a live disk. No image build, repair or write occurs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parent.parent
HOST = ROOT / 'tools/files_query_host.rs'
BACKEND = 'crates/helix-kernel/src/files_query.rs'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verified_sources(manifest, frozen):
    records = json.loads(manifest.read_text())
    wanted = {p: h for p, h in records.items() if p.startswith(('crates/prism/', 'crates/next-fs/')) or p == BACKEND}
    if BACKEND not in wanted or not all(any(p.startswith(prefix) for p in wanted) for prefix in ('crates/prism/', 'crates/next-fs/')):
        raise RuntimeError('manifest lacks query backend or required crates')
    for relative, value in wanted.items():
        path = frozen / relative
        if not path.resolve().is_relative_to(frozen.resolve()) or not path.is_file() or digest(path) != value:
            raise RuntimeError(f'retained query source differs from manifest: {relative}')
    for crate in ('prism', 'next-fs'):
        actual = {p.relative_to(frozen).as_posix() for p in (frozen / 'crates' / crate).rglob('*') if p.is_file()}
        expected = {p for p in wanted if p.startswith(f'crates/{crate}/')}
        if actual != expected:
            raise RuntimeError(f'unmanifested or missing {crate} source')
    return wanted


def ensure_host(manifest, frozen, output):
    sources = verified_sources(manifest, frozen)
    rustc = shutil.which('rustc')
    if not rustc:
        raise RuntimeError('source tools/env.sh first')
    output.mkdir(parents=True, exist_ok=True)
    libraries = {}
    for name, crate in [('next_fs', 'next-fs'), ('prism', 'prism')]:
        library = output / f'lib{name}.rlib'
        subprocess.run([rustc, '--edition=2021', '--crate-name', name, '--crate-type=rlib', '-Cpanic=abort',
                        str(frozen / f'crates/{crate}/src/lib.rs'), '-o', str(library)], check=True)
        libraries[name] = library
    generated = output / 'files_query_host.rs'
    code = HOST.read_text()
    needle = '#[path = "../crates/helix-kernel/src/files_query.rs"]'
    if code.count(needle) != 1:
        raise RuntimeError('host backend marker is ambiguous')
    generated.write_text(code.replace(needle, '#[path = ' + json.dumps(str(frozen / BACKEND), ensure_ascii=False) + ']'))
    helper = output / 'files-query-host'
    subprocess.run([rustc, '--edition=2021', '-Dwarnings', '-Cpanic=abort', str(generated),
                    '--extern', f'next_fs={libraries["next_fs"]}', '--extern', f'prism={libraries["prism"]}', '-o', str(helper)], check=True)
    binding = {'manifest_sha256': digest(manifest), 'sources': sources, 'helper_source_sha256': digest(HOST),
               'helper_sha256': digest(helper), 'libraries': {k: digest(v) for k, v in libraries.items()}}
    (output / 'host-binding.json').write_text(json.dumps(binding, indent=2) + '\n')
    return helper, binding


def query(helper, image, start, width=1280, height=800, text='', sort='name', descending=False, content=False):
    before = digest(image)
    raw = subprocess.check_output([str(helper), str(image), str(start), str(width), str(height), text,
                                   sort, 'desc' if descending else 'asc', 'content' if content else 'path']).decode()
    result = {'rows': [], 'controls': {}}
    for line in raw.splitlines():
        parts = line.split(maxsplit=5)
        if parts[0] == 'stats':
            result.update(zip(('revision', 'total', 'matched'), map(int, parts[1:])))
        elif parts[0] == 'row':
            result['rows'].append(dict(zip(('position', 'x', 'text_y', 'size', 'path'),
                                           [*map(int, parts[1:5]), parts[5]])))
        elif parts[0] == 'control':
            result['controls'][int(parts[1])] = list(map(int, parts[2:]))
        else:
            raise RuntimeError('unknown oracle output')
    if digest(image) != before:
        raise RuntimeError('disk changed during readonly query; pause live VM first')
    if len(result['rows']) != result.get('matched'):
        raise RuntimeError('incomplete query result')
    result['image_sha256'] = before
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True, type=Path)
    parser.add_argument('--source-manifest', required=True, type=Path)
    parser.add_argument('--frozen-source-root', required=True, type=Path)
    parser.add_argument('--output-dir', required=True, type=Path)
    parser.add_argument('--start', type=int, default=131072)
    parser.add_argument('--text', default='')
    parser.add_argument('--sort', choices=('name', 'size'), default='name')
    parser.add_argument('--descending', action='store_true')
    parser.add_argument('--content', action='store_true')
    args = parser.parse_args()
    output = args.output_dir.resolve()
    if not output.is_relative_to(ROOT / 'build') or args.start < 0:
        parser.error('output must be in build/ and start nonnegative')
    helper, binding = ensure_host(args.source_manifest.resolve(), args.frozen_source_root.resolve(), output)
    result = query(helper, args.image.resolve(), args.start, text=args.text, sort=args.sort,
                   descending=args.descending, content=args.content)
    result['source_binding'] = binding
    (output / 'query.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
