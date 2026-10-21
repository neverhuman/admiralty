import datetime, importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)
UTC = datetime.timezone.utc
NOW = datetime.datetime(2026, 10, 18, 15, 12, tzinfo=UTC)
CLAUDE_IDLE = "  No GPU was dispatched.\n Cooked for 36s · done 3:19 PM\n\n ────\n >\n ────\n  ⏵⏵ auto mode on (shift+tab to cycle)\n"
AGY_BUSY = "> URGENT RED 1\n  Running command...\n >\n esc to cancel      Gemini 4 Argon · high\n"

def codes(): return [f['code'] for f in sc.findings]

class Watch(unittest.TestCase):
    def setUp(self): sc.findings.clear()
    def test_claude_footer_without_verified_empty_composer_requires_inspection(self):
        prev = {'watch:jopedime_flywheel': {'hash': sc.tail_hash(CLAUDE_IDLE), 'since': '2026-10-18T14:32:00+00:00'}}
        newh = {}
        w = sc.idle_watch({'jopedime_flywheel': 'x'}, {'jopedime_flywheel': 'jopedime'}, prev, newh, NOW, hc=lambda n, p: CLAUDE_IDLE)
        self.assertFalse(w['jopedime_flywheel']['busy']); self.assertEqual(codes(), ['MANAGED_SCREEN_INPUT_UNVERIFIED'])
        self.assertEqual(w['jopedime_flywheel']['composer_state'], 'unknown')
        self.assertFalse(w['jopedime_flywheel']['idle_eligible'])
        self.assertNotRegex(sc.findings[0]['text'], r'(?i)alt\+m|file pointer \+ CR')
        self.assertTrue(sc.findings[0]['must_act'])
    def test_busy_agy_not_idle_and_first_sighting_not_flagged(self):
        sc.idle_watch({'jopedime_agy': 'x'}, {'jopedime_agy': 'jopedime'}, {}, {}, NOW, hc=lambda n, p: AGY_BUSY)
        sc.idle_watch({'jopedime_flywheel': 'x'}, {'jopedime_flywheel': 'jopedime'}, {}, {}, NOW, hc=lambda n, p: CLAUDE_IDLE)
        self.assertEqual(codes(), [])
    def test_claude_background_monitor_counts_as_busy(self):
        txt = " The five runs are in progress.\n Brewed for 39s · done 3:46 PM · 1 monitor still running\n >\n  auto mode on · 1 monitor · ↓ to manage\n"
        self.assertTrue(sc.tail_busy(txt)); self.assertFalse(sc.tail_busy(CLAUDE_IDLE))
    def test_positive_running_background_ui_footer_is_busy(self):
        for label in ['1 monitor still running', '2 monitors running',
                      '1 background task running', '2 shells still running']:
            with self.subTest(label=label):
                self.assertTrue(sc.tail_busy(f' Brewed for 1m 39s · done 3:46 PM · {label}\n >\n'))
    def test_zero_completed_and_plaintext_counts_do_not_suppress_idle(self):
        for text in [
            'auto mode on · 0 monitors · ↓ to manage',
            'Brewed for 39s · done 3:46 PM · 0 monitors still running',
            'Brewed for 39s · done 3:46 PM · 0 background tasks running',
            'Brewed for 39s · done 3:46 PM · 1 monitor completed',
            'auto mode on · 1 monitor · ↓ to manage',
            'A previous task had 1 monitor running.',
            '1 monitor still running',
            'Documentation: 2 background tasks running',
        ]:
            with self.subTest(text=text):
                self.assertFalse(sc.tail_busy(text + '\n >\n'))
    def test_quoted_or_fenced_busy_examples_are_not_live_ui(self):
        example = 'Brewed for 39s · done 3:46 PM · 1 monitor still running'
        for text in ['> ' + example, '`' + example + '`',
                     '```text\n' + example + '\n```',
                     '~~~text\n' + example + '\n~~~']:
            with self.subTest(text=text):
                self.assertFalse(sc.tail_busy(text + '\n >\n'))
    def test_zero_monitor_unknown_composer_still_requires_action(self):
        text = CLAUDE_IDLE + 'auto mode on · 0 monitors · ↓ to manage\n'
        prev = {'watch:fixture_worker': {'hash': sc.tail_hash(text), 'since': '2026-10-18T14:32:00+00:00'}}
        watched = sc.idle_watch({'fixture_worker': 'x'}, {'fixture_worker': 'fixture-project'},
                                prev, {}, NOW, hc=lambda n, p: text)
        self.assertFalse(watched['fixture_worker']['busy'])
        self.assertEqual(codes(), ['MANAGED_SCREEN_INPUT_UNVERIFIED'])
        self.assertFalse(watched['fixture_worker']['idle_eligible'])
        self.assertEqual(watched['fixture_worker']['composer_state'], 'unknown')
        self.assertTrue(sc.findings[0]['must_act'])
        self.assertNotRegex(sc.findings[0]['text'], r'(?i)alt\+m|file pointer \+ CR')
    def test_newer_completed_or_zero_footer_overrides_old_running_footer(self):
        prior = 'Brewed for 39s · done 3:46 PM · 1 monitor still running\n'
        for current in ['Cooked for 2s · done 3:48 PM\n',
                        'auto mode on · 0 monitors · ↓ to manage\n',
                        'Cooked for 2s · done 3:48 PM · 1 monitor completed\n']:
            with self.subTest(current=current):
                self.assertFalse(sc.tail_busy(prior + current + '>\n'))
    def test_old_running_footer_outside_raw_tail_is_not_revived_by_filtering(self):
        prior = 'Brewed for 39s · done 3:46 PM · 1 monitor still running\n'
        quoted = ''.join(f'> archived quotation line {i}\n' for i in range(20))
        fenced = '```text\n' + ''.join(f'archived example line {i}\n' for i in range(20)) + '```\n'
        for later in [quoted, fenced]:
            with self.subTest(later=later):
                self.assertFalse(sc.tail_busy(prior + later + '>\n'))
        self.assertTrue(sc.tail_busy(prior + '>\n'))
    def test_foreground_examples_are_not_live_ui(self):
        example = 'esc to cancel      Gemini 4 Argon · high'
        for text in ['> ' + example, '`' + example + '`',
                     '```text\n' + example + '\n```',
                     '~~~text\n' + example + '\n~~~']:
            with self.subTest(text=text):
                self.assertFalse(sc.tail_busy(text + '\n >\n'))
        self.assertTrue(sc.tail_busy(AGY_BUSY))
    def test_newer_completed_status_overrides_old_foreground_busy_hint(self):
        self.assertFalse(sc.tail_busy(AGY_BUSY + CLAUDE_IDLE))
        self.assertTrue(sc.tail_busy(CLAUDE_IDLE + AGY_BUSY))
    def test_parked_and_positional_lane_lines_are_managed(self):
        scs = {'captains': {'jopedime': {'state': 'jopedime_flywheel working on spec', 'pr': '1061448.jopedime_flywheel ETA 15:05Z'}}}
        m = sc.managed_screens(scs, {'jopedime': {'jopedime_agy': 'LANE | 2830191.jopedime_agy | X | active'}})
        self.assertEqual(m['jopedime_flywheel'], 'jopedime'); self.assertEqual(m['jopedime_agy'], 'jopedime')
        self.assertNotIn('super_captain', m)

