import ast
import datetime
import hashlib
import importlib.util
import re
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "tools"
sys.path.insert(0, str(TOOLS))

def screen_function():
    source = (TOOLS / "sc_audit.py").read_text()
    tree = ast.parse(source)
    node = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "screen_state")
    ns = {"re": re, "hashlib": hashlib,
          "SURVEY": re.compile("never-match"), "QUOTA": re.compile("never-match"),
          "APPROVAL": re.compile("never-match")}
    exec(compile(ast.Module(body=[node], type_ignores=[]), "<screen_state>", "exec"), ns)
    return ns["screen_state"]

class ScreenRegression(unittest.TestCase):
    def test_codex_running_is_not_idle(self):
        state = screen_function()("Working (4s • esc to interrupt)\n›\n? for shortcuts")
        self.assertTrue(state["busy"])
        self.assertFalse(state["idle_prompt"])

    def test_codex_pasted_draft_is_protected_even_with_lossy_prompt(self):
        state = screen_function()("Working (4s • esc to interrupt)\n: [Pasted Content 1024 chars]\n  tab to queue message")
        self.assertTrue(state["typed_input"])
        self.assertFalse(state["queued_msgs"], "unsent composer is distinct from persisted queue")

    def test_old_client_running_remains_supported(self):
        self.assertTrue(screen_function()("working esc to cancel")["busy"])

    def test_explicit_empty_modern_prompt_can_be_idle(self):
        self.assertTrue(screen_function()("›\n? for shortcuts")["idle_prompt"])

    def test_missing_prompt_is_unknown_not_idle(self):
        self.assertFalse(screen_function()("unrecognized terminal")["idle_prompt"])

def audit_watch_functions():
    source = (TOOLS / "sc_audit.py").read_text()
    tree = ast.parse(source)
    functions = {"screen_state", "tail_busy", "tail_hash", "idle_watch", "F", "MA",
                 "check_redrive_delivered", "redrive_claims", "parse_utc"}
    constants = {"IDLE_RED_MIN", "BUSY_RE", "CLAUDE_STATUS_PREFIX",
                 "BACKGROUND_STATUS_RE", "BACKGROUND_BUSY_RE", "REDRIVE_GRACE_MIN", "SCREEN_ID_RE"}
    nodes = [node for node in tree.body
             if (isinstance(node, ast.FunctionDef) and node.name in functions)
             or (isinstance(node, ast.Assign)
                 and any(isinstance(target, ast.Name) and target.id in constants
                         for target in node.targets))]
    ns = {"re": re, "hashlib": hashlib, "datetime": datetime, "findings": [],
          "SURVEY": re.compile("never-match"), "QUOTA": re.compile("never-match"),
          "APPROVAL": re.compile("never-match"), "pty_last_input": lambda pid: None}
    exec(compile(ast.Module(body=nodes, type_ignores=[]), "<audit_watch>", "exec"), ns)
    return ns


