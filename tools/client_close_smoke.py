#!/usr/bin/env python3
"""Actual signed cooperative-close guest gate; no VM run is implied by syntax.

Requires the retained compiled source manifest and exact image digest. Reuses
mandatory surface/DNS/baseline proofs. Refused/Closing never count
as lifetime teardown: actual exit or explicit trusted kill must clear the full
surface generation. No hardware, isolated compositor or production UX claim.
"""
import hashlib
import json
from pathlib import Path
import re
import sys
import time
import surface_capability_smoke as surface

PACKAGE = 'org.next.development.native.client-close-probe'
FIXTURE = 'crates/next-user/src/bin/client-close-probe.rs'
COLOR = bytes.fromhex('a050a0')


def exercise(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report, original):
    binding = surface.BINDING
    sources = json.loads(Path(binding['manifest_path']).read_text())
    if FIXTURE not in sources:
        raise RuntimeError('retained source manifest lacks the actual signed close fixture')
    output = surface.ROOT / 'build/client-close'
    for index, arg in enumerate(sys.argv[1:]):
        if arg == '--output-dir':
            output = Path(sys.argv[index + 2]).resolve()
        elif arg.startswith('--output-dir='):
            output = Path(arg.split('=', 1)[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    captures, actions, clients = [], [], []

    def line(pattern, start=0):
        return wait_regex(pattern + rb'[^\r\n]*\r?\n', start)

    def completed(event):
        wait_regex(rb'PRISM (?:OUTPUT )?FRAME: generation=(\d+)', event.end())

    def command(text, expected):
        start = len(data)
        serial.sendall((text + '\r').encode())
        event = line(re.escape(expected.encode()), start)
        completed(event)
        return event

    def capture(name, client):
        path = output / (name + '.ppm')
        qmp('screendump', {'filename': str(path), 'format': 'ppm'})
        raw = path.read_bytes()
        width, height, pixels = surface.ppm(raw)
        bx, by, bw, bh = client['button']
        block = None
        # Require a full original64x48 literal region below this authenticated
        # card's own logged close control, not an unrelated8x8 color elsewhere.
        for y in range(max(0, by + bh), min(height - 47, by + 280)):
            start = (y * width + max(0, bx - 260)) * 3
            end = (y * width + min(width, bx + bw + 12)) * 3
            offset = pixels.find(COLOR * 64, start, end)
            while offset >= 0:
                x = (offset // 3) % width
                if offset % 3 == 0 and x + 64 <= width and all(
                    pixels[((y + row) * width + x) * 3:((y + row) * width + x + 64) * 3] == COLOR * 64
                    for row in range(48)
                ):
                    block = [x, y, 64, 48]
                    break
                offset = pixels.find(COLOR * 64, offset + 3, end)
            if block:
                break
        if not block:
            raise RuntimeError('authenticated close card lacks its full literal app pixel region')
        captures.append({'path': str(path.relative_to(surface.ROOT)),
                         'sha256': hashlib.sha256(raw).hexdigest(),
                         'full_origin': client['tag'], 'authenticated_package': PACKAGE,
                         'observed_literal_region': block})
        return width, height, pixels

    def move_click(bounds):
        # A middle-button transition exposes actual coordinates without a left
        # activation gesture. Pure Move has no serial marker in frozen Source12.
        def observed_motion(delta):
            mark = len(data)
            events = [{'type': 'rel', 'data': {'axis': axis, 'value': delta[i]}}
                      for i, axis in enumerate(('x', 'y')) if delta[i]]
            events.append({'type': 'btn', 'data': {'button': 'middle', 'down': True}})
            qmp('input-send-event', {'events': events})
            pointed = wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=4', mark)
            completed(pointed)
            released = len(data)
            qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'middle', 'down': False}}]})
            event = line(rb'PRISM POINTER: [^\r\n]*buttons=0', released)
            completed(event)
            return list(map(int, pointed.groups()))
        target = [bounds[0] + bounds[2] // 2, bounds[1] + bounds[3] // 2]
        position = observed_motion([0, 0])
        for _ in range(64):
            if position == target:
                break
            delta = [max(-90, min(90, target[i] - position[i])) for i in range(2)]
            position = observed_motion(delta)
        else:
            raise RuntimeError('actual PS/2 feedback did not converge to trusted control')
        start = len(data)
        actions.append({'kind': 'real-pointer-click', 'target': target, 'serial_start': start,
                        'host_monotonic_ns': time.monotonic_ns()})
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': True}}]})
        pointer = wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=1', start)
        if list(map(int, pointer.groups())) != target:
            raise RuntimeError('actual PS/2 pointer missed trusted source-bound control')
        release = len(data)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': False}}]})
        event = line(rb'PRISM POINTER: [^\r\n]*buttons=0', release)
        completed(event)
        return start

    def launch():
        start = len(data)
        serial.sendall(b'runfiles /system/client-close-probe.elf\r')
        spawned = wait_regex(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/client-close-probe\.elf profile=Native', start)
        pid = int(spawned[1])
        identity = wait_regex(rb'HELIX: APPLICATION IDENTITY pid=' + str(pid).encode() + rb' package=([^ ]+) signer=', start)
        if identity[1].decode() != PACKAGE:
            raise RuntimeError('unexpected authenticated close-fixture principal')
        wait_text(f'NEXT APP pid={pid}: CLIENT CLOSE PROBE: ready; negotiated v2; unknown Ack features preserved configuration; dirty file held', start)
        frame = line(rb'PRISM SURFACE: slot=(\d+) generation=(\d+) frame=1 pid=' + str(pid).encode() + rb' package=' + PACKAGE.encode(), start)
        client = {'pid': pid, 'slot': int(frame[1]), 'generation': int(frame[2]), 'start': start}
        tag = f"slot={client['slot']} generation={client['generation']} pid={pid}"
        client['tag'] = tag
        button = wait_regex(rb'PRISM CLIENT CLOSE BUTTON: ' + tag.encode() + rb' x=(\d+) y=(\d+) width=(\d+) height=(\d+)', start)
        client['button'] = tuple(map(int, button.groups()))
        completed(button)
        clients.append(client)
        return client

    def status(client, value, start):
        event = line(re.escape(('PRISM CLIENT CLOSE STATUS: ' + client['tag'] + ' status=' + value).encode()), start)
        completed(event)
        return event

    first = launch()
    start = move_click(first['button'])
    wait_text(f"NEXT APP pid={first['pid']}: CLIENT CLOSE PROBE: refused first request; READ-only/foreign role/bad response/ticket/sequence denied; stale reply denied; process continues", start)
    status(first, 'Refused', start)
    if re.search(rb'HELIX: PROCESS EXIT pid=' + str(first['pid']).encode() + rb'\b', data[start:]):
        raise RuntimeError('refused close terminated the client')
    capture('refused-still-visible', first)
    start = move_click(first['button'])
    wait_text(f"NEXT APP pid={first['pid']}: CLIENT CLOSE PROBE: WillClose acknowledged; process remains alive while saving", start)
    status(first, 'Closing', start)
    if re.search(rb'HELIX: PROCESS EXIT pid=' + str(first['pid']).encode() + rb'\b', data[start:]):
        raise RuntimeError('WillClose implicitly removed the saving process')
    capture('closing-save-pending-visible', first)
    wait_text(f"NEXT APP pid={first['pid']}: CLIENT CLOSE PROBE: saved file readback verified; cooperative exit; no automatic kernel termination", start)
    wait_exit(first['pid'], 51, start)
    cleared = line(re.escape(('PRISM SURFACE CLEARED: ' + first['tag']).encode()), start)
    completed(cleared)
    command('cat /native/close-proof', 'NEXT DATA: saved-close')

    command('rm /native/close-proof', 'NEXT DATA: Removed /native/close-proof')
    report['client_close'] = {'signed_package': PACKAGE, 'fixture_sha256': sources[FIXTURE],
        'clients': clients, 'captures': captures, 'actions': actions,
        'limits': 'Frozen Source12 fixture has a500ms save interval and no ignore mode: this gate proves refusal and cooperative save/exit, not deadline/Unresponsive or Force Quit behavior. Trusted kernel Prism and explicit development file grant; no isolated compositor or physical timing certification.'}
    report['checks'].extend([
        'negotiated v2 close preserves legacy head and malformed copyout; actual rights/role/ticket/sequence/replay denials execute in signed fixture',
        'real close click can be refused while process and literal app pixels remain visible',
        'WillClose preserves visible app while actual file save/readback precedes cooperative exit51 and exact-generation cleanup'])
    return original(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report)


def main():
    surface.exercise_surface = exercise
    if not any(arg == '--output-dir' or arg.startswith('--output-dir=') for arg in sys.argv):
        sys.argv += ['--output-dir', str(surface.ROOT / 'build/client-close')]
    return surface.main()

if __name__ == '__main__':
    sys.exit(main())