class Eta(unittest.TestCase):
    def setUp(self): sc.findings.clear()
    def test_passed_eta_in_live_status_is_red(self):
        scs = {'captains': {'jopedime': {'state': 'jopedime_flywheel working on AUTH-GAP-01/02 closure spec (ETA 15:05Z)'}},
               'decisions': ['old thing ETA 09:00Z']}
        scs['captains']['jopedime']['pr'] = 'merge ETA 14:55Z'
        sc.check_eta_overdue(scs, NOW)
        self.assertEqual(codes(), ['OWED_ETA_OVERDUE']); self.assertEqual(sc.findings[0]['scope'], 'jopedime')
        self.assertIn('2 ETA(s)', sc.findings[0]['text'])
    def test_future_mdt_and_merged_not_flagged(self):
        scs = {'captains': {'dope': {'pr': 'PR #272 MERGED (ETA 15:05Z)', 'state': 'next PR ETA 09:40 AM MDT'}}}
        sc.check_eta_overdue(scs, NOW); self.assertEqual(codes(), [])
    def test_mdt_conversion(self):
        hits = sc.eta_overdue({'captains': {'x': {'s': 'spec due 09:05 AM MDT'}}}, NOW)
        self.assertEqual(hits[0][1], 7)

class Redrive(unittest.TestCase):
    LOG = ("## 2026-10-18T14:49:30Z (08:49 AM MDT) — ACK\n"
           "- **WATCH:** Verified `1061448.jopedime_flywheel` completed X at 14:32Z and re-drove it to write SPEC.json (ETA 15:05Z).\n")
    def setUp(self): sc.findings.clear()
    def test_claim_without_terminal_input_is_red(self):
        li = lambda n: datetime.datetime(2026, 10, 18, 14, 31, 31, tzinfo=UTC)
        sc.check_redrive_delivered(self.LOG, {'jopedime_flywheel': 1}, NOW, last_input=li)
        self.assertEqual(codes(), ['REDRIVE_NOT_DELIVERED'])
    def test_claim_with_input_ok(self):
        li = lambda n: datetime.datetime(2026, 10, 18, 14, 48, tzinfo=UTC)
        sc.check_redrive_delivered(self.LOG, {'jopedime_flywheel': 1}, NOW, last_input=li); self.assertEqual(codes(), [])

class PushAndClaims(unittest.TestCase):
    def setUp(self): sc.findings.clear()
    def test_no_push_30m_with_open_work(self):
        rep = {'projects': {'jarambit': {'done_recomputed': 6, 'total': 37}, 'admiralty': {'done_recomputed': 32, 'total': 32, 'open_prs': []}}}
        sc.check_no_push(rep, NOW, push_fn=lambda p: datetime.datetime(2026, 10, 18, 14, 30, tzinfo=UTC))
        self.assertEqual(codes(), ['NO_PUSH_30M']); self.assertEqual(sc.findings[0]['scope'], 'jarambit')
    def test_state_text_and_idle_claims(self):
        rep = {'projects': {'jopedime': {'done_recomputed': 0, 'total': 66}}}
        scs = {'captains': {'jopedime': {'state': 'P1-A TOP PRIORITY — 66/66 done on main', 'done': 0, 'workers_idle': 0}}}
        watch = {'jopedime_flywheel': {'project': 'jopedime', 'busy': False, 'unchanged_min': 40}}
        sc.check_sc_claims(rep, scs, watch)
        self.assertEqual(sorted(codes()), ['SC_IDLE_CLAIM_FALSE', 'SC_STATE_TEXT_OVERCLAIM'])

if __name__ == "__main__":
    unittest.main()
