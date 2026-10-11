#!/usr/bin/env python3
"""Strict bounded decoder for development-only, post-present Prism semantics.

This validates actual diagnostic records, not accessibility service readiness.
Node names are hex UTF-8; values are intentionally excluded. Geometry is
optional descriptive metadata and never supplies action authority.
"""
import argparse
import json
from pathlib import Path
import re
import unicodedata

PREFIX = b'PRISM A11Y '
BEGIN = re.compile(rb'PRISM A11Y BEGIN: frame=(\d+) generation=(\d+) context=(\d+) token=(\d+) count=(\d+) focused=(\d+)')
NODE = re.compile(rb'PRISM A11Y NODE: id=(\d+) parent=(\d+) role=(Window|Dialog|Button|TextBox|List|ListItem|CheckBox|Status) enabled=([01]) visible=([01]) focused=([01]) selected=([01]) checked=(-1|0|1) busy=([01]) bounds=(none|-?\d+,-?\d+,-?\d+,-?\d+) name_hex=([0-9a-f]+)')
END = re.compile(rb'PRISM A11Y END: frame=(\d+) generation=(\d+) count=(\d+)')


def integer(raw, bits=64, nonzero=False):
    value = int(raw)
    if not (int(nonzero) <= value < 1 << bits):
        raise ValueError('out-of-range snapshot integer')
    return value


def parse(serial):
    """Decode complete transactions; reject any malformed or partial A11Y data.

    Callers scope serial to the requested readback. Repeated dumps may have an
    unchanged frame/generation; neither can move backwards within one VM.
    """
    snapshots = []
    current = None
    for line in serial.splitlines():
        if not line.startswith(PREFIX):
            continue
        if match := BEGIN.fullmatch(line):
            if current is not None:
                raise ValueError('nested or incomplete snapshot')
            frame, generation, context, token, count, focused = match.groups()
            current = dict(frame=integer(frame, nonzero=True),
                           generation=integer(generation, nonzero=True),
                           context=integer(context, 32, True), token=integer(token),
                           count=integer(count, 32, True), focused=integer(focused, 32), nodes=[])
            if current['count'] > 48:
                raise ValueError('snapshot exceeds bounded node capacity')
            if snapshots and any(current[key] < snapshots[-1][key] for key in ('frame', 'generation')):
                raise ValueError('snapshot frame or generation moved backwards')
        elif match := NODE.fullmatch(line):
            if current is None or len(current['nodes']) >= current['count']:
                raise ValueError('node outside declared snapshot extent')
            local, parent, role, enabled, visible, focused, selected, checked, busy, bounds, name = match.groups()
            local, parent = integer(local, 32, True), integer(parent, 32)
            known = {node['id'] for node in current['nodes']}
            if local in known or (not known and (local != 1 or parent)) or (known and parent not in known):
                raise ValueError('duplicate, missing root, or dangling parent')
            try:
                encoded = bytes.fromhex(name.decode('ascii'))
                decoded = encoded.decode('utf-8')
            except (ValueError, UnicodeError) as error:
                raise ValueError('invalid hex UTF-8 name') from error
            if not encoded or len(encoded) > 128 or any(unicodedata.category(c) == 'Cc' for c in decoded):
                raise ValueError('unbounded or control-bearing name')
            geometry = None
            if bounds != b'none':
                geometry = tuple(map(int, bounds.split(b',')))
                if any(not -(1 << 31) <= value < 1 << 31 for value in geometry):
                    raise ValueError('bounds exceed signed geometry representation')
                if geometry[2] < 0 or geometry[3] < 0:
                    raise ValueError('negative geometry extent')
            node = dict(id=local, parent=parent, role=role.decode(), name=decoded,
                        enabled=enabled == b'1', visible=visible == b'1', focused=focused == b'1',
                        selected=selected == b'1', checked=None if checked == b'-1' else checked == b'1',
                        busy=busy == b'1', bounds=geometry)
            if node['focused'] and (not node['enabled'] or not node['visible'] or any(n['focused'] for n in current['nodes'])):
                raise ValueError('focused node lacks visible enabled unique state')
            current['nodes'].append(node)
        elif match := END.fullmatch(line):
            if current is None:
                raise ValueError('end without snapshot')
            identity = tuple(map(int, match.groups()))
            if identity != (current['frame'], current['generation'], current['count']) or len(current['nodes']) != current['count']:
                raise ValueError('snapshot trailer identity or node count differs')
            actual_focus = next((node['id'] for node in current['nodes'] if node['focused']), 0)
            if actual_focus != current['focused']:
                raise ValueError('snapshot focus header differs from nodes')
            snapshots.append(current)
            current = None
        else:
            raise ValueError('unknown or malformed accessibility readback record')
    if current is not None or not snapshots:
        raise ValueError('missing complete accessibility readback')
    return snapshots


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('serial', type=Path)
    args = parser.parse_args()
    print(json.dumps({'scope': 'Development-only trusted snapshot diagnostics; no external transport or speech claim',
                      'snapshots': parse(args.serial.read_bytes())}, indent=2))

if __name__ == '__main__':
    main()
