#!/usr/bin/env python3
"""Measure QMP/PS2 input to serial-observed completed software frames in one VM."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import statistics
import subprocess
import tempfile
import time
from udp_responder import Responder
from tcp_responder import Responder as TcpResponder

ROOT = Path(__file__).resolve().parents[1]
FRAME = rb'PRISM FRAME: generation=(\d+)\r?\n'
DESKTOP = rb'PRISM DESKTOP: focused=Some\(([^)]+)\) home=(true|false)[^\r\n]*\r?\n'
STATUS = rb'NEXT: kernel-control=independent boot-info=validated framebuffer=active input=polling\r?\n'


def summarize(samples):
    values = sorted(s['latency_ms'] for s in samples)
    return {'samples': len(values), 'median_ms': statistics.median(values),
            'p95_ms': values[math.ceil(len(values) * 0.95) - 1], 'max_ms': values[-1],
            'p95_definition': 'nearest-rank ceiling(0.95*N), sorted host-observed milliseconds'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path, default=ROOT / 'build/next-dev.img')
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'build/latency')
    parser.add_argument('--cpu', choices=['qemu64', 'max'], default='qemu64')
    parser.add_argument('--cycles', type=int, default=20, help='20..100 cycles per phase; each cycle measures two focus actions and one command')
    parser.add_argument('--timeout', type=float, default=240)
    parser.add_argument('--max-p95-ms', type=float, help='optional published development-VM budget; no hardware rating')
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0 or not 20 <= args.cycles <= 100:
        parser.error('timeout must be finite and positive; cycles must be 20..100')
    if args.max_p95_ms is not None and (not math.isfinite(args.max_p95_ms) or args.max_p95_ms <= 0):
        parser.error('optional p95 budget must be finite and positive')
    image = args.image.resolve();out = args.output_dir.resolve()
    if not image.is_file() or not out.is_relative_to(ROOT / 'build'):
        parser.error('image must exist and outputs must stay below build/')
    out.mkdir(parents=True, exist_ok=True);(ROOT / 'work').mkdir(exist_ok=True)
    original = hashlib.sha256(image.read_bytes()).hexdigest()
    report = {'passed': False, 'image_sha256': original, 'checks': [], 'samples': [],
              'qemu_profile': f'q35/TCG cpu={args.cpu} smp=1 RAM=256MiB software framebuffer PS/2',
              'scope': 'Host wall-clock QMP keyboard action/typed-command Enter to serial-observed completed software frame. Includes QEMU TCG, host scheduling, QMP/serial and instrumentation delays; not kernel IRQ latency, physical hardware certification or guaranteed display/vblank latency.',
              'idle_definition': 'PID1 and its persistent worker remain active; idle means no added noncooperative console children.',
              'phase_order': ['idle', 'loaded'], 'phase_order_limit': 'Fixed same-run order; cache/warmup/drift effects are not isolated.',
              'development_vm_budget_p95_ms': args.max_p95_ms,
              'validator_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    report_path = out / 'latency-smoke.json'
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    env = os.environ.copy();sysroot = Path(env.get('NEXT_SYSROOT', '/workspace/toolchains/sysroot'))
    local = sysroot / 'usr/bin/qemu-system-x86_64'
    qemu = str(local) if local.exists() else shutil.which('qemu-system-x86_64')
    if local.exists():
        libs = str(sysroot / 'usr/lib/x86_64-linux-gnu')
        env['LD_LIBRARY_PATH'] = libs + (':' + env['LD_LIBRARY_PATH'] if env.get('LD_LIBRARY_PATH') else '')
    firmware = sysroot / 'usr/share/OVMF'
    if not firmware.exists():firmware = Path('/usr/share/OVMF')
    deadline = time.monotonic() + args.timeout
    process = serial = qmp = qmp_stream = serial_log = udp = tcp = None
    transcript = bytearray();qmp_id = 0;last_frame = -1

    def remaining():
        left = deadline - time.monotonic()
        if left <= 0:raise TimeoutError('latency benchmark deadline exceeded')
        if process is not None and process.poll() is not None:raise RuntimeError('QEMU exited before benchmark completion')
        return left

    def connect(path):
        while True:
            remaining();channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:channel.connect(str(path));return channel
            except (FileNotFoundError, ConnectionRefusedError):
                channel.close();time.sleep(min(0.05, remaining()))

    def wait(pattern, start=0):
        expression = re.compile(pattern)
        while True:
            match = expression.search(transcript, start)
            if match:return match, time.perf_counter_ns()
            ready, _, _ = select.select([serial], [], [], min(0.05, remaining()))
            if ready:
                chunk = serial.recv(65536)
                if not chunk:raise RuntimeError('serial channel closed')
                transcript.extend(chunk);serial_log.write(chunk);serial_log.flush()
                if any(marker in transcript for marker in [b'HELIX PANIC:', b'HELIX FATAL:', b'NEXT BOOT FAILED:', b'HELIX: NATIVE STARTUP FAILED']):
                    raise RuntimeError('guest fatal/startup error')

    def completed(response):
        nonlocal last_frame
        frame, observed = wait(FRAME, response.end())
        generation = int(frame[1])
        if generation <= last_frame:raise RuntimeError('completed frame generation did not advance')
        last_frame = generation
        return generation, observed, frame

    def qmp_read():
        qmp.settimeout(remaining());line = qmp_stream.readline()
        if not line:raise RuntimeError('QMP channel closed')
        return json.loads(line)

    def qmp_command(command, arguments=None):
        nonlocal qmp_id
        qmp_id += 1;request = {'execute': command, 'id': qmp_id}
        if arguments is not None:request['arguments'] = arguments
        qmp.sendall(json.dumps(request).encode() + b'\n')
        while True:
            response = qmp_read()
            if response.get('id') == qmp_id:
                if 'error' in response:raise RuntimeError(f'QMP {command}: {response["error"]}')
                return response.get('return')

    def key(code):
        # Real QEMU PS/2 make/break events, no serial substitution for benchmark input.
        qmp_command('input-send-event', {'events': [
            {'type': 'key', 'data': {'down': True, 'key': {'type': 'qcode', 'data': code}}},
            {'type': 'key', 'data': {'down': False, 'key': {'type': 'qcode', 'data': code}}}]})

    def control(text, expected):
        start = len(transcript);serial.sendall((text + '\r').encode())
        response, _ = wait(expected, start);completed(response)
        return response

    def focus_setup(code, expected):
        start = len(transcript);key(code)
        response, _ = wait(expected, start);completed(response)

    def measure(phase, cycle, kind, expected, load_pids):
        start = len(transcript);sent = time.perf_counter_ns();key('ret')
        response, response_ns = wait(expected, start)
        generation, observed, frame = completed(response)
        record = {'phase': phase, 'cycle': cycle, 'kind': kind, 'key': 'ret',
                  'sent_monotonic_ns': sent, 'response_observed_monotonic_ns': response_ns,
                  'frame_observed_monotonic_ns': observed, 'latency_ms': (observed - sent) / 1_000_000,
                  'frame_generation': generation, 'serial_response_start': response.start(),
                  'serial_response_end': response.end(), 'serial_frame_start': frame.start(),
                  'load_pids': list(load_pids)}
        report['samples'].append(record)

    def sample_phase(phase, load_pids):
        for cycle in range(args.cycles):
            # Home resets its selection to Terminal. Tab selects Files; Enter focuses it.
            focus_setup('f10', rb'PRISM DESKTOP: focused=Some\([^)]+\) home=true[^\r\n]*\r?\n')
            focus_setup('tab', rb'PRISM DESKTOP: focused=Some\([^)]+\) home=true[^\r\n]*\r?\n')
            measure(phase, cycle, 'focus_files', rb'PRISM DESKTOP: focused=Some\(Files\) home=false[^\r\n]*\r?\n', load_pids)
            focus_setup('f10', rb'PRISM DESKTOP: focused=Some\(Files\) home=true[^\r\n]*\r?\n')
            measure(phase, cycle, 'focus_terminal', rb'PRISM DESKTOP: focused=Some\(Terminal\) home=false[^\r\n]*\r?\n', load_pids)
            typed = []
            for code in 'status':
                sent = time.perf_counter_ns();key(code)
                typed.append({'key': code, 'sent_monotonic_ns': sent, 'qmp_ack_monotonic_ns': time.perf_counter_ns()})
                # Outside the measured Enter interval: bounded human-like input pacing.
                time.sleep(min(0.025, remaining()))
            measure(phase, cycle, 'typed_status_enter', STATUS, load_pids)
            report['samples'][-1]['typing_events'] = typed
        values = [s for s in report['samples'] if s['phase'] == phase]
        report.setdefault('statistics', {})[phase] = {'all': summarize(values),
            'focus': summarize([s for s in values if s['kind'].startswith('focus_')]),
            'typed_command': summarize([s for s in values if s['kind'] == 'typed_status_enter'])}

    def process_states():
        start = len(transcript);serial.sendall(b'ps\r')
        response, _ = wait(rb'NEXT PROCESS: pid=\d+ state=[^\r\n]*\r?\n', start)
        _, observed, frame = completed(response)
        states = {int(m[1]): {'state': m[2].decode(), 'preemptions': int(m[3]), 'console': m[4].decode()}
                  for m in re.finditer(rb'NEXT PROCESS: pid=(\d+) state=(\w+) preemptions=(\d+) console=(true|false)\r?\n', transcript[start:frame.start()])}
        return states, observed

    def terminate(_signal, _frame):raise KeyboardInterrupt('termination requested')
    previous = signal.signal(signal.SIGTERM, terminate)
    try:
        if not qemu:raise RuntimeError('QEMU missing; source tools/env.sh')
        udp = Responder();tcp = TcpResponder()
        with tempfile.TemporaryDirectory(prefix='latency-', dir=ROOT / 'work') as temp, (out / 'qemu.log').open('wb') as log, (out / 'serial.log').open('wb') as serial_output:
            serial_log = serial_output;temp = Path(temp);env['TMPDIR'] = str(temp)
            shutil.copyfile(firmware / 'OVMF_VARS_4M.fd', temp / 'vars.fd')
            cmd = [qemu, '-machine', 'q35,accel=tcg', '-cpu', args.cpu, '-m', '256M', '-smp', '1', '-vga', 'std',
                   '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE_4M.fd"}',
                   '-drive', f'if=pflash,format=raw,file={temp / "vars.fd"}',
                   '-drive', f'if=none,id=nextdisk,format=raw,snapshot=on,file={image}',
                   '-device', 'virtio-blk-pci,drive=nextdisk,disable-modern=on',
                   '-netdev', 'user,id=nextnet', '-device', 'virtio-net-pci,netdev=nextnet,disable-modern=on',
                   '-audiodev', f'wav,id=nextaudio,path={out / "audio.wav"},out.frequency=48000,out.channels=2,out.format=s16',
                   '-device', 'AC97,audiodev=nextaudio', '-display', 'none',
                   '-serial', f'unix:{temp / "serial.sock"},server=on,wait=off',
                   '-qmp', f'unix:{temp / "qmp.sock"},server=on,wait=off', '-no-reboot']
            for data in ['usr/share/qemu', 'usr/share/seabios', 'usr/lib/ipxe/qemu']:
                if (sysroot / data).exists():cmd.extend(['-L', str(sysroot / data)])
            process = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            serial = connect(temp / 'serial.sock');qmp = connect(temp / 'qmp.sock');qmp_stream = qmp.makefile('rb')
            if 'QMP' not in qmp_read():raise RuntimeError('invalid QMP greeting')
            qmp_command('qmp_capabilities');wait(rb'HELIX: NATIVE SYSTEM READY');wait(rb'PRISM: READY')
            wait(rb'NEXTD: service crash restart, heartbeat watchdog recovery and PID1 survival passed')
            spawns = list(re.finditer(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/service-worker\.elf profile=Native', transcript))
            if not spawns:raise RuntimeError('persistent service worker not established')
            worker = int(spawns[-1][1]);report['baseline_pids'] = [1, worker]
            control('status', STATUS)
            sample_phase('idle', [])
            children = []
            for _ in range(2):
                match = control('run /system/spin.elf', rb'NEXT RUN: pid=(\d+)\r?\n');pid = int(match[1])
                if pid in [1, worker, *children]:raise RuntimeError('noncooperative spawn reused a protected/live PID')
                children.append(pid)
            report['load_pids'] = children
            control('run /system/spin.elf', rb'NEXT RUN FAILED: NoMemory\r?\n')
            report['checks'].append('two noncooperative children plus supervisor/worker exhaust four process slots')
            before_states, before_ns = process_states()
            report['loaded_process_states_before'] = before_states
            report['loaded_process_states_before_monotonic_ns'] = before_ns
            sample_phase('loaded', children)
            states, after_ns = process_states()
            report['loaded_process_states_after_monotonic_ns'] = after_ns
            if not all(pid in states and pid in before_states and states[pid]['preemptions'] > before_states[pid]['preemptions'] for pid in children):
                raise RuntimeError('timer preemption was not demonstrated for every load child')
            if not all(pid in states for pid in [1, worker]):raise RuntimeError('protected baseline task disappeared')
            report['loaded_process_states'] = states
            report['checks'].append('every noncooperative load child has actual timer preemptions; baseline tasks remain live')
            for pid in children:
                mark = len(transcript);control(f'kill {pid}', f'NEXT KILL: pid={pid}\r?\n'.encode())
                exited, _ = wait(f'HELIX: PROCESS EXIT pid={pid} status=-13 preemptions=(\\d+)\\r?\\n'.encode(), mark)
                if int(exited[1]) == 0:raise RuntimeError('load child exited without timer preemption')
            for group in ['all', 'focus', 'typed_command']:
                idle = report['statistics']['idle'][group];loaded = report['statistics']['loaded'][group]
                report.setdefault('loaded_to_idle_ratio', {})[group] = {'median': loaded['median_ms']/idle['median_ms'], 'p95': loaded['p95_ms']/idle['p95_ms']}
            report['checks'].append('real PS/2 focus and typed-command actions observed through strictly subsequent completed frame generations')
            if args.max_p95_ms is not None:
                report['development_vm_budget_met'] = all(report['statistics']['loaded'][g]['p95_ms'] <= args.max_p95_ms for g in ['focus', 'typed_command'])
                if not report['development_vm_budget_met']:raise RuntimeError('optional published development-VM p95 budget exceeded')
            report['passed'] = True
    except (Exception, KeyboardInterrupt) as error:
        report['error'] = str(error)
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:process.wait(timeout=3)
            except subprocess.TimeoutExpired:os.killpg(process.pid, signal.SIGKILL);process.wait(timeout=3)
        if qmp_stream is not None:qmp_stream.close()
        for channel in [serial, qmp]:
            if channel is not None:channel.close()
        for peer in [udp, tcp]:
            if peer is not None:peer.close()
        signal.signal(signal.SIGTERM, previous)
        report['source_image_unchanged'] = hashlib.sha256(image.read_bytes()).hexdigest() == original
        if not report['source_image_unchanged']:report['passed'] = False;report['error'] = 'source image changed'
        report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'samples': len(report['samples']), 'statistics': report.get('statistics'), 'error': report.get('error'), 'report': str(report_path)}, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':raise SystemExit(main())
