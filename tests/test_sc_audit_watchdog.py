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
    def test_claude_auto_mode_idle_is_red_after_20_min(self):
        prev = {'watch:jopedime_flywheel': {'hash': sc.tail_hash(CLAUDE_IDLE), 'since': '2026-10-18T14:32:00+00:00'}}
        newh = {}
        w = sc.idle_watch({'jopedime_flywheel': 'x'}, {'jopedime_flywheel': 'jopedime'}, prev, newh, NOW, hc=lambda n, p: CLAUDE_IDLE)
        self.assertFalse(w['jopedime_flywheel']['busy']); self.assertEqual(codes(), ['MANAGED_SCREEN_IDLE'])
        self.assertTrue(sc.findings[0]['must_act'])
    def test_busy_agy_not_idle_and_first_sighting_not_flagged(self):
        sc.idle_watch({'jopedime_agy': 'x'}, {'jopedime_agy': 'jopedime'}, {}, {}, NOW, hc=lambda n, p: AGY_BUSY)
        sc.idle_watch({'jopedime_flywheel': 'x'}, {'jopedime_flywheel': 'jopedime'}, {}, {}, NOW, hc=lambda n, p: CLAUDE_IDLE)
        self.assertEqual(codes(), [])
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