class ManagedDraftRegression(unittest.TestCase):
    def observe_stable(self, text):
        ns = audit_watch_functions()
        now = datetime.datetime(2026, 10, 21, 2, 0, tzinfo=datetime.timezone.utc)
        since = (now - datetime.timedelta(minutes=21)).isoformat(timespec="seconds")
        prev = {"watch:captain_fixture": {"hash": ns["tail_hash"](text), "since": since}}
        result = ns["idle_watch"]({"captain_fixture": object()},
            {"captain_fixture": "dope"}, prev, {}, now,
            hc=lambda name, path: text)
        return result["captain_fixture"], ns["findings"]

    def test_blank_prompt_cannot_hide_pasted_draft(self):
        state = screen_function()("›\n[Pasted Content 1024 chars]\n? for shortcuts")
        self.assertTrue(state["typed_input"])
        self.assertFalse(state["idle_prompt"])

    def test_managed_unsent_draft_never_gets_redrive_instruction(self):
        state, findings = self.observe_stable(": [Pasted Content 1024 chars]\n  tab to queue message")
        self.assertTrue(state["typed_input"])
        self.assertFalse(state["idle_eligible"])
        self.assertIn("MANAGED_SCREEN_DRAFT_PROTECTED", {f["code"] for f in findings})
        self.assertNotIn("MANAGED_SCREEN_IDLE", {f["code"] for f in findings})
        self.assertFalse(any("CR + alt+m" in f["text"] for f in findings))

    def test_managed_unknown_composer_is_unverified_not_blank(self):
        state, findings = self.observe_stable("unrecognized native terminal")
        self.assertFalse(state["idle_eligible"])
        self.assertIn("MANAGED_SCREEN_INPUT_UNVERIFIED", {f["code"] for f in findings})
        self.assertNotIn("MANAGED_SCREEN_IDLE", {f["code"] for f in findings})

    def test_managed_verified_blank_prompt_still_reports_idle(self):
        state, findings = self.observe_stable("›\n? for shortcuts")
        self.assertTrue(state["idle_eligible"])
        self.assertIn("MANAGED_SCREEN_IDLE", {f["code"] for f in findings})

    def test_managed_busy_draft_remains_busy(self):
        state, findings = self.observe_stable("Working (4s • esc to interrupt)\n: [Pasted text 10 chars]")
        self.assertTrue(state["busy"])
        self.assertTrue(state["typed_input"])
        self.assertFalse(state["idle_eligible"])
        self.assertNotIn("MANAGED_SCREEN_IDLE", {f["code"] for f in findings})

    def test_managed_queue_hint_does_not_request_duplicate_input(self):
        state, findings = self.observe_stable("queued messages\n›\n? for shortcuts")
        self.assertTrue(state["queued_msgs"])
        self.assertFalse(state["idle_eligible"])
        self.assertIn("MANAGED_SCREEN_INPUT_UNVERIFIED", {f["code"] for f in findings})
        self.assertNotIn("MANAGED_SCREEN_IDLE", {f["code"] for f in findings})


class ContinuationDraftRegression(unittest.TestCase):
    observe_stable = ManagedDraftRegression.observe_stable

    def assert_unverified_without_input_instruction(self, text):
        state, findings = self.observe_stable(text)
        self.assertEqual(state["composer_state"], "unknown")
        self.assertFalse(state["idle_eligible"])
        self.assertIn("MANAGED_SCREEN_INPUT_UNVERIFIED", {f["code"] for f in findings})
        self.assertNotIn("MANAGED_SCREEN_IDLE", {f["code"] for f in findings})

    def test_exact_multiline_draft_after_old_blank_is_unverified(self):
        self.assert_unverified_without_input_instruction(
            "›\n  preserve my unsent multiline draft\n? for shortcuts")

    def test_stale_footer_with_unknown_following_text_is_unverified(self):
        self.assert_unverified_without_input_instruction(
            "›\n? for shortcuts\n  unknown current continuation")

    def test_unsupported_current_composer_after_old_footer_is_unverified(self):
        self.assert_unverified_without_input_instruction(
            "›\n? for shortcuts\n[unsupported current composer]\n  unsent text")

    def test_verified_blank_finding_never_requests_terminal_keys(self):
        state, findings = self.observe_stable("›\n? for shortcuts")
        self.assertTrue(state["idle_eligible"])
        finding = next(f for f in findings if f["code"] == "MANAGED_SCREEN_IDLE")
        self.assertIn("BLOCKED_TRANSPORT", finding["text"])
        self.assertIn("queue/priority", finding["text"])
        self.assertNotRegex(finding["text"], r"(?i)alt\+m|send\s+CR|file pointer \+ CR|paste the pointer")

    def test_unverified_historical_redrive_never_requests_terminal_keys(self):
        ns = audit_watch_functions()
        now = datetime.datetime(2026, 10, 21, 2, 0, tzinfo=datetime.timezone.utc)
        log = "## observed 2026-10-21T02:00:00Z\nre-drove 1234.captain_fixture"
        ns["check_redrive_delivered"](log, {"captain_fixture": object()}, now,
            last_input=lambda name: now - datetime.timedelta(minutes=30))
        finding = next(f for f in ns["findings"] if f["code"] == "REDRIVE_NOT_DELIVERED")
        self.assertIn("BLOCKED_TRANSPORT", finding["text"])
        self.assertNotRegex(finding["text"], r"(?i)alt\+m|send\s+CR|paste the pointer")


