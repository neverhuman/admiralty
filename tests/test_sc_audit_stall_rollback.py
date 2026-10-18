import datetime, importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)
UTC = datetime.timezone.utc
NOW = datetime.datetime(2026, 10, 18, 19, 18, tzinfo=UTC)

def H(*pairs): return [{"utc": u, "done": d} for u, d in pairs]

class Rollback(unittest.TestCase):
    def test_dope_overclaim_blip_does_not_reset_clock(self):
        h = H(("2026-10-18T11:40:00+00:00", 58), ("2026-10-18T11:49:03+00:00", 59), ("2026-10-18T18:51:54+00:00", 60), ("2026-10-18T19:09:49+00:00", 59))
        self.assertEqual(sc.last_sustained_increase(h, 59), "2026-10-18T11:49:03+00:00")
    def test_blip_with_current_rollback(self):
        h = H(("2026-10-18T11:49:03+00:00", 59), ("2026-10-18T12:00:00+00:00", 59), ("2026-10-18T18:51:54+00:00", 60))
        self.assertIsNone(sc.last_sustained_increase(h, 59))
    def test_real_increase_counts(self):
        h = H(("2026-10-18T11:49:03+00:00", 59), ("2026-10-18T18:51:54+00:00", 60), ("2026-10-18T19:09:49+00:00", 60))
        self.assertEqual(sc.last_sustained_increase(h, 60), "2026-10-18T18:51:54+00:00")
    def test_overclaim_then_real(self):
        h = H(("2026-10-18T11:00:00+00:00", 58), ("2026-10-18T11:49:03+00:00", 59), ("2026-10-18T18:00:00+00:00", 60), ("2026-10-18T18:30:00+00:00", 59), ("2026-10-18T19:00:00+00:00", 60))
        self.assertEqual(sc.last_sustained_increase(h, 60), "2026-10-18T19:00:00+00:00")
    def test_stall_fires_on_dope_case(self):
        h = H(("2026-10-18T11:40:00+00:00", 58), ("2026-10-18T11:49:03+00:00", 59), ("2026-10-18T18:51:54+00:00", 60), ("2026-10-18T19:09:49+00:00", 59))
        got = [c for _, c, _ in sc.stall_escalation("dope", 59, 65, sc.last_sustained_increase(h, 59), NOW)]
        self.assertIn("STALL_60M", got); self.assertIn("STALL_REPEATED", got)

def R(sha, c, t): return {"headSha": sha, "conclusion": c, "status": "completed", "workflowName": "Admiralty checks", "createdAt": t}

class MainRed(unittest.TestCase):
    def test_p1_persistent_red(self):
        runs = [R("7bb", "failure", "2026-10-18T19:05:36Z"), R("2f7", "failure", "2026-10-18T18:50:32Z"), R("843", "failure", "2026-10-18T18:41:05Z"), R("a68", "failure", "2026-10-18T17:43:50Z"), R("ok1", "success", "2026-10-18T17:30:05Z")]
        sev, code, text = sc.main_ci_severity("admiralty", "7bb2784dc", ["Admiralty checks"], runs, NOW)
        self.assertEqual((sev, code), ("red", "MAIN_CI_RED_PERSISTENT")); self.assertIn("4 consecutive", text)
    def test_p1_fresh_single_red_stays_amber(self):
        runs = [R("new", "failure", "2026-10-18T19:10:00Z"), R("ok1", "success", "2026-10-18T19:00:00Z")]
        self.assertEqual(sc.main_ci_severity("admiralty", "new", ["x"], runs, NOW)[0], "amber")
    def test_p2_stays_amber(self):
        runs = [R("a", "failure", "2026-10-18T15:00:00Z"), R("b", "failure", "2026-10-18T14:00:00Z"), R("c", "failure", "2026-10-18T13:00:00Z")]
        self.assertEqual(sc.main_ci_severity("dope", "a", ["CI"], runs, NOW)[1], "MAIN_CI_RED")

if __name__ == "__main__":
    unittest.main()
