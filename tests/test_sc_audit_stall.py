import datetime, importlib.util, pathlib, tempfile, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)
UTC = datetime.timezone.utc
NOW = datetime.datetime(2026, 10, 18, 17, 20, tzinfo=UTC)

def codes(r): return [c for _, c, _ in r]

class Stall(unittest.TestCase):
    def test_under_60_quiet(self):
        self.assertEqual(sc.stall_escalation("jarambit", 6, 37, "2026-10-18T16:30:00+00:00", NOW), [])
    def test_60_needs_inspection_and_plan(self):
        self.assertEqual(codes(sc.stall_escalation("jarambit", 6, 37, "2026-10-18T16:15:00+00:00", NOW)), ["STALL_60M", "STALL_NO_PLAN_CHANGE"])
    def test_plan_change_after_stall_clears_only_that(self):
        pc = [datetime.datetime(2026, 10, 18, 16, 50, tzinfo=UTC)]
        self.assertEqual(codes(sc.stall_escalation("jarambit", 6, 37, "2026-10-18T16:15:00+00:00", NOW, pc)), ["STALL_60M"])
    def test_plan_change_before_stall_does_not_count(self):
        pc = [datetime.datetime(2026, 10, 18, 14, 0, tzinfo=UTC)]
        self.assertIn("STALL_NO_PLAN_CHANGE", codes(sc.stall_escalation("jarambit", 6, 37, "2026-10-18T16:15:00+00:00", NOW, pc)))
    def test_jarambit_today_is_repeated(self):
        # 6/37 since 14:31Z, merges of #115/#116 must not clear it
        pc = [datetime.datetime(2026, 10, 18, 15, 30, tzinfo=UTC)]
        got = sc.stall_escalation("jarambit", 6, 37, "2026-10-18T14:31:32+00:00", NOW, pc)
        self.assertEqual(codes(got), ["STALL_60M", "STALL_REPEATED"])
        self.assertIn("did not move the count", got[-1][2])
    def test_complete_project_exempt(self):
        self.assertEqual(sc.stall_escalation("admiralty", 32, 32, "2026-10-18T10:00:00+00:00", NOW), [])
    def test_bad_inputs(self):
        self.assertEqual(sc.stall_escalation("x", None, 10, "2026-10-18T10:00:00+00:00", NOW), [])
        self.assertEqual(sc.stall_escalation("x", 1, 10, None, NOW), [])
        self.assertEqual(sc.stall_escalation("x", 1, 10, "garbage", NOW), [])
    def test_parse_plan_change_lines(self):
        with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as f:
            f.write("## x\n- STALL_PLAN_CHANGE jarambit 2026-10-18T17:05:00Z: added 3 workers\n- STALL_PLAN_CHANGE `dope` `2026-10-18T17:06Z` re-scope\n")
        got = sc.stall_plan_changes([f.name])
        self.assertEqual(got["jarambit"][0], datetime.datetime(2026, 10, 18, 17, 5, tzinfo=UTC))
        self.assertEqual(got["dope"][0].minute, 6)

if __name__ == "__main__":
    unittest.main()
