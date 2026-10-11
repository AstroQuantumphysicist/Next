"""Typed development-VM LAPIC calibration outcome, separate from PIT delivery.

An optional masked calibration can be rejected without stopping the PIT clock.
A rejection never becomes calibration proof. Runtime continuation is checked
independently against real timer, ISR-delivery and noncooperative-process logs.
"""
from dataclasses import asdict, dataclass
import re

KNOWN_ERRORS = frozenset(('Resource', 'Reference', 'Busy', 'Stopped', 'Timeout', 'Unstable', 'Bounds'))
PREFIX = b'HELIX LAPIC TIMER '
PASS = re.compile(
    rb'HELIX LAPIC TIMER PASS divider=(\d+) divided_hz=(\d+) '
    rb'samples=\[Sample \{ reference_ticks: (\d+), countdown_ticks: (\d+) \}, '
    rb'Sample \{ reference_ticks: (\d+), countdown_ticks: (\d+) \}\] '
    rb'count_1ms=(\d+) adopted=(true|false); masked firmware timer left stopped, PIT retained')
UNAVAILABLE = re.compile(rb'HELIX LAPIC TIMER UNAVAILABLE ([A-Za-z]+); PIT retained')

@dataclass(frozen=True)
class Outcome:
    calibration_verified: bool
    reason: str | None
    divider: int | None = None
    divided_hz: int | None = None
    pit_retained: bool = True

    def report(self):
        return asdict(self)


def parse_outcome(serial: bytes, require_calibration: bool = False) -> Outcome:
    lines = [line for line in serial.replace(b'\r', b'').splitlines()
             if line.startswith(PREFIX) and not line.startswith(PREFIX + b'STATE ')]
    if len(lines) != 1:
        raise ValueError('expected exactly one LAPIC calibration outcome')
    line = lines[0]
    match = PASS.fullmatch(line)
    if match:
        values = [int(value) for value in match.groups()[:7]]
        if values[0] != 16 or any(value <= 0 or value > (1 << 64) - 1 for value in values):
            raise ValueError('invalid LAPIC calibration numeric bounds')
        if values[3] > 0xffffffff or values[5] > 0xffffffff or values[6] > 0xffffffff:
            raise ValueError('LAPIC countdown does not fit hardware counter')
        outcome = Outcome(True, None, values[0], values[1])
    else:
        match = UNAVAILABLE.fullmatch(line)
        if not match or match[1].decode('ascii') not in KNOWN_ERRORS:
            raise ValueError('unknown or malformed LAPIC calibration outcome')
        outcome = Outcome(False, match[1].decode('ascii'))
    if require_calibration and not outcome.calibration_verified:
        raise ValueError('required LAPIC calibration unavailable: ' + outcome.reason)
    return outcome


def verify_continuation(serial: bytes) -> dict:
    """Require existing mandatory runtime proofs; calibration cannot supply them."""
    if b'HELIX: TIMER SELFTEST PASS' not in serial:
        raise ValueError('missing actual PIT timer preservation proof')
    routing = re.search(
        rb'HELIX IOAPIC IRQ RESULT Report \{ gsi: (\d+), apic_interrupts: (\d+), '
        rb'pic_interrupts: (\d+), owner_retained: (true|false), result: Ok', serial)
    if not routing or int(routing[2]) < 3 or routing[4] != b'true':
        raise ValueError('missing owned ISR-observed PIT/IOAPIC delivery')
    if b'NEXTD: non-yielding child preempted and killed' not in serial:
        raise ValueError('missing real noncooperative process preemption proof')
    return {'pit_timer_verified': True, 'ioapic_interrupts': int(routing[2]),
            'preemption_verified': True}
