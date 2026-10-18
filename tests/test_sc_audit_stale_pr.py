import datetime, importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)
NOW = datetime.datetime(2026, 10, 18, 16, 30, tzinfo=datetime.timezone.utc)

def pr(**kw):
    base = {"number": 1, "headRefName": "fix/x", "isDraft": False, "createdAt": "2026-10-18T16:20:00Z", "mergeable": "MERGEABLE", "statusCheckRollup": []}
    base.update(kw); return base

class StalePr(unittest.TestCase):
    def test_fresh_clean_pr_ok(self):
        self.assertEqual(sc.stale_pr_problems(pr(), NOW), [])
    def test_old_pr_flagged(self):
        self.assertEqual(sc.stale_pr_problems(pr(createdAt="2026-10-18T15:30:00Z"), NOW), ["open 60 min"])
    def test_draft_ignored(self):
        self.assertEqual(sc.stale_pr_problems(pr(isDraft=True, createdAt="2026-10-01T00:00:00Z"), NOW), [])
    def test_conflict_and_backup_branch(self):
        got = sc.stale_pr_problems(pr(mergeable="CONFLICTING", headRefName="backup/pr208-pre-rebase"), NOW)
        self.assertIn("conflicts with main", got)
        self.assertTrue(any("backup branch" in g for g in got))
    def test_failure_superseded_by_later_success_not_flagged(self):
        roll = [{"name": "cpu", "conclusion": "FAILURE", "completedAt": "2026-10-18T16:10:00Z"},
                {"name": "cpu", "conclusion": "SUCCESS", "completedAt": "2026-10-18T16:25:00Z"}]
        self.assertEqual(sc.stale_pr_problems(pr(statusCheckRollup=roll), NOW), [])
    def test_latest_failure_flagged(self):
        roll = [{"name": "security", "conclusion": "SUCCESS", "completedAt": "2026-10-18T16:00:00Z"},
                {"name": "security", "conclusion": "FAILURE", "completedAt": "2026-10-18T16:22:00Z"}]
        self.assertEqual(sc.stale_pr_problems(pr(statusCheckRollup=roll), NOW), ["failing security"])
    def test_bad_timestamp_ignored(self):
        self.assertEqual(sc.stale_pr_problems(pr(createdAt=None), NOW), [])

if __name__ == "__main__":
    unittest.main()
