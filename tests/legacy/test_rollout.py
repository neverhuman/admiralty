"""Offline bridge checks; no collector import, Screen session, or live service."""

import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rollout", ROOT / "dashboard" / "rollout.py")
rollout = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rollout)
NOW = datetime.datetime(2026, 10, 18, 12, 0, tzinfo=datetime.timezone.utc)


def manifest():
    return {"schema_version": 1, "project": {"id": "build-123", "name": "Example build"},
            "updated_utc": NOW.isoformat(), "producer": "test/v1",
            "packets": [{"id": "P01", "title": "Foundation", "status": "ready", "depends_on": []}],
            "lanes": [], "pr": {"url": "https://example.com/repo/pull/1", "head": "abc123"}}


def verification(revision="abc123", verifier="reviewer"):
    return {"receipt": "receipts/verification.json", "revision": revision,
            "verifier": verifier, "observed_utc": NOW.isoformat()}


def integration(revision="abc123"):
    return {"receipt": "receipts/integration.json", "revision": revision,
            "target_ref": "refs/heads/main", "observed_utc": NOW.isoformat()}


class RolloutLoaderTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.path = self.root / "rollout.json"

    def load(self, value=None, environ=None):
        if value is not None:
            self.path.write_text(json.dumps(value), encoding="utf-8")
        return rollout.collect_rollout(self.root, now=NOW, environ=environ or {})

    def test_missing_default_is_optional(self):
        self.assertIsNone(self.load())

    def test_explicit_missing_manifest_is_visible(self):
        state = self.load(environ={"ADMIRALTY_ROLLOUT_MANIFEST": str(self.root / "missing.json")})
        self.assertEqual(state["state"], "unavailable")
        self.assertTrue(state["read_only"])

    def test_explicit_manifest_overrides_default(self):
        other = self.root / "another.json"
        other.write_text(json.dumps(manifest()), encoding="utf-8")
        state = self.load(environ={"ADMIRALTY_ROLLOUT_MANIFEST": str(other)})
        self.assertEqual(state["source"]["filename"], "another.json")
        self.assertEqual(state["state"], "ready")

    def test_valid_manifest_has_computed_counts_hash_and_source(self):
        state = self.load(manifest())
        self.assertEqual(state["counts"]["ready"], 1)
        self.assertEqual(state["total"], 1)
        self.assertEqual(state["source"]["sha256"], hashlib.sha256(self.path.read_bytes()).hexdigest())
        self.assertEqual(state["freshness"]["state"], "fresh")
        self.assertEqual(state["project"]["id"], "build-123")

    def test_source_time_not_mtime_controls_freshness(self):
        value = manifest()
        value["updated_utc"] = (NOW - datetime.timedelta(seconds=301)).isoformat()
        state = self.load(value)
        self.assertEqual(state["freshness"]["state"], "stale")
        self.assertEqual(state["freshness"]["age_s"], 301)

    def test_lane_identity_and_source_age_reach_react_without_claiming_acceptance(self):
        value = manifest()
        value["lanes"] = [{"id": "build-lane", "screen": "custom_worker", "model": "local-model",
            "packet_id": "P01A", "source_updated_utc": "original timestamp", "status": "stale; screen present",
            "screen_state": "present", "untrusted_extra": "discard"}]
        lane = self.load(value)["lanes"][0]
        self.assertEqual(lane["screen"], "custom_worker")
        self.assertEqual(lane["model"], "local-model")
        self.assertEqual(lane["packet_id"], "P01A")
        self.assertEqual(lane["source_updated_utc"], "original timestamp")
        self.assertNotIn("untrusted_extra", lane)
        self.assertNotIn("verification", lane)

    def test_future_source_is_flagged(self):
        value = manifest()
        value["updated_utc"] = (NOW + datetime.timedelta(minutes=10)).isoformat()
        self.assertEqual(self.load(value)["freshness"]["state"], "future")

    def test_bad_json_and_deep_json_fail_closed(self):
        for raw in (b"{broken", b"\xff", b"[" * 2000 + b"]" * 2000):
            with self.subTest(raw=raw[:10]):
                self.path.write_bytes(raw)
                self.assertEqual(self.load()["state"], "invalid")

    def test_schema_and_type_errors_are_bounded(self):
        variants = []
        for field, value in (("schema_version", 2), ("schema_version", True), ("packets", {}),
                             ("updated_utc", "2026-10-18T12:00:00"), ("lanes", "bad"),
                             ("stale_after_s", -1), ("stale_after_s", True)):
            item = manifest()
            item[field] = value
            variants.append(item)
        variants.extend([[], {"schema_version": 1}, None])
        for value in variants:
            with self.subTest(value=value):
                self.path.write_text(json.dumps(value), encoding="utf-8")
                self.assertEqual(self.load()["state"], "invalid")

    def test_duplicate_packets_and_unknown_status_are_invalid(self):
        value = manifest()
        value["packets"] *= 2
        self.assertEqual(self.load(value)["state"], "invalid")
        value = manifest()
        value["packets"][0]["status"] = "done"
        self.assertEqual(self.load(value)["state"], "invalid")

    def test_oversized_or_special_input_cannot_hang_collector(self):
        self.path.write_bytes(b" " * (rollout.MAX_BYTES + 1))
        self.assertEqual(self.load()["state"], "invalid")
        self.path.unlink()
        self.path.mkdir()
        self.assertIn(self.load()["state"], ("invalid", "unavailable"))
        self.path.rmdir()
        if hasattr(os, "mkfifo"):
            os.mkfifo(self.path)
            self.assertEqual(self.load()["state"], "invalid")

    def test_embedded_null_path_reports_invalid(self):
        state = self.load(environ={"ADMIRALTY_ROLLOUT_MANIFEST": "bad\0path"})
        self.assertEqual(state["state"], "invalid")

    def test_references_remain_text_never_read_or_executed(self):
        secret = self.root / "never-open-this.txt"
        secret.write_text("PRIVATE CONTENT", encoding="utf-8")
        value = manifest()
        value["packets"][0].update({"capsule_path": str(secret),
            "evidence": [{"kind": "candidate_receipt", "path": "$(touch unexpected)", "revision": "abc"}],
            "command": "touch unexpected"})
        state = self.load(value)
        self.assertNotIn("PRIVATE CONTENT", json.dumps(state))
        self.assertEqual(state["packets"][0]["capsule_path"], str(secret))
        self.assertNotIn("command", state["packets"][0])
        self.assertFalse((self.root / "unexpected").exists())
        self.assertEqual(secret.read_text(), "PRIVATE CONTENT")

    def test_loader_does_not_mutate_manifest(self):
        self.load(manifest())
        before = self.path.read_bytes()
        self.load()
        self.assertEqual(self.path.read_bytes(), before)

    def test_self_report_cannot_establish_verification_or_integration(self):
        for status in ("verified", "integrated"):
            value = manifest()
            value["packets"][0]["status"] = status
            packet = self.load(value)["packets"][0]
            self.assertEqual(packet["status"], "claimed")
            self.assertTrue(packet["warnings"])
            self.assertEqual(packet["reported_status"], status)

    def test_matching_provenance_distinguishes_observed_stages(self):
        value = manifest()
        packet = value["packets"][0]
        packet.update({"status": "integrated", "verification": verification(), "integration": integration()})
        state = self.load(value)
        self.assertEqual(state["counts"]["integrated"], 1)
        self.assertIn("not independently validated", state["assurance"])
        del packet["integration"]
        self.assertEqual(self.load(value)["packets"][0]["status"], "verified")

    def test_revision_mismatch_and_self_verifier_cannot_establish_acceptance(self):
        value = manifest()
        packet = value["packets"][0]
        packet.update({"status": "integrated", "candidate_revision": "different",
                       "verification": verification(), "integration": integration()})
        self.assertEqual(self.load(value)["packets"][0]["status"], "claimed")
        packet["candidate_revision"] = "abc123"
        packet["integration"] = integration("other")
        self.assertEqual(self.load(value)["packets"][0]["status"], "verified")
        packet["integration"] = integration()
        packet["claim"] = {"owner": "reviewer"}
        self.assertEqual(self.load(value)["packets"][0]["status"], "claimed")

    def test_active_alias_and_arbitrary_nodes_are_preserved(self):
        value = manifest()
        value["packets"][0]["status"] = "active"
        value["lanes"] = [{"id": "worker-1", "owner": "agent", "node": "my-laptop", "branch": "feature/a"}]
        state = self.load(value)
        self.assertEqual(state["packets"][0]["status"], "in_progress")
        self.assertEqual(state["lanes"][0]["node"], "my-laptop")

    def test_untrusted_links_are_discarded(self):
        for url in ("javascript:alert(1)", "data:text/html,bad", "https://user:secret@example.com", "//example.com"):
            value = manifest()
            value["pr"]["url"] = url
            value["project"]["repository_url"] = url
            state = self.load(value)
            self.assertEqual(state["pr"]["url"], "")
            self.assertEqual(state["project"]["repository_url"], "")

    def test_bounded_worktree_activity_survives_without_changing_packet_acceptance(self):
        value = manifest()
        value["lanes"] = [{"id": "worker", "status": "stale; reported integrated; owned scope files 2", "activity": {
            "source": "owned_worktree_metadata/v1", "state": "observed", "observed_utc": NOW.isoformat(),
            "head_sha": "a" * 40, "reported_head_matches": True, "owned_scope_files": 2,
            "scanned_entries": 3, "scope_paths": 1, "skipped_entries": 0, "bounded": False,
            "latest_owned_path_mtime_utc": NOW.isoformat(), "accepted": True, "command": "never execute",
            "filenames": ["private.rs"], "assurance": "forged acceptance"}}]
        state = self.load(value)
        activity = state["lanes"][0]["activity"]
        self.assertEqual(activity["owned_scope_files"], 2)
        self.assertEqual(activity["head_sha"], "a" * 40)
        self.assertNotIn("command", activity)
        self.assertNotIn("accepted", activity)
        self.assertNotIn("filenames", activity)
        self.assertIn("no agent liveness", activity["assurance"])
        self.assertEqual(state["packets"][0]["status"], "ready")
        self.assertIn("stale", state["lanes"][0]["status"])

    def test_malformed_worktree_activity_is_dropped_or_narrowed(self):
        value = manifest()
        base = {"source": "owned_worktree_metadata/v1", "state": "bounded", "observed_utc": NOW.isoformat(),
                "owned_scope_files": 2001, "scanned_entries": True, "scope_paths": -1,
                "head_sha": "not-a-head", "reported_head_matches": True, "bounded": "false"}
        value["lanes"] = [{"id": "worker", "activity": base}]
        activity = self.load(value)["lanes"][0]["activity"]
        self.assertEqual(set(activity), {"source", "state", "observed_utc", "assurance"})
        for key, bad in (("source", "unknown"), ("state", "accepted"), ("observed_utc", "bad")):
            value["lanes"][0]["activity"] = {**base, key: bad}
            self.assertNotIn("activity", self.load(value)["lanes"][0])


