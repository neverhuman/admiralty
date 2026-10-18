import datetime, importlib.util, pathlib, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)
NOW = datetime.datetime(2026, 10, 18, 13, 50, tzinfo=datetime.timezone.utc)

class QueuedStuck(unittest.TestCase):
    def test_queued_an_hour_is_stuck(self):
        roll = [{"name": "contracts", "status": "QUEUED", "conclusion": "", "startedAt": "2026-10-18T12:47:06Z"}]
        self.assertEqual(sc.stuck_queued_checks(roll, NOW), [("contracts", 63)])
    def test_recent_queue_and_running_and_done_not_stuck(self):
        roll = [{"name": "a", "status": "QUEUED", "conclusion": "", "startedAt": "2026-10-18T13:40:00Z"},
                {"name": "b", "status": "IN_PROGRESS", "conclusion": "", "startedAt": "2026-10-18T12:00:00Z"},
                {"name": "c", "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": "2026-10-18T12:00:00Z"}]
        self.assertEqual(sc.stuck_queued_checks(roll, NOW), [])
    def test_bad_timestamp_ignored(self):
        self.assertEqual(sc.stuck_queued_checks([{"name": "x", "status": "QUEUED", "startedAt": None}], NOW), [])

if __name__ == "__main__":
    unittest.main()
