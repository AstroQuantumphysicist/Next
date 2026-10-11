#!/usr/bin/env python3
"""Source-bound HPET dispatch-occupancy budget guest gate, single-CPU QEMU.

Actual invocation is required for proof. HPET occupancy is charged across dispatch;
PIT delivery overshoot is debt, not a nanosecond scheduler or real-time guarantee.
"""
import hashlib
import json
from pathlib import Path
import re
import sys
import surface_capability_smoke as surface

PROFILE = 'HELIX: QUOTA PROFILE HPET64 dispatch occupancy ordinary=100ms/200ms slice<=20ms; PIT delivery overshoot debited; authenticated supervisor/services protected; no RT/SMP/group quotas'
SOURCES = ('crates/helix-core/src/budget.rs', 'crates/helix-kernel/src/native.rs',
           'crates/helix-kernel/src/hpet.rs', 'crates/next-user/src/bin/spin.rs')


def bounded(values):
    result = tuple(int(value) for value in values)
    if any(value < 0 or value > (1 << 64)-1 for value in result):
        raise RuntimeError('budget diagnostic integer is outside u64')
    return result


def validate_cycle(previous, occupied, refill):
    bounded((occupied, refill))
    if occupied < 100_000_000 or refill == 0 or (previous and
            (occupied <= previous['occupied_ns'] or refill <= previous['refill_at_ns'])):
        raise RuntimeError('quota throttle occupancy/refill is not positive monotonic measured accounting')


def validate_final(cycles, values):
    occupied, debt, dispatches, overruns, throttles, resumes = bounded(values)
    if (occupied < cycles[-1]['occupied_ns'] or debt > occupied or dispatches < len(cycles)
            or overruns > dispatches or throttles < len(cycles) or resumes < len(cycles)
            or resumes > throttles):
        raise RuntimeError('final budget metrics contradict actual dispatch/throttle/refill observations')
    return dict(zip(('occupied_ns','debt_ns','dispatches','overruns','throttles','resumes'),
                    (occupied,debt,dispatches,overruns,throttles,resumes)))


def verify_sources(manifest):
    sources = json.loads(manifest.read_text())
    frozen = manifest.parent / 'frozen-src'
    for name in SOURCES:
        if name not in sources or hashlib.sha256((frozen/name).read_bytes()).hexdigest() != sources[name]:
            raise RuntimeError('CPU budget producer/fixture is not bound to retained compiled source: ' + name)
    return sources


