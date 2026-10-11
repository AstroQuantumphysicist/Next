#!/usr/bin/env python3
"""Source-bound guest accessibility diagnostics; no screen reader/service claim.

Requires the same immutable manifest/image arguments as the native surface gate.
Runs the mandatory desktop/DNS baseline and observes real F9 dumps and actual
PS/2 actions. Never starts QEMU unless explicitly invoked by the VM coordinator.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import time

from accessibility_readback import parse
import prism_files_appearance_smoke as files
import surface_capability_smoke as surface


def exercise(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report, original):
    manifest = Path(surface.BINDING['manifest_path'])
    sources = json.loads(manifest.read_text())
    frozen = manifest.parent / 'frozen-src'
    required = {
        'crates/helix-kernel/src/main.rs': ('fn dump_accessibility()', 'PRISM A11Y END:'),
        'crates/helix-kernel/src/input.rs': ('InspectAccessibility', 'diagnostic_down'),
        'crates/prism/src/accessibility_shell.rs': ('write_accessibility_snapshot',),
    }
    for name, markers in required.items():
        retained = frozen / name
        if name not in sources or hashlib.sha256(retained.read_bytes()).hexdigest() != sources[name]:
            raise RuntimeError('accessibility gate requires exact retained compiled producer source')
        if any(marker not in retained.read_text() for marker in markers):
            raise RuntimeError('retained image producer lacks the required diagnostic contract')
    output = surface.ROOT / 'build/accessibility'
    for index, arg in enumerate(sys.argv[1:]):
        if arg == '--output-dir': output = Path(sys.argv[index + 2]).resolve()
        elif arg.startswith('--output-dir='): output = Path(arg.split('=', 1)[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    helper, binding = files.ensure_host(manifest, frozen, output / 'host')
    dimensions = wait_regex(rb'PRISM: READY (\d+)x(\d+);')
    width, height = map(int, dimensions.groups())
    geometry = {name: tuple(map(int, values)) for name, *values in
                (line.split() for line in subprocess.check_output([str(helper), 'geometry', str(width), str(height)]).decode().splitlines())}
    observations, captures = [], []

    def dump(label, expected_context):
        mark = len(data)
        qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'f9'}], 'hold-time': 40})
        end = wait_regex(rb'PRISM A11Y END: frame=\d+ generation=\d+ count=\d+\r?\n', mark)
        # F9 deliberately ignores repeats while held. Explicit release avoids
        # relying on send-key's host timer before the next readback request.
        qmp('input-send-event', {'events': [{'type': 'key', 'data': {
            'down': False, 'key': {'type': 'qcode', 'data': 'f9'}}}]})
        decoded = parse(bytes(data[mark:end.end()]))
        if len(decoded) != 1 or decoded[0]['context'] != expected_context:
            raise RuntimeError('F9 readback describes a different trusted active context')
        state = decoded[0]
        previous_frames = re.findall(rb'PRISM (?:OUTPUT )?FRAME: generation=(\d+)', data[:mark])
        if previous_frames and state['frame'] < int(previous_frames[-1]):
            raise RuntimeError('readback refers to a frame older than the completed action')
        for node in state['nodes']:
            if node['bounds'] is not None:
                x, y, w, h = node['bounds']
                if min(x, y, w, h) < 0 or x + w > width or y + h > height:
                    raise RuntimeError('exposed default-viewport control geometry escapes framebuffer')
        state['label'] = label
        state['serial_start'] = mark
        state['host_monotonic_ns'] = time.monotonic_ns()
        observations.append(state)
        # No render/event is required for F9 itself: it observes a completed tree.
        return state

    def capture(label, state):
        path = output / (label + '.ppm')
        qmp('screendump', {'filename': str(path), 'format': 'ppm'})
        raw = path.read_bytes()
        image = surface.ppm(raw)
        if image[:2] != (width, height):
            raise RuntimeError('diagnostic capture framebuffer dimensions changed')
        captures.append({'path': str(path.relative_to(surface.ROOT)), 'sha256': hashlib.sha256(raw).hexdigest(),
                         'snapshot_frame': state['frame'], 'semantic_generation': state['generation']})

    def assert_node(state, name, role, focused=None):
        matches = [node for node in state['nodes'] if node['name'] == name and node['role'] == role]
        if len(matches) != 1 or focused is not None and matches[0]['focused'] != focused:
            raise RuntimeError(f'active semantics lack exact expected {role}: {name}')
        return matches[0]

    portal_observed = False
    def baseline_qmp(command, arguments=None):
        nonlocal portal_observed
        if command == 'send-key' and not portal_observed:
            modals = list(re.finditer(rb'PRISM PORTAL: request=(\d+) pid=(\d+) access=1', data))
            if modals:
                portal_observed = True
                wait_regex(rb'PRISM (?:OUTPUT )?FRAME: generation=\d+', modals[-1].end())
                state = dump('trusted-file-portal', 20)
                if state['token'] != int(modals[-1][1]):
                    raise RuntimeError('portal semantic identity differs from actual outstanding request')
                assert_node(state, 'File access request', 'Dialog')
                assert_node(state, 'Cancel', 'Button', True)
                assert_node(state, 'Allow read only', 'Button')
                if any(node['role'] == 'Window' for node in state['nodes']):
                    raise RuntimeError('underlying window leaked into active modal semantic tree')
                capture('trusted-file-portal', state)
        return qmp(command, arguments)
    original(serial, data, wait_text, wait_regex, wait_exit, baseline_qmp, remaining, report)
    if not portal_observed:
        raise RuntimeError('baseline never exposed the actual trusted file-consent context')

    def key(code, frame=True):
        mark = len(data)
        qmp('send-key', {'keys': [{'type': 'qcode', 'data': code}], 'hold-time': 40})
        if frame:
            wait_regex(rb'PRISM (?:OUTPUT )?FRAME: generation=\d+', mark)

    def click(bounds):
        target = [bounds[0] + bounds[2] // 2, bounds[1] + bounds[3] // 2]
        def motion(dx, dy):
            mark = len(data)
            events = [{'type': 'rel', 'data': {'axis': axis, 'value': delta}}
                      for axis, delta in (('x', dx), ('y', dy)) if delta]
            events.append({'type': 'btn', 'data': {'button': 'middle', 'down': True}})
            qmp('input-send-event', {'events': events})
            actual = wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=4', mark)
            wait_regex(rb'PRISM FRAME: generation=\d+', actual.end())
            mark = len(data)
            qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'middle', 'down': False}}]})
            release = wait_regex(rb'PRISM POINTER: x=\d+ y=\d+ buttons=0', mark)
            wait_regex(rb'PRISM FRAME: generation=\d+', release.end())
            return list(map(int, actual.groups()))
        position = motion(0, 0)
        for _ in range(64):
            if position == target: break
            delta = [max(-90, min(90, target[i] - position[i])) for i in range(2)]
            position = motion(*delta)
        else: raise RuntimeError('actual PS/2 pointer did not reach source-bound control')
        mark = len(data)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': True}}]})
        pressed = wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=1', mark)
        if list(map(int, pressed.groups())) != target: raise RuntimeError('actual pointer missed control')
        mark = len(data)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': False}}]})
        release = wait_regex(rb'PRISM POINTER: x=\d+ y=\d+ buttons=0', mark)
        wait_regex(rb'PRISM FRAME: generation=\d+', release.end())

    click(geometry['settings_button'])
    settings = dump('settings', 4)
    assert_node(settings, 'Next Settings', 'Window')
    again = dump('settings-readback-no-mutation', 4)
    if any(settings[key] != again[key] for key in ('generation', 'focused', 'nodes', 'token')):
        raise RuntimeError('diagnostic-only F9 altered active semantic state')
    key('tab')
    traversed = dump('settings-tab', 4)
    if traversed['focused'] == settings['focused'] or traversed['generation'] <= settings['generation']:
        raise RuntimeError('actual Settings Tab failed to advance focus/semantic generation')
    capture('settings-tab', traversed)
    click(geometry['files_button'])
    listing = dump('files', 2)
    assert_node(listing, 'Prism Files', 'Window')
    if not any(node['selected'] and node['role'] == 'ListItem' for node in listing['nodes']):
        raise RuntimeError('actual Files selection lacks a selected semantic row')
    key('slash', frame=False)
    key('right')  # Character slash opens query but has no FRAME log.
    query = dump('file-query-modal', 27)
    assert_node(query, 'Find and sort files', 'Dialog')
    assert_node(query, 'File query', 'TextBox', True)
    if any(node['role'] == 'Window' for node in query['nodes']):
        raise RuntimeError('underlying Files window leaked into active query tree')
    key('tab')
    query_tab = dump('file-query-tab', 27)
    if query_tab['focused'] == query['focused']: raise RuntimeError('query Tab did not traverse active controls')
    capture('file-query-modal', query_tab)
    key('esc')
    restored = dump('query-cancel-restores-files', 2)
    if restored['generation'] <= query_tab['generation']: raise RuntimeError('modal cancellation retained stale generation')
    assert_node(restored, 'Prism Files', 'Window')
    # Inherited baseline continues with Terminal snapping after this hook.
    click(geometry['terminal_button'])
    report['accessibility'] = {'snapshots': observations, 'captures': captures, 'host_binding': binding,
                             'scope': 'Active trusted shell contexts only; optional geometry; no app tree, speech, braille or external accessibility service claim'}
    report['checks'].append('actual post-present F9 names/roles/focus/Files selection and exposed bounds; Settings Tab, trusted portal/query modal precedence and cancellation; readback does not mutate context')


def main():
    surface.exercise_surface = exercise
    if not any(arg == '--output-dir' or arg.startswith('--output-dir=') for arg in sys.argv):
        sys.argv.extend(['--output-dir', str(surface.ROOT / 'build/accessibility')])
    return surface.main()

if __name__ == '__main__':
    sys.exit(main())
