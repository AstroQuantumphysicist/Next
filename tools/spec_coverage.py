#!/usr/bin/env python3
"""Render every specification requirement with its scoped evidence and remaining work."""
import argparse
import copy
import hashlib
import html
import json
import os
import tempfile
from pathlib import Path
import unittest
from spec_audit import audit, inventory

ROOT = Path(__file__).resolve().parents[1]


def source_snapshots(tracker, root):
    """Compare recorded source snapshots; drift never promotes a requirement."""
    snapshots = set()
    for row in tracker['requirements']:
        for item in row['evidence']:
            parent = (root / item['artifact']).parent
            while parent.is_relative_to(root) and parent != root:
                candidate = parent / 'source-manifest.json'
                if candidate.is_file():
                    snapshots.add(candidate)
                    break
                parent = parent.parent
    results = []
    for path in sorted(snapshots):
        manifest = json.loads(path.read_text())
        changed, missing = [], []
        for relative, expected in manifest.items():
            source = (root / relative).resolve()
            if not source.is_relative_to(root.resolve()) or not source.is_file():
                missing.append(relative)
            elif hashlib.sha256(source.read_bytes()).hexdigest() != expected:
                changed.append(relative)
        results.append({'artifact': str(path.relative_to(root)),
                        'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'recorded_files': len(manifest), 'changed_files': changed,
                        'missing_files': missing,
                        'matches_current_working_tree': not changed and not missing,
                        'interpretation': 'A mismatch identifies historical evidence. It does not invalidate that historical run, validate current code, or promote requirement status.'})
    return results


def report_data(spec, tracker, root):
    errors, counts, levels = audit(spec, tracker, root)
    if errors:
        raise ValueError('\n'.join(errors))
    expected = inventory(spec)
    rows = copy.deepcopy(tracker['requirements'])
    if {r['id'] for r in rows} != set(expected) or len(rows) != len(expected):
        raise ValueError('coverage inventory does not match complete specification inventory')
    return {'schema': 1, 'spec_sha256': hashlib.sha256(spec.encode()).hexdigest(),
            'tracker_sha256': hashlib.sha256(json.dumps(tracker, sort_keys=True).encode()).hexdigest(),
            'product_ready': tracker['product_ready'], 'requirements_total': len(rows),
            'status': counts, 'evidence_levels': levels, 'note': tracker.get('note', ''),
            'interpretation': 'Partial means bounded progress with remaining acceptance work. Host/VM evidence does not establish certified hardware or full product readiness. This report audits recorded artifacts, not the current unvalidated working tree.',
            'requirements': rows, 'source_snapshots': source_snapshots(tracker, root)}


def render(data, root, output_dir):
    escape = html.escape
    spec_link = html.escape(os.path.relpath(root / 'docs/NEXT-1.0-SPEC.md', output_dir), quote=True)
    rows = []
    for record in data['requirements']:
        evidence = []
        for item in record['evidence']:
            link = os.path.relpath(root / item['artifact'], output_dir)
            evidence.append('<details><summary>' + escape(item['level']) + ' · <a href="' + escape(link, quote=True) + '">' + escape(item['artifact']) + '</a></summary><p>' + escape(item['scope']) + '</p><pre>' + escape(item['command']) + '</pre><p class="hash">SHA256 ' + escape(item['sha256']) + '</p></details>')
        rows.append('<tr id="' + escape(record['id'], quote=True) + '" data-status="' + escape(record['status'], quote=True) + '"><td><a href="#' + escape(record['id'], quote=True) + '">' + escape(record['id']) + '</a><br><small>' + escape(record['kind']) + '</small></td><td>' + escape(record['text']) + '</td><td>' + escape(record['status']) + '</td><td>' + (''.join(evidence) or 'No validation evidence recorded') + '</td><td>' + escape(record['remaining']) + '</td></tr>')
    snapshots = []
    for item in data.get('source_snapshots', []):
        state = 'Recorded files match the current working tree' if item['matches_current_working_tree'] else 'Historical source snapshot differs from the current working tree'
        snapshots.append('<li><strong>' + escape(state) + '</strong>: ' + escape(item['artifact']) + f" ({len(item['changed_files'])} changed, {len(item['missing_files'])} missing of {item['recorded_files']} recorded files)." + '</li>')
    snapshot_html = '<h2>Source versus recorded validation</h2><ul>' + ''.join(snapshots) + '</ul><p>Matching a bounded file manifest alone does not certify a binary/image or unrecorded new files. Requirement status comes from the audited ledger; rebuilding source does not replace a validated milestone.</p>'
    counts = data['status']
    summary = f"{data['requirements_total']} requirements · {counts['pending']} pending · {counts['partial']} partial · {counts['verified']} verified"
    readiness = 'Recorded readiness is true; review all exact acceptance scopes.' if data['product_ready'] else 'Next 1.0 is not ready.'
    return '''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Next specification coverage</title>
<style>body{font:16px/1.5 system-ui,sans-serif;margin:2rem;color:#172033;background:#fafbfc}h1{margin-bottom:.3rem}.notice{padding:1rem;background:#fff3cf;border-left:4px solid #a36a00}label{margin-right:1rem}input,select{padding:.5rem;font:inherit}table{border-collapse:collapse;width:100%;margin-top:1rem;background:white}th,td{text-align:left;vertical-align:top;padding:.8rem;border:1px solid #dde2e8}th{background:#eaf0f6}td:nth-child(2),td:nth-child(5){min-width:16rem}a{color:#174f9d}small,.hash{font-size:.8rem}.hash,pre{overflow-wrap:anywhere;white-space:pre-wrap}summary{cursor:pointer}tr[hidden]{display:none}@media(max-width:900px){.table{overflow-x:auto}}</style>
<h1>Complete specification inventory</h1><p><a href="''' + spec_link + '">Original specification</a> · Explicit identifiers, numbered section gates and final deliverables; narrative sentences are not independently certified.</p><p>' + escape(summary) + '</p><p class="notice"><strong>' + escape(readiness) + '</strong> ' + escape(data['interpretation']) + '</p><p>' + escape(data['note']) + '</p>' + snapshot_html + '<p class="hash">Specification SHA256 ' + escape(data['spec_sha256']) + '</p><label>Search <input id="search" type="search" placeholder="ID, contract, evidence or remaining work"></label><label>Status <select id="status"><option value="">All</option><option>pending</option><option>partial</option><option>verified</option></select></label><p id="visible" aria-live="polite"></p><div class="table"><table><thead><tr><th>Requirement</th><th>Contract</th><th>Status</th><th>Scoped evidence</th><th>Remaining acceptance work</th></tr></thead><tbody>' + ''.join(rows) + '''</tbody></table></div>
<script>const search=document.getElementById('search'),status=document.getElementById('status'),rows=[...document.querySelectorAll('tbody tr')];function filter(){const query=search.value.toLowerCase();let count=0;for(const row of rows){row.hidden=!(row.textContent.toLowerCase().includes(query)&&(!status.value||row.dataset.status===status.value));if(!row.hidden)count++;}document.getElementById('visible').textContent=count+' of '+rows.length+' requirements shown';}search.addEventListener('input',filter);status.addEventListener('change',filter);filter();</script></html>'''


