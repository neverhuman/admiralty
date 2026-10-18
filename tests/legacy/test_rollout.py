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
 vm.runInContext('rolloutPanel(payload)',context);return {...elements.rollout}});
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


if __name__ == "__main__":
    unittest.main()
