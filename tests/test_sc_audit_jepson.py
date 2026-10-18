import importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)

class JepsonBottleneck(unittest.TestCase):
    def test_review_request_flagged(self):
        self.assertEqual(sc.jepson_pr_reasons({"reviewRequests": [{"login": "jepsontaylor"}]}), ["review requested from jepsontaylor"])
    def test_bot_review_not_flagged(self):
        self.assertEqual(sc.jepson_pr_reasons({"reviewRequests": [{"login": "neverhumanbot"}], "latestReviews": [{"author": {"login": "neverhumanbot"}, "state": "CHANGES_REQUESTED"}]}), [])
    def test_jepson_changes_requested(self):
        self.assertEqual(sc.jepson_pr_reasons({"latestReviews": [{"author": {"login": "JepsonTaylor"}, "state": "CHANGES_REQUESTED"}]}), ["jepsontaylor requested changes"])
    def test_draft_ignored(self):
        self.assertEqual(sc.jepson_pr_reasons({"isDraft": True, "reviewRequests": [{"login": "jepsontaylor"}]}), [])
    def test_text_waiting_on_jepson(self):
        self.assertEqual(len(sc.jepson_text_hits([("s", "PR #9 is waiting on Jepson to merge")])), 1)
        self.assertEqual(len(sc.jepson_text_hits([("s", "W01 NEEDS JEPSON decision on schedule")])), 1)
    def test_text_reserved_excepted(self):
        self.assertEqual(sc.jepson_text_hits([("s", "Kumo license is new paid spend, waiting on Jepson")]), [])
        self.assertEqual(sc.jepson_text_hits([("s", "anything on xbabe0 needs Jepson")]), [])
    def test_text_clean(self):
        self.assertEqual(sc.jepson_text_hits([("s", "Jepson ruling applied; super_captain merged #12")]), [])
    def test_protection_codeowner(self):
        prot = {"required_pull_request_reviews": {"require_code_owner_reviews": True}}
        self.assertTrue(sc.jepson_protection_reasons(prot, "* @jepsontaylor\n"))
        self.assertEqual(sc.jepson_protection_reasons(prot, "* @neverhuman\n"), [])
        self.assertEqual(sc.jepson_protection_reasons({}, "* @jepsontaylor\n"), [])
    def test_protection_restrictions(self):
        self.assertTrue(sc.jepson_protection_reasons({"restrictions": {"users": [{"login": "jepsontaylor"}]}}, ""))

if __name__ == "__main__":
    unittest.main()