class CoverageTests(unittest.TestCase):
    def setUp(self):
        self.spec = (ROOT / 'docs/NEXT-1.0-SPEC.md').read_text()
        self.tracker = json.loads((ROOT / 'docs/requirements.json').read_text())

    def test_all_contracts_and_remaining_work_survive_roundtrip(self):
        data = report_data(self.spec, self.tracker, ROOT)
        self.assertEqual(len(data['requirements']), len(inventory(self.spec)))
        self.assertEqual(data['requirements'], self.tracker['requirements'])
        self.assertEqual(json.loads(json.dumps(data)), data)

    def test_pending_rows_display_without_invented_evidence(self):
        data = report_data(self.spec, self.tracker, ROOT)
        output = render(data, ROOT, ROOT / 'build/coverage')
        for row in data['requirements']:
            self.assertIn('id="' + row['id'] + '"', output)
        self.assertIn('No validation evidence recorded', output)
        self.assertIn('Next 1.0 is not ready.', output)

    def test_contract_markup_cannot_become_active_html(self):
        data = report_data(self.spec, self.tracker, ROOT)
        data['requirements'][0]['text'] = '<script>alert("contract")</script>'
        output = render(data, ROOT, ROOT / 'build/coverage')
        self.assertNotIn('<script>alert("contract")</script>', output)
        self.assertIn('&lt;script&gt;alert(&quot;contract&quot;)&lt;/script&gt;', output)

    def test_changed_evidence_cannot_generate_a_success_report(self):
        next(e for r in self.tracker['requirements'] for e in r['evidence'])['sha256'] = 'wrong'
        with self.assertRaisesRegex(ValueError, 'hash mismatch'):
            report_data(self.spec, self.tracker, ROOT)

    def test_source_drift_is_historical_and_cannot_promote_status(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'source.rs').write_text('validated source')
            (root / 'evidence').mkdir()
            (root / 'evidence/source-manifest.json').write_text(json.dumps({'source.rs': hashlib.sha256(b'validated source').hexdigest()}))
            tracker = {'requirements': [{'status': 'partial', 'evidence': [{'artifact': 'evidence/run.log'}]}]}
            self.assertTrue(source_snapshots(tracker, root)[0]['matches_current_working_tree'])
            (root / 'source.rs').write_text('new unvalidated feature')
            result = source_snapshots(tracker, root)[0]
            self.assertFalse(result['matches_current_working_tree'])
            self.assertEqual(result['changed_files'], ['source.rs'])
            self.assertEqual(tracker['requirements'][0]['status'], 'partial')
            (root / 'source.rs').unlink()
            self.assertEqual(source_snapshots(tracker, root)[0]['missing_files'], ['source.rs'])

    def test_historical_vm_evidence_cannot_verify_a_new_feature(self):
        self.tracker['requirements'][0]['status'] = 'verified'
        with self.assertRaisesRegex(ValueError, 'verified requires'):
            report_data(self.spec, self.tracker, ROOT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'build/coverage')
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        return 0 if unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(CoverageTests)).wasSuccessful() else 1
    out = args.output_dir.resolve()
    if not out.is_relative_to(ROOT / 'build'):
        parser.error('generated coverage must stay below build/')
    try:
        spec = (ROOT / 'docs/NEXT-1.0-SPEC.md').read_text()
        tracker = json.loads((ROOT / 'docs/requirements.json').read_text())
        data = report_data(spec, tracker, ROOT)
        out.mkdir(parents=True, exist_ok=True)
        (out / 'coverage.json').write_text(json.dumps(data, indent=2) + '\n')
        (out / 'index.html').write_text(render(data, ROOT, out))
    except (OSError, ValueError, KeyError) as error:
        print('Coverage generation failed: ' + str(error))
        return 1
    print(json.dumps({'requirements': data['requirements_total'], 'status': data['status'], 'product_ready': data['product_ready'], 'json': str(out / 'coverage.json'), 'html': str(out / 'index.html')}, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