def exercise(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report, original):
    binding = surface.BINDING
    manifest = Path(binding['manifest_path'])
    sources = verify_sources(manifest)
    wait_regex(re.escape(PROFILE.encode()) + rb'\r?\n')
    if b'HELIX: QUOTA UNAVAILABLE' in data or b'HELIX: QUOTA CLOCK ERROR' in data:
        raise RuntimeError('HPET accounting unavailable; PIT-only fallback is not budget proof')
    workers = list(re.finditer(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/service-worker\.elf profile=Native', data))
    if len(workers) < 3:
        raise RuntimeError('authenticated persistent service recovery baseline missing')
    worker = int(workers[-1][1])

    def command(text, pattern):
        start = len(data)
        serial.sendall((text+'\r').encode('ascii'))
        return wait_regex(pattern + rb'\r?\n', start)

    command(f'kill {worker}', rb'NEXT KILL FAILED: AccessDenied')
    pids, observations = [], []
    for _ in range(2):
        start = len(data)
        spawned = command('run /system/spin.elf', rb'NEXT RUN: pid=(\d+)')
        pid = int(spawned[1])
        if pid in pids or pid <= 1 or pid == worker:
            raise RuntimeError('budget fixture PID is not a fresh ordinary process')
        identity = wait_regex(rb'HELIX: APPLICATION IDENTITY pid='+str(pid).encode()+rb' package=([^ ]+) signer=[^\r\n]+\r?\n', start)
        if identity[1] != b'org.next.development.native.spin':
            raise RuntimeError('nonyield fixture identity differs from signed spin package')
        pids.append(pid)
    command('run /system/spin.elf', rb'NEXT RUN FAILED: NoMemory')
    # Two always-ready tasks can each stay below their50% individual quota.
    # Capacity is a separate assertion; retire the second before demanding
    # sustained throttling from the sole ordinary CPU-bound task.
    capacity_pids = list(pids)
    retired = pids.pop()
    command(f'kill {retired}', rb'NEXT KILL: pid='+str(retired).encode())
    command(f'kill {retired}', rb'NEXT KILL FAILED: NotFound')
    for pid in pids:
        tag = str(pid).encode()
        throttles = []
        mark = 0
        for count in (1, 2, 3):
            t = wait_regex(rb'HELIX: QUOTA THROTTLE pid='+tag+rb' occupied_ns=(\d+) refill_at=(\d+) count='+str(count).encode()+rb'\r?\n', mark)
            occupied, refill = bounded(t.groups())
            validate_cycle(throttles[-1] if throttles else None, occupied, refill)
            resumed = wait_regex(rb'HELIX: QUOTA RESUME pid='+tag+rb' count='+str(count).encode()+rb'\r?\n', t.end())
            throttles.append({'count':count,'occupied_ns':occupied,'refill_at_ns':refill,
                              'throttle_serial_offset':t.start(),'resume_serial_offset':resumed.start()})
            mark = resumed.end()
        observations.append({'pid':pid,'throttle_resume_cycles':throttles})
    command('status', rb'NEXT: kernel-control=independent[^\r\n]*')
    command('ps', rb'NEXT PROCESS: pid=1 state=[^\r\n]*')
    command('ps', rb'NEXT PROCESS: pid='+str(worker).encode()+rb' state=[^\r\n]*')
    for observation in observations:
        pid = observation['pid'];start=len(data)
        command(f'kill {pid}', rb'NEXT KILL: pid='+str(pid).encode())
        exited = wait_regex(rb'HELIX: PROCESS EXIT pid='+str(pid).encode()+rb' status=-\d+ preemptions=(\d+)\r?\n', start)
        if int(exited[1]) < 3:
            raise RuntimeError('nonyielding budget fixture lacked actual timer preemption')
        observation['preemptions']=int(exited[1])
        final = wait_regex(rb'HELIX: QUOTA FINISH pid='+str(pid).encode()+rb' occupied_ns=(\d+) debt_ns=(\d+) dispatches=(\d+) overruns=(\d+) throttles=(\d+) resumes=(\d+)\r?\n', start)
        observation['final_accounting'] = validate_final(observation['throttle_resume_cycles'], final.groups())
        command(f'kill {pid}', rb'NEXT KILL FAILED: NotFound')
    if re.search(rb'HELIX: QUOTA (?:ERROR|CLOCK ERROR)', data):
        raise RuntimeError('actual budget clock/accounting failed')
    report['cpu_budget']={'producer_sha256':{name:sources[name] for name in SOURCES},
        'persistent_worker_pid':worker,'profile':{'period_ns':200_000_000,'quota_ns':100_000_000,'slice_ns':20_000_000,'clock':'validated HPET64 dispatch occupancy'},
        'capacity_probe_pids':capacity_pids,'ordinary_processes':observations,'limits':'PIT delivery may overshoot; occupancy is measured, not hard real-time scheduling or SMP/group quotas. Deadline/refill markers alone do not certify long-run CPU-share precision.'}
    report['checks'].extend(['sole signed nonyield client executes three actual HPET budget throttle/refill cycles after separate two-child capacity check',
        'sole sustained ordinary client timer-preempted and kill/reap rejects stale PID while authenticated worker and PID1 remain live'])
    result = original(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)
    verify_sources(manifest)
    return result


def main():
    if not any(arg in ('--help', '-h', '--self-test') for arg in sys.argv):
        manifest_arg = None
        for index, argument in enumerate(sys.argv[1:], 1):
            if argument == '--source-manifest' and index + 1 < len(sys.argv):
                manifest_arg = sys.argv[index + 1]
            elif argument.startswith('--source-manifest='):
                manifest_arg = argument.split('=', 1)[1]
        if manifest_arg is None:
            raise SystemExit('CPU budget gate requires retained --source-manifest before QEMU')
        verify_sources(Path(manifest_arg).resolve())
    surface.exercise_surface=exercise
    if not any(arg=='--output-dir' or arg.startswith('--output-dir=') for arg in sys.argv):
        sys.argv += ['--output-dir',str(surface.ROOT/'build/cpu-budget')]
    return surface.main()

if __name__=='__main__':
    sys.exit(main())