@unittest.skipUnless(shutil.which("node"), "Node required for actual dashboard JavaScript checks")
class RolloutUITests(unittest.TestCase):
    def render(self, payloads):
        html = (ROOT / "dashboard" / "index.html").read_text(encoding="utf-8")
        script = re.search(r"<script>(.*?)</script>", html, re.S).group(1)
        harness = """
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0,'utf8'));
const elements = {};
const context = {URL, console, setInterval:()=>{}, fetch:()=>new Promise(()=>{}),
 document:{getElementById:id=>elements[id]||(elements[id]={innerHTML:'',hidden:true})}};
vm.createContext(context);vm.runInContext(input.script,context);
const results = input.payloads.map(payload=>{context.payload=payload;
 vm.runInContext('rolloutPanel(payload)',context);return {...elements.rollout,summary:vm.runInContext('rolloutSummary(payload)',context)}});
process.stdout.write(JSON.stringify(results));
"""
        run = subprocess.run([shutil.which("node"), "-e", harness],
                             input=json.dumps({"script": script, "payloads": payloads}),
                             text=True, capture_output=True, timeout=10)
        self.assertEqual(run.returncode, 0, run.stderr)
        return json.loads(run.stdout)

    def test_no_manifest_hides_panel_and_clears_previous_state(self):
        result = self.render([{"rollout": {"state": "invalid", "error": "bad"}}, {}])
        self.assertFalse(result[0]["hidden"])
        self.assertTrue(result[1]["hidden"])
        self.assertEqual(result[1]["innerHTML"], "")

    def test_untrusted_manifest_content_is_escaped_in_rendered_dashboard(self):
        evil = '<img src=x onerror="alert(1)">'
        value = manifest()
        value["project"]["name"] = evil
        value["packets"][0].update({"title": evil, "lane": evil,
            "evidence": [{"kind": "candidate_receipt", "path": evil}]})
        value["pr"]["url"] = "javascript:alert(1)"
        value["lanes"] = [{"id": "lane", "node": evil}]
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "rollout.json").write_text(json.dumps(value))
            state = rollout.collect_rollout(directory, now=NOW, environ={})
        result = self.render([{"rollout": state}])[0]["innerHTML"]
        self.assertNotIn("<img", result)
        self.assertIn("&lt;img", result)
        self.assertNotIn('href="javascript:', result)
        self.assertIn("Pending packets (1)", result)
        self.assertIn("not independently validated", result)

    def test_ui_also_rejects_unsafe_links_without_loader(self):
        value = manifest()
        value.update({"state": "ready", "pr": {"url": "javascript:alert(1)"}})
        result = self.render([{"rollout": value}])[0]["innerHTML"]
        self.assertNotIn('href="javascript:', result)

    def test_execution_lanes_are_visible_with_separate_candidate_and_implementation_labels(self):
        value = manifest()
        value.update({"state": "ready", "lanes": [
            {"id": "adm_lane_ui", "owner": "/root/ui", "branch": "codex/ui", "status": "fresh; reported candidate_committed; screen not_exposed; gpt-6-luna medium; CPU reserved 104,105 NOT pinned"},
            {"id": "adm_lane_store", "status": "fresh; reported in_progress; screen not_exposed"},
        ]})
        result = self.render([{"rollout": value}])[0]["innerHTML"]
        self.assertIn('aria-label="Implementation agents"', result)
        self.assertIn("Implementation agents (2)", result)
        self.assertLess(result.index("Implementation agents (2)"), result.index("Pending packets (1)"))
        self.assertNotIn("<summary>Execution lanes", result)
        self.assertIn("<details><summary>Pending packets", result)
        self.assertNotIn("<details open><summary>Pending packets", result)
        self.assertIn("Candidate committed · awaiting review", result)
        self.assertIn("Implementing (reported)", result)
        self.assertIn("Process liveness not exposed", result)
        self.assertIn("CPU reserved 104,105 NOT pinned", result)
        self.assertIn("do not establish packet acceptance", result)

    def test_header_rollout_chip_links_to_visible_workers_with_reported_counts(self):
        value = manifest()
        value.update({"state": "ready", "lanes": [
            {"id": "one", "status": "fresh; reported candidate_committed; screen not_exposed"},
            {"id": "two", "status": "fresh; reported in_progress; screen not_exposed"},
        ]})
        summary = self.render([{"rollout": value}])[0]["summary"]
        self.assertIn('href="#rollout"', summary)
        self.assertIn("<b>2</b> agent reports", summary)
        self.assertIn("1 implementing · 1 candidate awaiting review", summary)
        self.assertIn("Example build", summary)
        self.assertEqual(self.render([{}])[0]["summary"], "")

    def test_lane_completion_and_screen_presence_do_not_display_verified_acceptance(self):
        value = manifest()
        value.update({"state": "ready", "lanes": [
            {"id": "worker", "status": "stale; reported verified; screen present"}
        ]})
        result = self.render([{"rollout": value}])[0]["innerHTML"]
        self.assertIn("Completion reported · evidence pending", result)
        self.assertIn("Stale lane report", result)
        self.assertIn("Screen present · task activity unknown", result)

    def test_actual_observer_statuses_survive_loader_and_remain_conservative(self):
        spec = importlib.util.spec_from_file_location("rollout_observe", ROOT / "tools/rollout_observe.py")
        observer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(observer)
        statuses = ["candidate_committed_unverified", "implementing_on_frozen_seed", "dispatched_switching_private_worktree", "candidate_committed_magic"]
        reports = {"updated_utc": NOW.isoformat(), "lanes": [
            {"lane": f"lane-{index}", "status": status, "screen": None, "agent_session": f"/root/lane-{index}"}
            for index, status in enumerate(statuses)
        ]}
        merged = observer.merge(manifest(), reports, {}, {}, now=NOW)
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "rollout.json").write_text(json.dumps(merged))
            normalized = rollout.collect_rollout(directory, now=NOW, environ={})
        self.assertNotIn("reported_status", normalized["lanes"][0])
        self.assertEqual(normalized["packets"][0]["status"], "ready")
        result = self.render([{"rollout": normalized}])[0]
        self.assertIn("Candidate committed · awaiting review", result["innerHTML"])
        self.assertIn("Implementing (reported)", result["innerHTML"])
        self.assertIn("Dispatched · preparing workspace", result["innerHTML"])
        self.assertIn("Reported: candidate_committed_magic", result["innerHTML"])
        self.assertIn("Process liveness not exposed", result["innerHTML"])
        self.assertIn("1 implementing · 1 candidate awaiting review · 1 preparing workspace", result["summary"])


if __name__ == "__main__":
    unittest.main()