class QueueRegression(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = Path(self.tmp.name) / "queue.sqlite"
        c = sqlite3.connect(self.db)
        c.execute("create table queued_items(id text primary key,thread_id text,payload_json text,queue_order integer,created_at_ms integer,updated_at_ms integer)")
        c.close()
        import native_queue_health
        self.health = native_queue_health

    def tearDown(self):
        self.tmp.cleanup()

    def insert(self, count, thread="admiral", created=1_000):
        c = sqlite3.connect(self.db)
        c.executemany("insert into queued_items values(?,?,?,?,?,?)",
          [(f"{thread}-{i}", thread, "SECRET_MUST_NOT_BE_EXPORTED", i, created, created)
           for i in range(count)])
        c.commit(); c.close()

    def test_incident_36_pending_is_red_without_terminal_hint(self):
        self.insert(36)
        before = self.db.read_bytes()
        report = self.health.read_queue_health(self.db, now_ms=25_000_000)
        self.assertEqual(report["threads"][0]["pending"], 36)
        self.assertEqual(report["threads"][0]["state"], "stalled")
        self.assertEqual(report["threads"][0]["oldest_age_ms"], 24_999_000)
        self.assertNotIn("SECRET", str(report))
        self.assertEqual(before, self.db.read_bytes())

    def test_missing_database_is_unknown_and_not_created(self):
        self.db.unlink()
        report = self.health.read_queue_health(self.db, now_ms=1000)
        self.assertEqual(report["status"], "unknown")
        self.assertFalse(self.db.exists())

    def test_corrupt_database_is_unknown_not_empty(self):
        self.db.write_bytes(b"not SQLite")
        self.assertEqual(self.health.read_queue_health(self.db, now_ms=1000)["status"], "unknown")

    def test_wal_committed_rows_are_included_and_writer_is_untouched(self):
        writer = sqlite3.connect(self.db)
        writer.execute("pragma journal_mode=wal")
        writer.execute("insert into queued_items values(?,?,?,?,?,?)",
                       ("wal-1","admiral","secret",1,1000,1000))
        writer.commit()
        self.assertEqual(self.health.read_queue_health(self.db, now_ms=1001)["threads"][0]["pending"],1)
        writer.execute("insert into queued_items values(?,?,?,?,?,?)",
                       ("not-committed","admiral","secret",2,1000,1000))
        self.assertEqual(self.health.read_queue_health(self.db, now_ms=1001)["threads"][0]["pending"],1)
        writer.rollback(); writer.close()

    def test_recent_queue_is_pending_not_delivered(self):
        self.insert(1)
        item = self.health.read_queue_health(self.db, now_ms=1001)["threads"][0]
        self.assertEqual(item["state"], "pending")
        self.assertFalse(item["consumption_verified"])

    def test_future_timestamp_reports_clock_unknown(self):
        self.insert(1, created=9999)
        self.assertEqual(self.health.read_queue_health(self.db, now_ms=1001)["threads"][0]["state"],"clock_unknown")

    def test_result_limit_is_explicitly_incomplete(self):
        self.insert(1,"a");self.insert(1,"b")
        out=self.health.read_queue_health(self.db, now_ms=1001,max_threads=1)
        self.assertFalse(out["complete"])
        self.assertEqual(len(out["threads"]),1)

    def test_wrong_runtime_is_rejected_before_send(self):
        with self.assertRaisesRegex(ValueError,"not loaded"):
            self.health.require_live_target("admiral","/captains",
                {"id":"admiral","cwd":"/captains","status":{"type":"notLoaded"}},None)
        with self.assertRaisesRegex(ValueError,"identity"):
            self.health.require_live_target("admiral","/captains",
                {"id":"other","cwd":"/captains","status":{"type":"active"}},{"id":"turn","status":"inProgress"})

    def test_live_guard_requires_actual_in_progress_turn(self):
        self.assertEqual(self.health.require_live_target("admiral","/captains",
            {"id":"admiral","cwd":"/captains","status":{"type":"active"}},
            {"id":"turn","status":"inProgress"}),"turn")
        with self.assertRaises(ValueError):
            self.health.require_live_target("admiral","/captains",
                {"id":"admiral","cwd":"/captains","status":{"type":"active"}},
                {"id":"turn","status":"interrupted"})

if __name__ == "__main__":
    unittest.main()
