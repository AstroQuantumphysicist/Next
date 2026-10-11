#!/usr/bin/env python3
"""Audit the specification inventory and evidence; no implementation is inferred."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
LEVELS = ('host_tested', 'boot_smoke', 'integrated', 'certified_hardware')

def inventory(spec):
    result = {}
    for match in re.finditer(r'^.*`(NEXT-[A-Z]+-\d+)`: (.*)$', spec, re.M):
        identifier, text = match.groups()
        if identifier in result:
            raise ValueError('duplicate specification identifier: ' + identifier)
        result[identifier] = ('explicit', text.rstrip(' '))
    for number, title in re.findall(r'^#{1,2} (\d+(?:\.\d+)?)\.?(?: )(.+)$', spec, re.M):
        result['SECTION-' + number] = ('section', title)
    contract = spec.split('## 28.2 Indispensable final-version capabilities\n', 1)[1].split('## 28.3', 1)[0]
    for line in contract.splitlines():
        if line.startswith('| ') and not line.startswith('| Product dimension'):
            title, outcome = (part.strip() for part in line.strip('|').split('|'))
            slug = re.sub(r'[^a-z0-9]+', '-', title.lower()).strip('-')
            result['DELIVERABLE-' + slug] = ('final_deliverable', title + ': ' + outcome)
    return result

def new_tracker(spec):
    return {'schema': 1, 'spec_sha256': hashlib.sha256(spec.encode()).hexdigest(),
            'product_ready': False,
            'note': 'Foundations are in progress. No final product capability is established by scaffolding or a host test.',
            'requirements': [{'id': key, 'kind': kind, 'text': text, 'status': 'pending',
                              'evidence': [], 'remaining': 'Implement and validate the complete specification contract.'}
                             for key, (kind, text) in inventory(spec).items()]}

def audit(spec, tracker, root):
    expected = inventory(spec)
    errors = []
    if tracker.get('schema') != 1:
        errors.append('unsupported tracking schema')
    if tracker.get('spec_sha256') != hashlib.sha256(spec.encode()).hexdigest():
        errors.append('specification hash mismatch')
    records = tracker.get('requirements', [])
    actual = [record.get('id') for record in records]
    if len(actual) != len(set(actual)):
        errors.append('duplicate tracking identifiers')
    for identifier in sorted(set(expected) - set(actual)):
        errors.append('missing requirement: ' + identifier)
    for identifier in sorted(set(actual) - set(expected)):
        errors.append('invented requirement: ' + str(identifier))
    counts = {'pending': 0, 'partial': 0, 'verified': 0}
    layers = dict.fromkeys(LEVELS, 0)
    for record in records:
        identifier = record.get('id')
        if identifier not in expected:
            continue
        kind, text = expected[identifier]
        if (record.get('kind'), record.get('text')) != (kind, text):
            errors.append(identifier + ': specification text or kind differs')
        status = record.get('status')
        if status not in counts:
            errors.append(identifier + ': invalid status')
            continue
        counts[status] += 1
        evidence = record.get('evidence', [])
        observed = set()
        for item in evidence:
            level = item.get('level')
            if level not in LEVELS:
                errors.append(identifier + ': unknown evidence level')
                continue
            observed.add(level)
            if not item.get('command') or not item.get('scope'):
                errors.append(identifier + ': evidence needs command and bounded scope')
            artifact = item.get('artifact', '')
            path = (root / artifact).resolve()
            if not artifact or not path.is_relative_to(root.resolve()) or not path.is_file():
                errors.append(identifier + ': evidence artifact missing or outside repository')
            elif item.get('sha256') != hashlib.sha256(path.read_bytes()).hexdigest():
                errors.append(identifier + ': evidence artifact hash mismatch')
        for level in observed:
            layers[level] += 1
        if status == 'pending' and evidence:
            errors.append(identifier + ': pending status cannot claim validation evidence')
        if status == 'partial' and (not evidence or not record.get('remaining')):
            errors.append(identifier + ': partial requires evidence and remaining work')
        if status == 'verified' and (not {'integrated', 'certified_hardware'} <= observed or not record.get('acceptance')):
            errors.append(identifier + ': verified requires integration, real hardware and acceptance evidence')
    ready = bool(records) and counts['verified'] == len(expected) and not errors
    if tracker.get('product_ready') is not ready:
        errors.append('product_ready disagrees with full specification coverage')
    return errors, counts, layers

def render(tracker):
    lines = ['# Next 1.0 requirement status', '',
             'Generated from `docs/requirements.json` by `python3 tools/spec_audit.py --write-report`.', '',
             ('**Next 1.0 acceptance gates are verified.** Review the scoped evidence before release.' if tracker['product_ready'] else '**Next 1.0 is not ready.** Foundations in progress do not establish finished product behavior.'), '',
             'Coverage includes explicit NEXT identifiers, numbered sections/subsections, and the final capability table. '
             'Section rows are acceptance gates, not a claim that every sentence has an independent test.', '',
             'Evidence distinguishes host-tested primitives, boot smoke tests, integrated system behavior, and certified hardware. '
             'Only integrated, hardware-validated contracts with documented acceptance may be verified.', '',
             '| Requirement | Contract | Status | Evidence levels |', '|---|---|---|---|']
    for record in tracker['requirements']:
        text = record['text'].replace('|', '\\|').replace('\n', ' ')
        levels = ', '.join(sorted({item['level'] for item in record['evidence']})) or 'none'
        lines.append(f"| {record['id']} | {text} | {record['status']} | {levels} |")
    return '\n'.join(lines) + '\n'

class AuditTests(unittest.TestCase):
    def setUp(self):
        self.spec = (ROOT / 'docs/NEXT-1.0-SPEC.md').read_text()
        self.tracker = new_tracker(self.spec)
    def errors(self):
        return audit(self.spec, self.tracker, ROOT)[0]
    def test_complete_pending_inventory(self):
        self.assertEqual(self.errors(), [])
        self.assertIn('SECTION-22.5', inventory(self.spec))
        self.assertIn('DELIVERABLE-hardware', inventory(self.spec))
    def test_missing_and_invented_requirements_rejected(self):
        self.tracker['requirements'][0]['id'] = 'NEXT-FAKE-001'
        self.assertTrue(any('missing requirement' in e for e in self.errors()))
        self.assertTrue(any('invented requirement' in e for e in self.errors()))
    def test_unsupported_completion_rejected(self):
        self.tracker['requirements'][0]['status'] = 'verified'
        self.tracker['product_ready'] = True
        self.assertTrue(any('verified requires' in e for e in self.errors()))
        self.assertTrue(any('product_ready' in e for e in self.errors()))
    def test_host_test_does_not_establish_product_completion(self):
        record = self.tracker['requirements'][0]
        record.update(status='partial', evidence=[{'level': 'host_tested', 'command': 'example', 'scope': 'parser only',
                       'artifact': 'docs/NEXT-1.0-SPEC.md', 'sha256': self.tracker['spec_sha256']}])
        self.assertEqual(self.errors(), [])
        record['status'] = 'verified'
        self.assertTrue(any('verified requires' in e for e in self.errors()))
    def test_changed_artifact_and_outside_paths_rejected(self):
        record = self.tracker['requirements'][0]
        record.update(status='partial', evidence=[{'level':'boot_smoke', 'command':'example', 'scope':'boot only',
                       'artifact':'docs/NEXT-1.0-SPEC.md', 'sha256':'wrong'}])
        self.assertTrue(any('hash mismatch' in e for e in self.errors()))
        record['evidence'][0]['artifact'] = '../outside'
        self.assertTrue(any('outside repository' in e for e in self.errors()))
    def test_modified_contract_and_duplicate_rejected(self):
        self.tracker['requirements'][0]['text'] = 'Unsupported replacement claim'
        self.tracker['requirements'].append(copy.deepcopy(self.tracker['requirements'][0]))
        self.assertTrue(any('differs' in e for e in self.errors()))
        self.assertTrue(any('duplicate' in e for e in self.errors()))

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--write-report', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(AuditTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    spec = (ROOT / 'docs/NEXT-1.0-SPEC.md').read_text()
    tracker = json.loads((ROOT / 'docs/requirements.json').read_text())
    errors, counts, layers = audit(spec, tracker, ROOT)
    if errors:
        print('\n'.join(errors), file=sys.stderr)
        return 1
    if args.write_report:
        (ROOT / 'docs/STATUS.md').write_text(render(tracker))
    print(json.dumps({'requirements': len(tracker['requirements']), 'status': counts,
                      'evidence_levels': layers, 'product_ready': tracker['product_ready']}, indent=2))
    return 0

if __name__ == '__main__':
    sys.exit(main())
