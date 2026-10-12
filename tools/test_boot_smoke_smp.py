import unittest

import boot_smoke


class BootSmokeSmpProfileTests(unittest.TestCase):
    def qmp_cpus(self, count):
        return [{"cpu-index": index, "props": {"apic-id": index}} for index in range(count)]

    def acpi_record(self, count):
        return (
            f"HELIX ACPI PASS cpus={count} io_apics=1 overrides=5 tables=7 bsp=0; discovery only\r\n"
        ).encode()

    def debug_path(self, path):
        raw = [ord(character) for character in path]
        raw.extend([0] * (260 - len(raw)))
        return f"Path {{ bytes: [{', '.join(map(str, raw))}], len: {len(path)} }}"

    def files_tab_actions(self):
        parent = self.debug_path("/native")
        alpha = self.debug_path("/native/alpha")
        root = self.debug_path("/native")
        nested = self.debug_path("/native/alpha")
        kinds = [
            ("CreateFolder", f"parent: FolderSnapshot {{ path: {parent}, id: 1, revision: 2 }}, path: {alpha}", "Ok(3)"),
            ("Navigate", f"path: {root}", "Ok(4)"),
            ("Navigate", f"path: {nested}", "Ok(5)"),
            ("Navigate", f"path: {root}", "Ok(6)"),
        ]
        return "".join(
            f"PRISM DIRECTORY ACTION: request={index} kind={kind} {{ {fields} }} result={result}\r\n"
            for index, (kind, fields, result) in enumerate(kinds, start=1)
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

    def test_rtc_anchor_uses_launch_time_even_after_slow_guest_startup(self):
        host_at_launch = 1_791_795_241
        guest_cmos_anchor = host_at_launch + 2
        host_when_prism_is_ready = host_at_launch + 25
        self.assertGreater(abs(guest_cmos_anchor - host_when_prism_is_ready), 20)
        self.assertEqual(
            boot_smoke.validate_guest_rtc_anchor(guest_cmos_anchor, host_at_launch),
            {
                "guest_utc_seconds": guest_cmos_anchor,
                "host_qemu_launch_utc_seconds": host_at_launch,
                "difference_seconds": 2,
            },
        )

    def test_rtc_anchor_rejects_mismatch_at_launch(self):
        with self.assertRaisesRegex(RuntimeError, "at QEMU launch"):
            boot_smoke.validate_guest_rtc_anchor(1_000_021, 1_000_000)

    def test_files_tabs_require_guest_action_paths_in_order(self):
        records = boot_smoke.validate_files_tab_action_sequence(self.files_tab_actions())
        self.assertEqual([record["kind"].split()[0] for record in records],
                         ["CreateFolder", "Navigate", "Navigate", "Navigate"])
        self.assertEqual([record["paths"][-1] for record in records], [
            "/native/alpha", "/native", "/native/alpha", "/native",
        ])

    def test_files_tabs_reject_wrong_path_order_or_failed_guest_action(self):
        good = self.files_tab_actions()
        with self.assertRaisesRegex(RuntimeError, "action 2 path"):
            lines = good.splitlines(keepends=True)
            lines[1] = lines[1].replace(b"len: 7", b"len: 6")
            boot_smoke.validate_files_tab_action_sequence(b"".join(lines))
        with self.assertRaisesRegex(RuntimeError, "failed"):
            boot_smoke.validate_files_tab_action_sequence(good.replace(b"result=Ok(5)", b"result=Err(NotFound)"))
        with self.assertRaisesRegex(RuntimeError, "expected 4"):
            boot_smoke.validate_files_tab_action_sequence(good.rsplit(b"PRISM DIRECTORY ACTION:", 1)[0])

    def test_files_copy_requires_one_successful_commit_with_exact_source_and_target(self):
        source = "/native/alpha/copy-source"
        destination = "/native/copy-source"
        line = (
            f"PRISM FILE COMMIT: request=88 intent=Copy {{ source: Snapshot {{ path: "
            f"{self.debug_path(source)}, revision: 4 }}, destination: {self.debug_path(destination)}, "
            "replace: None } result=Ok(())\r\n"
        ).encode()
        record = boot_smoke.validate_files_copy_commit(line, source, destination)
        self.assertEqual(record["request"], 88)
        self.assertEqual(record["source"], source)
        self.assertEqual(record["destination"], destination)
        with self.assertRaisesRegex(RuntimeError, "expected one"):
            boot_smoke.validate_files_copy_commit(
                line.replace(b"result=Ok(())", b"result=Err(Conflict)"),
                source,
                destination,
            )
        with self.assertRaisesRegex(RuntimeError, "Copy paths"):
            boot_smoke.validate_files_copy_commit(line, source, "/native/wrong")

    def test_files_tab_click_geometry_matches_default_guest_window(self):
        self.assertEqual(
            boot_smoke.files_tab_click_targets(1280, 800),
            {"add": (528, 222), "root": (203, 222), "nested": (413, 222),
             "address": (120, 257), "left_switch": (533, 257), "right_switch": (1009, 257),
             "copy_to_other": (776, 180), "left_first_row": (334, 293)},
        )
        small = boot_smoke.files_tab_click_targets(1024, 768)
        self.assertTrue(all(0 <= x < 1024 and 0 <= y < 768 for x, y in small.values()))
        with self.assertRaises(ValueError):
            boot_smoke.files_tab_click_targets(0, 768)


if __name__ == "__main__":
    unittest.main()
