"""Offline exact collector-function tests; importing the collector starts runtime I/O."""
import ast
import collections
import datetime
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
MAIN = 'a' * 40
ANCESTOR = 'b' * 40
TASK = {'id': 'I01', 'title': 'Bounded fixture',
        'completion_criterion': 'The task passes its protected test cases.',
        'acceptance_cases': ['ordinary input', 'overflow input']}


def collector_functions():
    source = ast.parse((ROOT / 'dashboard/collector.py').read_text())
    names = {'audit_failure_verdicts', 'joil_validator', 'jope_evidence_bytes',
             'jope_receipt_proof', 'jope_receipts', 'collect_jopedime_legacy', 'compute_stages', 'bucket'}
    nodes = [node for node in source.body if isinstance(node, ast.FunctionDef) and node.name in names]
    namespace = dict(collections=collections, datetime=datetime, glob=glob, hashlib=hashlib,
                     json=json, os=os, re=re, stat=stat,
                     STAGES=['done', 'on_main', 'in_pr', 'implemented', 'in_progress', 'blocked', 'not_started'],
                     STAGE_LABEL={}, RANK={'not_started': 0, 'blocked': 1, 'in_progress': 2,
                                         'implemented': 3, 'in_pr': 4, 'on_main': 5, 'done': 6}, REPO_DIR={})
    exec(compile(ast.Module(body=nodes, type_ignores=[]), 'collector-offline-functions', 'exec'), namespace)
    return namespace


class StageEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='admiralty-stage-evidence-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'verify').mkdir()
        self.ns = collector_functions()
        self.ns.update(HOME=str(self.root), JE=str(self.root), JP=str(self.root / 'packet'),
                       JCTL=str(self.root), rj=self.read_json, run=self.git,
                       mtime_iso=lambda _: '2026-10-18T12:00:00+00:00',
                       pr_lookup=lambda *_: {}, tail_lines=lambda *_: [], needs_jepson=lambda *_: [],
                       merge_recent=lambda a, b: a + b, track=lambda *_: [])

    @staticmethod
    def read_json(path):
        try:
            return json.loads(Path(path).read_text())
        except (OSError, ValueError):
            return None

    @staticmethod
    def git(args, timeout=0):
        if 'rev-parse' in args:
            return 0, MAIN + '\n', ''
        if 'merge-base' in args:
            return (0 if args[-2] == ANCESTOR else 1), '', ''
        if 'log' in args:
            return 0, '1\n', ''
        raise AssertionError('Unexpected external command: ' + repr(args))

    def receipt(self, sha=MAIN):
        log = self.root / 'checks.log'
        log.write_text('head_sha: ' + sha + '\nexit_code: 0\nRan 2 tests in 0.01s\nOK\n')
        case = {'command': 'python3 -m unittest task_checks', 'exit_code': 0,
                'log_path': str(log), 'log_sha256': hashlib.sha256(log.read_bytes()).hexdigest()}
        return {'task_id': TASK['id'], 'status': 'PASS', 'sha': sha, 'checkout_sha': sha,
                'verified_at_utc': '2026-10-18T12:00:00Z',
                'completion_criterion': TASK['completion_criterion'],
                'case_to_test_map': [dict(case, acceptance_case=name) for name in TASK['acceptance_cases']]}

    def verify(self, receipt):
        (self.root / 'verify/I01.json').write_text(json.dumps(receipt))
        return self.ns['jope_receipts']({'I01': 'open'}, {'I01': TASK})

    def test_bare_pass_and_live_verified_claim_do_not_establish_done(self):
        self.assertEqual(self.verify({'status': 'PASS', 'sha': MAIN}), ([], []))
        (self.root / 'packet').mkdir()
        (self.root / 'packet/tasks.json').write_text(json.dumps({'tasks': [TASK]}))
        live = {'verified_tasks': [{'task_id': 'I01'}],
                'programme_progress': {'verified_task_ids': ['I01']},
                'receiving_main': {'sha': MAIN}}
        (self.root / 'LIVE_STATUS.json').write_text(json.dumps(live))
        project = self.ns['collect_jopedime_legacy']({}, False)
        self.assertNotIn('error', project)
        self.assertEqual(project['done'], 0)
        self.assertEqual(project['receipts']['unsupported_verified_claims'], ['I01'])
        self.assertEqual(project['live_status']['verified_task_claims'], ['I01'])
        self.ns['compute_stages']('jopedime', project, project['_statuses'], {}, {})
        self.assertEqual(project['stage_by_task']['I01']['stage'], 'on_main')

    def test_valid_packet_matched_receipt_and_ancestor_are_supported(self):
        self.assertEqual(self.verify(self.receipt()), (['I01'], []))
        self.assertEqual(self.verify(self.receipt(ANCESTOR)), (['I01'], []))

    def test_missing_or_invalid_time_never_counts_as_verified(self):
        for value in [None, 'bad time', '2026-10-18T12:00:00', '9999-01-01T00:00:00Z']:
            with self.subTest(value=value):
                receipt = self.receipt()
                receipt['verified_at_utc'] = value
                self.assertEqual(self.verify(receipt), ([], []))
        receipt = self.receipt()
        receipt['verified_at_utc'] = '1970-01-01T00:00:00Z'
        self.assertEqual(self.verify(receipt), ([], ['I01']))

    def test_wrong_task_checkout_criterion_or_unmapped_cases_are_rejected(self):
        for key, value in [('task_id', 'I02'), ('checkout_sha', ANCESTOR),
                           ('completion_criterion', 'An easier criterion'), ('case_to_test_map', [])]:
            with self.subTest(key=key):
                receipt = self.receipt()
                receipt[key] = value
                self.assertEqual(self.verify(receipt)[0], [])
        receipt = self.receipt()
        receipt['case_to_test_map'][0]['acceptance_case'] = 'unrelated case'
        self.assertEqual(self.verify(receipt)[0], [])

    def test_unsuccessful_missing_or_forged_execution_evidence_is_rejected(self):
        for key, value in [('exit_code', False), ('exit_code', '0'), ('exit_code', 1),
                           ('exit_code', None), ('command', 'echo PASS'), ('log_path', '/nonexistent'),
                           ('log_sha256', '0' * 64), ('log_sha256', None)]:
            with self.subTest(key=key, value=value):
                receipt = self.receipt()
                receipt['case_to_test_map'][0][key] = value
                self.assertEqual(self.verify(receipt)[0], [])

    def test_zero_test_log_is_not_successful_case_coverage(self):
        receipt = self.receipt()
        path = self.root / 'checks.log'
        path.write_text('Ran 0 tests in 0.00s\nOK\n')
        for case in receipt['case_to_test_map']:
            case['log_sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertEqual(self.verify(receipt)[0], [])

    def test_contradictory_log_head_and_exit_headers_are_rejected(self):
        for header in ['head_sha: ' + ANCESTOR, 'checkout_sha: ' + ANCESTOR, 'exit_code: 1']:
            with self.subTest(header=header):
                receipt = self.receipt()
                path = self.root / 'checks.log'
                path.write_text(header + '\n--- STDOUT ---\nRan 2 tests in 0.01s\nOK\n')
                for case in receipt['case_to_test_map']:
                    case['log_sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
                self.assertEqual(self.verify(receipt)[0], [])
        receipt = self.receipt()
        path = self.root / 'checks.log'
        path.write_text('head_sha: ' + MAIN + '\nexit_code: 0\n--- STDOUT ---\n'
                        'expected negative control printed exit_code: 1\nRan 2 tests in 0.01s\nOK\n')
        for case in receipt['case_to_test_map']:
            case['log_sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertEqual(self.verify(receipt)[0], ['I01'])

    def test_final_test_failure_is_not_overridden_by_receipt_exit_zero(self):
        failed = 'test result: FAILED. 0 passed; 1 failed; 0 ignored;\n'
        passed = 'test result: ok. 2 passed; 0 failed; 0 ignored;\n'
        for output, expected in [(failed, []), ('Ran 2 tests in 0.01s\nFAILED (failures=1)\n', []),
                                 (failed + passed, ['I01']),
                                 (passed + '```text\n' + failed + '```\n> ' + failed, ['I01'])]:
            with self.subTest(output=output):
                receipt = self.receipt()
                path = self.root / 'checks.log'
                path.write_text('head_sha: ' + MAIN + '\nexit_code: 0\n--- STDOUT ---\n' + output)
                for case in receipt['case_to_test_map']:
                    case['log_sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
                self.assertEqual(self.verify(receipt)[0], expected)

    def test_duplicate_json_and_symlink_or_oversized_evidence_are_rejected(self):
        path = self.root / 'verify/I01.json'
        path.write_text('{"status":"FAIL","status":"PASS","sha":"' + MAIN + '"}')
        self.assertEqual(self.ns['jope_receipts']({'I01': 'open'}, {'I01': TASK}), ([], []))
        receipt = self.receipt()
        original = self.root / 'checks.log'
        link = self.root / 'linked.log'
        link.symlink_to(original)
        receipt['case_to_test_map'][0]['log_path'] = str(link)
        self.assertEqual(self.verify(receipt)[0], [])
        original.write_bytes(b'x' * (8 * 1024 * 1024 + 1))
        self.assertIsNone(self.ns['jope_evidence_bytes'](str(original), 8 * 1024 * 1024))

    def test_aggregate_execution_log_budget_fails_closed(self):
        receipt = self.receipt()
        data = b'x' * (8 * 1024 * 1024)
        digest = hashlib.sha256(data).hexdigest()
        receipt['commands'] = [dict(command='python3 -m unittest task_checks', exit_code=0,
                                    log_path='large-' + str(index), log_sha256=digest)
                               for index in range(5)]
        reader = self.ns['jope_evidence_bytes']
        self.ns['jope_evidence_bytes'] = lambda path, limit: (data if len(data) <= limit else None) if path.startswith('large-') else reader(path, limit)
        self.assertFalse(self.ns['jope_receipt_proof']('I01', receipt, TASK, MAIN))

    def test_blocked_or_audit_held_task_stays_blocked_even_with_active_lane(self):
        self.ns['joil_main_statuses'] = lambda: {'T1': 'done'}
        for active in [{}, {'T1': ['reported active claim']}]:
            with self.subTest(active=bool(active)):
                project = {}
                self.ns['compute_stages']('joil', project, {'T1': 'blocked'}, {}, active)
                self.assertEqual(project['stage_by_task']['T1']['stage'], 'blocked')
                self.assertEqual(project['stage_meta']['blocked_code_stage'], {'done': 1})

    def test_ordinary_completed_main_task_still_promotes_to_done(self):
        self.ns['joil_main_statuses'] = lambda: {'T1': 'done'}
        project = {}
        self.ns['compute_stages']('joil', project, {'T1': 'review'}, {}, {})
        self.assertEqual(project['stage_by_task']['T1']['stage'], 'done')

    def test_current_real_failed_verdicts_and_explicit_negations(self):
        text = '''# LATEST audit
  - Verdict: **FAILED on check 3. Blocking: 1.**
  - Verdict: **FAIL. Blocking: 1.**
  - Verdicts: no FAIL. Blocking: none.
  - Verdicts: no FAIL, no HOLD. Blocking: none.
  - Verdict: 0 FAIL
  - Verdict: FAIL: 0
    - Red logs: guards each FAILED with exit 101, as expected.
Documentation example: "FAILED" means the check needs repair.
> - Verdict: FAILED (quoted historical documentation)
```
- Verdict: FAILED (code example)
```
## Previous
- Verdict: FAILED
'''
        directory = self.root / 'gorkbot/validator'
        directory.mkdir(parents=True)
        (directory / 'LATEST.md').write_text(text)
        self.assertEqual(self.ns['joil_validator']()['fail_mentions'], 2)

    def test_explicit_markdown_verdict_formats_and_zero_counts(self):
        helper = self.ns['audit_failure_verdicts']
        self.assertEqual(helper('**Verdict:** FAILED\n### Verdict: FAIL\n- **Verdict**: FAILED'), 3)
        self.assertEqual(helper('**Verdict:** **FAIL**: 0\nVerdict: zero failures\nVerdict: no FAILED checks'), 0)


if __name__ == '__main__':
    unittest.main()
