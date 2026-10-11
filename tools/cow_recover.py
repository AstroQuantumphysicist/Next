#!/usr/bin/env python3
"""Offline read-only NXTCOW01 snapshot export; no source repairs or full fsck."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import re
import sys
import cow_verify as cv

NOFOLLOW = os.O_NOFOLLOW
DIRECTORY = os.O_DIRECTORY
MANIFEST_LIMIT = 16384


def open_directory(path):
    """Walk directory descriptors without following any symlink component."""
    absolute = Path(path).absolute()
    cv.require('..' not in absolute.parts, 'dot-parent directory component')
    descriptor = os.open('/', os.O_RDONLY | DIRECTORY | NOFOLLOW)
    try:
        for component in absolute.parts[1:]:
            child = os.open(component, os.O_RDONLY | DIRECTORY | NOFOLLOW, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def snapshot(source, start, sectors, protected, generation=None):
    end = cv.extent(start, sectors)
    descriptor = os.open(source, os.O_RDONLY | os.O_NONBLOCK | NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as stream:
        info = os.fstat(stream.fileno())
        cv.require(stat.S_ISREG(info.st_mode), 'only regular offline scratch images supported')
        cv.require(info.st_size % 512 == 0 and end <= info.st_size // 512,
                   'truncated media or extent outside image')
        before = cv.digest(stream)
        stream.seek(start * 512)
        data = stream.read(cv.REQUIRED * 512)
        report = cv.verify_bytes(data, start, sectors, protected)
        cv.require(cv.digest(stream) == before, 'source changed during snapshot capture')
    candidates = [c for c in report['candidates'] if c.get('snapshot_valid')]
    if generation is None:
        generation = report['selected_generation']
    selected = next((c for c in candidates if c['generation'] == generation), None)
    cv.require(selected is not None, 'requested generation is not a verified committed snapshot')
    files = []
    for file in selected['files']:
        at = (2 + selected['bank'] * 65 + file['slot'] * 8) * 512
        payload = data[at:at + file['size']]
        # Export from the same validated captured bytes, never re-resolve source.
        files.append((file, payload))
    return before, selected, files


def write_new(directory, name, payload):
    descriptor = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | NOFOLLOW,
                         0o600, dir_fd=directory)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


def export(source, destination, start, sectors, protected, generation=None, checkpoint=lambda _: None):
    source_hash, selected, files = snapshot(source, start, sectors, protected, generation)
    destination = Path(destination).absolute()
    cv.require(destination.name not in ('', '.', '..') and '..' not in destination.parts,
               'invalid destination path')
    parent = open_directory(destination.parent)
    directory = None
    try:
        info = os.fstat(parent)
        cv.require(info.st_uid == os.geteuid() and not info.st_mode & 0o022,
                   'destination parent must be caller-owned and not group/world writable')
        # Exclusive mkdir rejects files, directories and symlinks already present.
        os.mkdir(destination.name, 0o700, dir_fd=parent)
        directory = os.open(destination.name, os.O_RDONLY | DIRECTORY | NOFOLLOW, dir_fd=parent)
        os.fsync(parent)
        write_new(directory, 'INCOMPLETE', b'Incomplete recovery export; do not restore.\n')
        os.fsync(directory)
        checkpoint('incomplete')
        manifest = {'format': 'NXTCOW01-recovery-export', 'version': 1,
                    'source_sha256': source_hash, 'start_lba': start, 'sectors': sectors,
                    'generation': selected['generation'], 'bank': selected['bank'], 'files': []}
        for metadata, payload in files:
            name = f"slot-{metadata['slot']}.bin"
            write_new(directory, name, payload)
            manifest['files'].append({'path': metadata['path'], 'slot': metadata['slot'],
                                      'file': name, 'size': len(payload),
                                      'crc32': metadata['crc32'],
                                      'sha256': hashlib.sha256(payload).hexdigest()})
            checkpoint(name)
        encoded = json.dumps(manifest, sort_keys=True, indent=2).encode() + b'\n'
        cv.require(len(encoded) <= MANIFEST_LIMIT, 'manifest bound exceeded')
        write_new(directory, '.manifest.pending', encoded)
        checkpoint('manifest-staged')
        # Hard-link publication is atomic and cannot replace an existing name.
        os.link('.manifest.pending', 'manifest.json', src_dir_fd=directory,
                dst_dir_fd=directory, follow_symlinks=False)
        os.fsync(directory)
        checkpoint('manifest-published')
        os.unlink('.manifest.pending', dir_fd=directory)
        os.fsync(directory)
        # Completion is manifest present AND marker absent. A crash before this
        # point conservatively leaves a rejected partial export, never repairs it.
        os.unlink('INCOMPLETE', dir_fd=directory)
        os.fsync(directory)
        os.fsync(parent)
        return manifest
    finally:
        if directory is not None:
            os.close(directory)
        os.close(parent)
        # Failures retain incomplete evidence; never recursively delete unknown data.


def read_regular(directory, name, maximum):
    descriptor = os.open(name, os.O_RDONLY | os.O_NONBLOCK | NOFOLLOW, dir_fd=directory)
    with os.fdopen(descriptor, 'rb') as stream:
        info = os.fstat(stream.fileno())
        cv.require(stat.S_ISREG(info.st_mode) and info.st_size <= maximum,
                   'export entry is not a bounded regular file')
        data = stream.read(maximum + 1)
        cv.require(len(data) <= maximum, 'export entry grew past bound')
        return data


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        cv.require(key not in result, 'duplicate recovery JSON key')
        result[key] = value
    return result


def hash_string(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def verify_export(destination):
    directory = open_directory(destination)
    try:
        names = set(os.listdir(directory))
        cv.require('INCOMPLETE' not in names and '.manifest.pending' not in names,
                   'incomplete export; never restore')
        data = read_regular(directory, 'manifest.json', MANIFEST_LIMIT)
        try:
            manifest = json.loads(data, object_pairs_hook=unique_object)
        except cv.Invalid:
            raise
        except (ValueError, RecursionError) as error:
            raise cv.Invalid('malformed or excessively nested recovery manifest') from error
        cv.require(isinstance(manifest, dict) and manifest.get('format') == 'NXTCOW01-recovery-export'
                   and type(manifest.get('version')) is int and manifest.get('version') == 1, 'unknown recovery manifest')
        cv.require(set(manifest) == {'format', 'version', 'source_sha256', 'start_lba', 'sectors', 'generation', 'bank', 'files'}, 'unknown recovery fields')
        cv.require(hash_string(manifest['source_sha256']), 'invalid source checksum')
        cv.require(type(manifest['generation']) is int and 1 <= manifest['generation'] <= cv.MAX_U64 and type(manifest['bank']) is int and manifest['bank'] in (0, 1), 'invalid recovery generation/bank')
        cv.require(type(manifest['start_lba']) is int and type(manifest['sectors']) is int and manifest['sectors'] >= cv.REQUIRED, 'invalid recovery geometry')
        cv.extent(manifest['start_lba'], manifest['sectors'])
        files = manifest.get('files')
        cv.require(isinstance(files, list) and len(files) <= 8, 'invalid recovery file count')
        expected, slots, paths = {'manifest.json'}, set(), set()
        for item in files:
            cv.require(isinstance(item, dict) and set(item) == {'path', 'slot', 'file', 'size', 'crc32', 'sha256'}, 'invalid recovery entry')
            cv.require(type(item['crc32']) is int and 0 <= item['crc32'] <= 0xffffffff and hash_string(item['sha256']), 'invalid recovery checksums')
            slot = item.get('slot')
            cv.require(type(slot) is int and 0 <= slot < 8 and slot not in slots,
                       'invalid or duplicate recovery slot')
            name = f'slot-{slot}.bin'
            cv.require(item.get('file') == name, 'unsafe recovery filename')
            path = item.get('path')
            cv.require(isinstance(path, str) and 2 <= len(path) <= 31 and path.startswith('/')
                       and all(p not in ('', '.', '..') and
                               all(c.isascii() and (c.isalnum() or c in '._-') for c in p)
                               for p in path[1:].split('/')) and path not in paths,
                       'invalid canonical recovery path')
            size = item.get('size')
            cv.require(type(size) is int and 0 <= size <= 4096, 'invalid recovery size')
            payload = read_regular(directory, name, 4096)
            cv.require(len(payload) == size and cv.crc(payload) == item.get('crc32')
                       and hashlib.sha256(payload).hexdigest() == item.get('sha256'),
                       'recovery payload checksum mismatch')
            expected.add(name); slots.add(slot); paths.add(path)
        cv.require(names == expected, 'unexpected recovery entries; nothing removed')
        return manifest
    finally:
        os.close(directory)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    create = commands.add_parser('export')
    create.add_argument('image'); create.add_argument('destination')
    create.add_argument('--start-lba', type=lambda s: int(s, 0), required=True)
    create.add_argument('--sectors', type=lambda s: int(s, 0), required=True)
    create.add_argument('--protect', type=cv.protected_range, action='append', required=True)
    create.add_argument('--generation', type=lambda s: int(s, 0))
    check = commands.add_parser('verify'); check.add_argument('destination')
    args = parser.parse_args()
    try:
        if args.operation == 'export':
            report = export(args.image, args.destination, args.start_lba, args.sectors,
                            args.protect, args.generation)
        else:
            report = verify_export(args.destination)
        print(json.dumps(report, indent=2))
    except (cv.Invalid, OSError, ValueError, TypeError) as error:
        print(f'COW recovery rejected: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
