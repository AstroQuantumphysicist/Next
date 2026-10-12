import unittest

import boot_smoke


class BootSmokeSmpProfileTests(unittest.TestCase):
    def qmp_cpus(self, count):
        return [{"cpu-index": index, "props": {"apic-id": index}} for index in range(count)]

    def acpi_record(self, count):
        return (
            f"HELIX ACPI PASS cpus={count} io_apics=1 overrides=5 tables=7 bsp=0; discovery only\r\n"
        ).encode()

    def test_supported_profiles_require_exact_qmp_objects_and_madt_processors(self):
        for count in boot_smoke.SMP_PROFILE_CPUS:
            with self.subTest(configured_cpus=count):
                self.assertEqual(boot_smoke.validate_smp_profile_count(count), count)
                self.assertEqual(
                    len(boot_smoke.validate_qmp_cpu_objects(self.qmp_cpus(count), count)),
                    count,
                )
                topology = boot_smoke.validate_acpi_enabled_cpu_count(self.acpi_record(count), count)
                self.assertEqual(topology["enabled_cpus"], count)
                self.assertEqual(topology["bootstrap_apic_id"], 0)

    def test_only_documented_profiles_are_accepted(self):
        for count in (0, 3, 5, 16, True):
            with self.subTest(configured_cpus=count), self.assertRaises(ValueError):
                boot_smoke.validate_smp_profile_count(count)

    def test_qmp_count_and_cpu_indices_must_match_profile(self):
        with self.assertRaisesRegex(RuntimeError, "expected 4"):
            boot_smoke.validate_qmp_cpu_objects(self.qmp_cpus(2), 4)
        with self.assertRaisesRegex(RuntimeError, "indices"):
            boot_smoke.validate_qmp_cpu_objects(
                [{"cpu-index": 0}, {"cpu-index": 1}, {"cpu-index": 1}, {"cpu-index": 3}],
                4,
            )
        with self.assertRaisesRegex(RuntimeError, "integer cpu-index"):
            boot_smoke.validate_qmp_cpu_objects([{}], 1)

    def test_madt_count_mismatch_missing_or_duplicate_records_fail(self):
        with self.assertRaisesRegex(RuntimeError, "reports 2 enabled CPUs, expected 4"):
            boot_smoke.validate_acpi_enabled_cpu_count(self.acpi_record(2), 4)
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            boot_smoke.validate_acpi_enabled_cpu_count(b"", 1)
        with self.assertRaisesRegex(RuntimeError, "found 2"):
            boot_smoke.validate_acpi_enabled_cpu_count(self.acpi_record(1) * 2, 1)

    def test_all_profiles_require_explicit_stopped_ap_posture(self):
        serial = (
            b"HELIX AP RESERVATION PASS base=0x1000 APs remain stopped\r\n"
            b"HELIX AP SCRATCH PASS base=0x2000 APs remain stopped\r\n"
        )
        posture = boot_smoke.validate_bsp_only(serial)
        self.assertFalse(posture["startup_enabled"])
        self.assertEqual(posture["scheduler_cpu_scope"], "bootstrap CPU only")
        for incomplete in (
            serial.replace(b"HELIX AP SCRATCH PASS", b"HELIX AP SCRATCH UNAVAILABLE"),
            serial.replace(b"APs remain stopped", b"AP state unknown", 1),
            serial + b"HELIX AP ONLINE apic_id=1\r\n",
            serial + b"HELIX AP SCHEDULER active\r\n",
        ):
            with self.subTest(serial=incomplete), self.assertRaises(RuntimeError):
                boot_smoke.validate_bsp_only(incomplete)


if __name__ == "__main__":
    unittest.main()
