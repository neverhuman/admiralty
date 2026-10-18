import importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)

class SurveyRegex(unittest.TestCase):
    def test_priority_label_not_survey(self):
        t = "  NOW. P1 = JopeDime GPU (all 3 GPUs at 0%) + Admiralty; re-drive idle screens;\nesc to cancel"
        self.assertFalse(sc.screen_state(t)["survey"])
    def test_real_rating_scale_is_survey(self):
        self.assertTrue(sc.screen_state("How is Claude doing this session?\n 1: Bad   2: Fine   3: Good   0: Dismiss")["survey"])
        self.assertTrue(sc.screen_state("Rate this session  1 = poor  2 = ok  3 = great")["survey"])
    def test_plain_assignment_not_survey(self):
        self.assertFalse(sc.screen_state("x1 = 5; y2 = 6\nP2 = Admiralty")["survey"])

if __name__ == "__main__":
    unittest.main()
