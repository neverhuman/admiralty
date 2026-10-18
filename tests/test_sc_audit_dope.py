"""Tests for the DOPE per-task receipt checks in tools/sc_audit.py (run: python3 -m unittest discover -s tests)."""
import hashlib, importlib.util, json, os, sys, tempfile, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('sc_audit', os.path.join(HERE, '..', 'tools', 'sc_audit.py'))
sc = importlib.util.module_from_spec(spec); spec.loader.exec_module(sc)

MAIN = 'a' * 40
OTHER = 'b' * 40
on_main = lambda s: s == MAIN


def sha(p):
    with open(p, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


class Packet:
    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = self.tmp.name
        os.makedirs(self.d + '/verify/main_x/logs')
        self.tasks = []

    def log(self, name, head=MAIN, ec=0, extra=''):
        p = f'{self.d}/verify/main_x/logs/{name}.log'
        open(p, 'w').write(f'$ cmd {name}\nhead_sha: {head}\nexit_code: {ec}\n--- STDOUT ---\nok {name} {extra}\n')
        return p

    def receipt(self, tid, body=None, pin=True, **over):
        lp = self.log(tid)
        cmd = {'cmd': f'pytest {tid}', 'exit_code': 0, 'head_sha': MAIN, 'log_path': lp, 'log_sha256': sha(lp)}
        r = body if body is not None else {'task_id': tid, 'status': 'passed', 'head_sha': MAIN, 'commands': [cmd],
                                            'verified_at_utc': '2026-10-18T12:00:00Z'}
        r.update(over)
        rp = f'{self.d}/verify/main_x/{tid}.json'
        json.dump(r, open(rp, 'w'))
        return rp, (sha(rp) if pin else None)

    def task(self, tid, ref=None, status='done', evidence=None):
        ev = evidence if evidence is not None else ['some prose'] + ([ref[0] + (f'#sha256={ref[1]}' if ref[1] else '')] if ref else [])
        self.tasks.append({'id': tid, 'status': status, 'evidence': ev})

    def write(self):
        json.dump({'tasks': self.tasks}, open(self.d + '/tasks.json', 'w'))
        return self.d


class CheckReceipt(unittest.TestCase):
    def setUp(self):
        self.p = Packet()

    def tearDown(self):
        self.p.tmp.cleanup()

    def check(self, tid, ref):
        return sc.check_dope_receipt(tid, ref[0], ref[1], on_main)[0]

    def test_good_receipt_passes(self):
        self.assertEqual(self.check('A01', self.p.receipt('A01')), [])

    def test_prose_exit_code_rejected(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); r['commands'][0]['exit_code'] = '0 (all tests passed)'; json.dump(r, open(rp, 'w'))
        self.assertIn('exit_code_not_integer', self.check('A01', (rp, None)))

    def test_bool_exit_code_rejected(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); r['commands'][0]['exit_code'] = False; json.dump(r, open(rp, 'w'))
        self.assertIn('exit_code_not_integer', self.check('A01', (rp, None)))

    def test_exit_code_list_ints_ok_and_nonzero_flagged(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); c = r['commands'][0]; del c['exit_code']; c['exit_codes'] = [0, 0]; json.dump(r, open(rp, 'w'))
        self.assertEqual(self.check('A01', (rp, None)), [])
        c['exit_codes'] = [0, 1]; json.dump(r, open(rp, 'w'))
        self.assertIn('nonzero_exit', self.check('A01', (rp, None)))

    def test_missing_log_and_no_log_path(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); os.remove(r['commands'][0]['log_path'])
        self.assertIn('log_missing', self.check('A01', (rp, None)))
        del r['commands'][0]['log_path']; json.dump(r, open(rp, 'w'))
        self.assertIn('no_log_path', self.check('A01', (rp, None)))

    def test_log_overwritten(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); open(r['commands'][0]['log_path'], 'a').write('tampered\n')
        self.assertIn('log_sha256_mismatch', self.check('A01', (rp, None)))

    def test_no_log_sha256(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); r['commands'][0]['log_sha256'] = 'abc123'; json.dump(r, open(rp, 'w'))
        self.assertIn('no_log_sha256', self.check('A01', (rp, None)))

    def test_checkout_sha_differs(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); r['commands'][0]['head_sha'] = OTHER; json.dump(r, open(rp, 'w'))
        self.assertIn('checkout_sha_mismatch', self.check('A01', (rp, None)))

    def test_explicit_checkout_sha_differs(self):
        rp, _ = self.p.receipt('A01', checkout_sha=OTHER)
        self.assertIn('checkout_sha_mismatch', self.check('A01', (rp, None)))

    def test_sha_not_on_main(self):
        lp = self.p.log('A01-other', head=OTHER)
        rp, _ = self.p.receipt('A01', body={'task_id': 'A01', 'status': 'passed', 'head_sha': OTHER, 'commands': [
            {'cmd': 'x', 'exit_code': 0, 'head_sha': OTHER, 'log_path': lp, 'log_sha256': sha(lp)}]})
        self.assertEqual(self.check('A01', (rp, None)), ['sha_not_on_main'])

    def test_truncated_sha(self):
        rp, _ = self.p.receipt('A01', head_sha=MAIN[:9])
        self.assertIn('bad_or_missing_sha', self.check('A01', (rp, None)))

    def test_log_header_disagrees(self):
        rp, _ = self.p.receipt('A01')
        r = json.load(open(rp)); lp = self.p.log('A01', ec=2); r['commands'][0]['log_sha256'] = sha(lp); json.dump(r, open(rp, 'w'))
        self.assertIn('log_exit_code_mismatch', self.check('A01', (rp, None)))
        lp = self.p.log('A01', head=OTHER); r['commands'][0]['log_sha256'] = sha(lp); json.dump(r, open(rp, 'w'))
        self.assertIn('log_names_other_sha', self.check('A01', (rp, None)))

    def test_receipt_edited_after_pin(self):
        rp, h = self.p.receipt('A01')
        open(rp, 'a').write(' ')
        self.assertIn('receipt_sha256_mismatch', self.check('A01', (rp, h)))

    def test_not_passed_and_no_commands(self):
        rp, _ = self.p.receipt('A01', status='failed', commands=[])
        self.assertTrue({'status_not_passed', 'no_commands', 'no_checkout_sha'} <= set(self.check('A01', (rp, None))))

    def test_receipt_for_other_task(self):
        rp, _ = self.p.receipt('A01')
        self.assertEqual(self.check('A02', (rp, None)), ['receipt_not_for_task'])

    def test_shared_receipt_without_entries(self):
        rp, _ = self.p.receipt('SUITE', body={'status': 'passed', 'head_sha': MAIN, 'exit_code': '0', 'covers': ['A01', 'A02']})
        self.assertEqual(self.check('A01', (rp, None)), ['shared_receipt_no_task_entry'])

    def test_shared_receipt_with_per_task_entries(self):
        e = {}
        for t in ('A01', 'A02'):
            lp = self.p.log(t)
            e[t] = {'status': 'passed', 'commands': [{'cmd': t, 'exit_code': 0, 'head_sha': MAIN, 'log_path': lp, 'log_sha256': sha(lp)}]}
        rp, _ = self.p.receipt('SUITE', body={'head_sha': MAIN, 'tasks': e})
        self.assertEqual(self.check('A01', (rp, None)), [])
        self.assertEqual(self.check('A03', (rp, None)), ['shared_receipt_no_task_entry'])

    def test_predates_commit(self):
        rp, _ = self.p.receipt('A01', verified_at_utc='2026-10-18T10:00:00Z')
        import datetime
        ct = lambda s: datetime.datetime(2026, 10, 18, 11, 0, tzinfo=datetime.timezone.utc)
        self.assertIn('receipt_predates_sha', sc.check_dope_receipt('A01', rp, None, on_main, ct)[0])


class AuditDope(unittest.TestCase):
    def setUp(self):
        sc.findings.clear()
        sc.LOGS = dict(sc.LOGS, dope='/nonexistent/COMMS.md')
        self.p = Packet()

    def tearDown(self):
        self.p.tmp.cleanup()

    def test_counts_only_verified(self):
        p = self.p
        p.task('A01', p.receipt('A01'))
        rp, h = p.receipt('A02'); r = json.load(open(rp)); r['commands'][0]['exit_code'] = 'passed'; json.dump(r, open(rp, 'w'))
        p.task('A02', (rp, None))
        p.task('A03')                                   # prose only
        p.task('A04', status='todo')
        shared, _ = p.receipt('A05')                    # A06 cites A05's receipt
        p.task('A05', (shared, None)); p.task('A06', (shared, None))
        res = sc.audit_dope(p.write(), on_main=on_main)
        self.assertEqual(res['ledger_done'], 5)
        self.assertEqual(res['done_recomputed'], 2)
        self.assertEqual(sorted(res['unverified_done']), ['A02', 'A03', 'A06'])
        codes = {f['code'] for f in sc.findings}
        self.assertTrue({'RECEIPT_EXIT_CODE_NOT_INTEGER', 'RECEIPT_NO_RECEIPT', 'RECEIPT_RECEIPT_NOT_FOR_TASK', 'DONE_UNVERIFIED'} <= codes)

    def test_same_log_for_two_tasks_not_counted(self):
        p = self.p
        rp1, _ = p.receipt('A01')
        lp = json.load(open(rp1))['commands'][0]['log_path']
        rp2, _ = p.receipt('A02', body={'task_id': 'A02', 'status': 'passed', 'head_sha': MAIN, 'commands': [
            {'cmd': 'x', 'exit_code': 0, 'head_sha': MAIN, 'log_path': lp, 'log_sha256': sha(lp)}]})
        p.task('A01', (rp1, None)); p.task('A02', (rp2, None))
        res = sc.audit_dope(p.write(), on_main=on_main)
        self.assertEqual(res['done_recomputed'], 0)
        self.assertIn('RECEIPT_LOG_SHARED_ACROSS_TASKS', {f['code'] for f in sc.findings})

    def test_generic_command_reported_but_counted(self):
        p = self.p
        for i in range(sc.DOPE_GENERIC_CMD_MIN):
            t = f'G{i:02d}'
            lp = p.log(t)
            p.task(t, p.receipt(t, body={'task_id': t, 'status': 'passed', 'head_sha': MAIN, 'commands': [
                {'cmd': 'python3 -m unittest generic', 'exit_code': 0, 'head_sha': MAIN, 'log_path': lp, 'log_sha256': sha(lp)}]}))
        res = sc.audit_dope(p.write(), on_main=on_main)
        self.assertEqual(res['done_recomputed'], sc.DOPE_GENERIC_CMD_MIN)
        self.assertIn('RECEIPT_GENERIC_COMMAND', {f['code'] for f in sc.findings})


class GBRedTeam0645(unittest.TestCase):
    """Vacuous / non-test receipts found by the GB DOPE red team on main 402954ed (2026-10-18)."""
    def setUp(self):
        self.p = Packet()

    def tearDown(self):
        self.p.tmp.cleanup()

    def _with_log(self, tid, cmd, body):
        rp, _ = self.p.receipt(tid)
        r = json.load(open(rp)); c = r['commands'][0]
        open(c['log_path'], 'a').write(body)
        c['cmd'] = cmd; c['log_sha256'] = sha(c['log_path']); json.dump(r, open(rp, 'w'))
        return sc.check_dope_receipt(tid, rp, None, on_main)[0]

    def test_cargo_filter_matching_zero_tests_is_vacuous(self):
        probs = self._with_log('D00', 'cargo test --locked --offline --lib prepared_view -- --nocapture',
                               'running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 278 filtered out; finished in 0.00s\n')
        self.assertIn('vacuous_zero_tests', probs)

    def test_cargo_with_real_tests_ok(self):
        probs = self._with_log('R04', 'cargo test --locked --offline --lib teacher_study -- --nocapture',
                               'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\ntest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 273 filtered out\n')
        self.assertEqual(probs, [])

    def test_unittest_ran_zero(self):
        self.assertIn('vacuous_zero_tests', self._with_log('X01', 'PYTHONPATH=. python3 -m unittest x -v', 'Ran 0 tests in 0.000s\n\nOK\n'))

    def test_sha256sum_without_args_is_not_a_test_and_empty_hash(self):
        probs = self._with_log('P00', 'sha256sum ', 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  -\n')
        self.assertIn('not_a_test_command', probs)
        self.assertIn('empty_input_hash', probs)

    def test_git_log_and_gh_run_list_not_tests(self):
        self.assertIn('not_a_test_command', self._with_log('I00', 'git -C /x log -n 70 --oneline abc', ''))
        self.assertIn('not_a_test_command', self._with_log('V03', 'gh run list --repo a/b --branch main -L 2', ''))

    def test_env_prefix_and_wrappers_skipped(self):
        self.assertEqual(sc.dope_cmd_program('PYTHONPATH=. taskset -c 1-2 nice -n 5 python3 -m unittest t'), 'python3')
        self.assertEqual(sc.dope_cmd_program('A=1 B=2 sha256sum f'), 'sha256sum')


if __name__ == '__main__':
    unittest.main()
