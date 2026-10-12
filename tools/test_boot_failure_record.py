import unittest

import boot_smoke


class BootFailureRecordTests(unittest.TestCase):
    def record(self, stage):
        code = boot_smoke.BOOT_FAILURE_CODES[stage]
        return f"NEXT BOOT FAILURE: code=0x{code:08x} stage={stage} action=HALT\r\n".encode()

    def test_all_stable_stage_codes_validate_with_serial_crlf(self):
        for stage in boot_smoke.BOOT_FAILURE_CODES:
            with self.subTest(stage=stage):
                actual = boot_smoke.validate_boot_failure_record(self.record(stage), stage)
                self.assertTrue(actual.endswith(f"stage={stage} action=HALT"))

    def test_duplicate_missing_and_malformed_records_fail_closed(self):
        valid = self.record("kernel")
        for payload in (b"", valid + valid, valid.replace(b"0x", b"0X")):
            with self.subTest(payload=payload[:80]):
                with self.assertRaises(RuntimeError):
                    boot_smoke.validate_boot_failure_record(payload)

    def test_code_stage_disagreement_and_wrong_expected_stage_fail(self):
        mismatch = b"NEXT BOOT FAILURE: code=0x4e580201 stage=initrd action=HALT\n"
        with self.assertRaisesRegex(RuntimeError, "inconsistent"):
            boot_smoke.validate_boot_failure_record(mismatch)
        with self.assertRaisesRegex(RuntimeError, "expected initrd"):
            boot_smoke.validate_boot_failure_record(self.record("kernel"), "initrd")

    def test_unknown_expected_stage_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "unknown boot failure stage"):
            boot_smoke.validate_boot_failure_record(self.record("kernel"), "storage")


if __name__ == "__main__":
    unittest.main()
