import unittest
from lapic_outcome import KNOWN_ERRORS, parse_outcome, verify_continuation

PASS = (b'HELIX LAPIC TIMER PASS divider=16 divided_hz=62689958 samples=['
        b'Sample { reference_ticks: 1000033, countdown_ticks: 626922 }, '
        b'Sample { reference_ticks: 1000005, countdown_ticks: 626901 }] '
        b'count_1ms=62690 adopted=true; masked firmware timer left stopped, PIT retained\r\n')
FALLBACK = b'HELIX LAPIC TIMER UNAVAILABLE Unstable; PIT retained\r\n'
CONTINUATION = (b'HELIX: TIMER SELFTEST PASS\r\n'
                b'HELIX IOAPIC IRQ RESULT Report { gsi: 2, apic_interrupts: 3, '
                b'pic_interrupts: 1, owner_retained: true, result: Ok(Delivery {}) }; PIT remains time source\r\n'
                b'NEXTD: non-yielding child preempted and killed\r\n')

class Outcomes(unittest.TestCase):
    def test_calibration_and_fallback_have_distinct_claims(self):
        self.assertTrue(parse_outcome(PASS, True).calibration_verified)
        for reason in KNOWN_ERRORS:
            result = parse_outcome(FALLBACK.replace(b'Unstable', reason.encode()))
            self.assertFalse(result.calibration_verified)
            self.assertEqual(result.reason, reason)
        with self.assertRaisesRegex(ValueError, 'required LAPIC'):
            parse_outcome(FALLBACK, True)

    def test_missing_unknown_duplicate_and_contradictory_outcomes_fail(self):
        for serial in (b'', PASS + FALLBACK, PASS + PASS, FALLBACK + FALLBACK,
                       FALLBACK.replace(b'Unstable', b'Invented'),
                       FALLBACK.replace(b'PIT retained', b'PIT lost'),
                       PASS.replace(b'divider=16', b'divider=32'),
                       PASS.replace(b'count_1ms=62690', b'count_1ms=0'),
                       PASS.replace(b'countdown_ticks: 626922', b'countdown_ticks: 4294967296'),
                       PASS + b'HELIX LAPIC TIMER UNKNOWN outcome\n'):
            with self.subTest(serial=serial), self.assertRaises(ValueError):
                parse_outcome(serial)
        self.assertTrue(parse_outcome(b'HELIX LAPIC TIMER STATE lvt=0\n' + PASS).calibration_verified)

    def test_fallback_requires_independent_real_runtime_continuation(self):
        self.assertTrue(verify_continuation(FALLBACK + CONTINUATION)['preemption_verified'])
        for serial in (FALLBACK, CONTINUATION.replace(b'HELIX: TIMER SELFTEST PASS', b''),
                       CONTINUATION.replace(b'apic_interrupts: 3', b'apic_interrupts: 0'),
                       CONTINUATION.replace(b'owner_retained: true', b'owner_retained: false'),
                       CONTINUATION.replace(b'NEXTD: non-yielding child preempted and killed', b'')):
            with self.subTest(serial=serial), self.assertRaises(ValueError):
                verify_continuation(serial)

if __name__ == '__main__':
    unittest.main()
