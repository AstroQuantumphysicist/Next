"""Host-only rejection tests for guest accounting evidence, no QEMU substitute."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from cpu_budget_smoke import SOURCES, validate_cycle, validate_final, verify_sources

class BudgetEvidence(unittest.TestCase):
    def test_cross_cycle_and_final_contradictions_reject(self):
        previous={'occupied_ns':110_000_000,'refill_at_ns':500_000_000}
        validate_cycle(previous,215_000_000,700_000_000)
        for values in ((90_000_000,700_000_000),(110_000_000,700_000_000),
                       (215_000_000,500_000_000),(215_000_000,0),
                       (1<<64,700_000_000)):
            with self.subTest(values=values),self.assertRaises(RuntimeError):
                validate_cycle(previous,*values)
        cycles=[previous,{'occupied_ns':215_000_000,'refill_at_ns':700_000_000},
                {'occupied_ns':320_000_000,'refill_at_ns':900_000_000}]
        accepted=(330_000_000,5_000_000,20,3,3,3)
        self.assertEqual(validate_final(cycles,accepted)['debt_ns'],5_000_000)
        for values in ((319_000_000,0,20,3,3,3),(330_000_000,330_000_001,20,3,3,3),
                       (330_000_000,0,2,1,3,3),(330_000_000,0,20,21,3,3),
                       (330_000_000,0,20,3,2,3),(330_000_000,0,20,3,3,2),
                       (330_000_000,0,20,3,3,4),(1<<64,0,20,3,3,3)):
            with self.subTest(values=values),self.assertRaises(RuntimeError):
                validate_final(cycles,values)

    def test_retained_producer_mutation_and_missing_binding_reject(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);manifest=root/'source-manifest.json';sources={}
            for name in SOURCES:
                p=root/'frozen-src'/name;p.parent.mkdir(parents=True,exist_ok=True)
                p.write_bytes(name.encode());sources[name]=hashlib.sha256(p.read_bytes()).hexdigest()
            manifest.write_text(json.dumps(sources));self.assertEqual(verify_sources(manifest),sources)
            p=root/'frozen-src'/SOURCES[0];p.write_bytes(b'changed after initial check')
            with self.assertRaises(RuntimeError):verify_sources(manifest)
            p.write_bytes(SOURCES[0].encode());del sources[SOURCES[-1]]
            manifest.write_text(json.dumps(sources))
            with self.assertRaises(RuntimeError):verify_sources(manifest)

if __name__=='__main__':unittest.main()
