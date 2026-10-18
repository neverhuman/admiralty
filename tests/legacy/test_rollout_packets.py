"""Offline contract tests for prepared implementation missions; no fleet access."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rollout_packets", ROOT / "tools/rollout_packets.py")
rollout = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rollout)
BASE = "a" * 40
HEAD = "b" * 40


class RolloutTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="admiralty-rollout-test-")
        self.addCleanup(self.temporary.cleanup)
        self.output = rollout.owned_output(Path(self.temporary.name) / "out", ROOT, create=True)
        self.plan = (ROOT / rollout.PLAN_DIR / "WORK_PACKETS.md").read_text()

    def export(self):
        with rollout.output_lock(self.output):
            return rollout.export(ROOT, self.output, BASE)

    def claimed(self):
        manifest = self.export()
        rollout.claim(manifest, "P00", "worker-1", "evidence-1", "node-any", "codex/admiralty-p00")
        return manifest

    def receipt(self):
        return {"schema_version": 1, "packet_id": "P00", "owner": "worker-1", "base_sha": BASE,
                "head_sha": HEAD, "interface_version": "unimplemented",
                "changed_paths": ["tests/fixtures/legacy/case.json"],
                "criteria": [{"id": "fixture", "status": "passed", "evidence": "synthetic offline fixture"}],
                "commands": [{"argv": ["python3", "-m", "unittest"], "cwd": "/owned/worktree", "exit_code": 0}],
                "environment": {"python": "test-only"}, "artifacts": [], "risks": [], "blockers": [],
                "next_action": "Independent exact-candidate verification"}

    def test_all_plan_packets_export_with_small_capsules_and_matching_contract(self):
        manifest = self.export()
        self.assertEqual(set(p["id"] for p in manifest["packets"]), set(rollout.ALLOWLISTS))
        self.assertEqual([p["id"] for p in manifest["packets"] if p["status"] == "ready"], ["P00", "P01"])
        self.assertEqual(manifest["producer"], "admiralty.rollout_packets/v1")
        self.assertEqual(manifest["pr"], {"head": BASE, "url": None})
        self.assertEqual(manifest["lanes"], [])
        rollout.validate_manifest(manifest)
        rollout.verify_capsules(self.output, manifest)
        for packet in manifest["packets"]:
            data = (self.output / packet["capsule_path"]).read_bytes()
            self.assertLess(len(data), 6000)
            self.assertIn(BASE.encode(), data)
            self.assertIn(b"Exact write allowlist", data)
            self.assertIn(b"next_action", data)
            self.assertIn(b"No live provider", data)
            self.assertIsNone(packet["verification"])
            self.assertIsNone(packet["integration"])

    def test_missing_required_plan_fields_rejected(self):
        for field in ("Depends on", "Owner", "Release", "Read first", "Write scope", "Validate", "Pass"):
            with self.subTest(field=field):
                changed = self.plan.replace(f"**{field}:**", "REMOVED", 1)
                with self.assertRaises(rollout.Invalid):
                    rollout.parse_plan(changed)

    def test_scope_changes_fail_closed(self):
        changed = self.plan.replace("`tests/fixtures/legacy/`", "`tests/fixtures/`", 1)
        with self.assertRaisesRegex(rollout.Invalid, "scope changed"):
            rollout.parse_plan(changed)

    def test_cycle_rejected(self):
        packets = rollout.parse_plan(self.plan)
        packets[0]["depends_on"] = ["P02"]
        with self.assertRaisesRegex(rollout.Invalid, "cycle"):
            rollout.validate_graph(packets)

    def test_unknown_duplicate_and_unsafe_ids_rejected(self):
        packets = rollout.parse_plan(self.plan)
        for bad in ("../P00", "P32", "P00\n", "P00/evil"):
            with self.subTest(bad=bad):
                mutated = copy.deepcopy(packets)
                mutated[0]["id"] = bad
                with self.assertRaises(rollout.Invalid):
                    rollout.validate_graph(mutated)
        packets[0]["depends_on"] = ["P99"]
        with self.assertRaises(rollout.Invalid):
            rollout.validate_graph(packets)

    def test_full_range_and_conditional_dependencies_kept(self):
        packets = rollout.parse_plan(self.plan)
        self.assertEqual(packets[31]["depends_on"], [f"P{i:02}" for i in range(31)])
        self.assertIn("P18", packets[19]["depends_on"])
        self.assertIn("P29", packets[30]["depends_on"])

    def test_paths_are_exact_and_traversal_rejected(self):
        for bad in ("../secret", "a/../secret", "/etc/passwd", ".git/config", "a//b", "a\\b", "a/./b", "x\ny"):
            with self.subTest(bad=bad), self.assertRaises(rollout.Invalid):
                rollout.safe_path(bad)
        self.assertTrue(rollout.allows("tests/fixtures/legacy/deep/f.json", rollout.allowlist("P00")))
        self.assertFalse(rollout.allows("tests/fixtures/legacy-other/f.json", rollout.allowlist("P00")))
        self.assertFalse(rollout.allows("crates/admiralty-store/src/writer.rs/evil", rollout.allowlist("P03")))
        self.assertNotIn("Cargo.toml", rollout.allowlist("P05"))

    def test_state_requires_integrated_dependencies_and_claim(self):
        manifest = self.export()
        manifest["packets"][2]["status"] = "ready"
        with self.assertRaisesRegex(rollout.Invalid, "not integrated"):
            rollout.validate_manifest(manifest)
        manifest["packets"][2]["status"] = "planned"
        manifest["packets"][0]["status"] = "claimed"
        with self.assertRaisesRegex(rollout.Invalid, "explicit claim"):
            rollout.validate_manifest(manifest)

    def test_verified_and_integrated_require_distinct_proof(self):
        manifest = self.claimed()
        packet = manifest["packets"][0]
        packet["status"] = "verified"
        with self.assertRaisesRegex(rollout.Invalid, "independent provenance"):
            rollout.validate_manifest(manifest)
        packet["verification"] = {"receipt": "proof.json", "revision": HEAD, "verifier": "independent", "observed_utc": "2026-10-18T00:00:00Z"}
        rollout.validate_manifest(manifest)
        packet["status"] = "integrated"
        with self.assertRaisesRegex(rollout.Invalid, "integration provenance"):
            rollout.validate_manifest(manifest)

    def test_claims_cannot_duplicate_or_skip_readiness(self):
        manifest = self.claimed()
        with self.assertRaisesRegex(rollout.Invalid, "only ready"):
            rollout.claim(manifest, "P00", "worker-2", "evidence-2", None, "codex/second")
        with self.assertRaisesRegex(rollout.Invalid, "active packet"):
            rollout.claim(manifest, "P01", "worker-1", "evidence-1", None, "codex/second")
        with self.assertRaisesRegex(rollout.Invalid, "only ready"):
            rollout.claim(manifest, "P02", "worker-2", "core", None, "codex/core")

    def test_receipt_import_is_idempotent_and_never_trusted(self):
        manifest = self.claimed()
        receipt = self.receipt()
        receipt["status"] = "integrated"
        rollout.reconcile(self.output, manifest, receipt)
        rollout.reconcile(self.output, manifest, receipt)
        packet = manifest["packets"][0]
        self.assertEqual(packet["status"], "review")
        self.assertIsNone(packet["verification"])
        self.assertIsNone(packet["integration"])
        self.assertEqual(len(packet["evidence"]), 1)
        self.assertEqual(packet["evidence"][0]["trust"], "worker_report_unverified")
        self.assertEqual(manifest["packets"][2]["status"], "planned")
        self.assertEqual(rollout.read_json(self.output / "rollout.json"), manifest)

    def test_receipts_require_scope_base_owner_and_complete_command_evidence(self):
        manifest = self.claimed()
        mutations = [
            ("changed_paths", ["Cargo.toml"]), ("changed_paths", ["tests/fixtures/legacy/../../evil"]),
            ("base_sha", "c" * 40), ("head_sha", "short"), ("owner", "impostor"),
            ("commands", [{"argv": ["test"], "cwd": "/tmp", "exit_code": True}]),
            ("criteria", []), ("artifacts", [{"path": "output", "sha256": "fake"}]),
        ]
        for key, value in mutations:
            with self.subTest(key=key, value=value):
                receipt = self.receipt()
                receipt[key] = value
                with self.assertRaises(rollout.Invalid):
                    rollout.validate_receipt(receipt, manifest)
        receipt = self.receipt()
        del receipt["environment"]
        with self.assertRaisesRegex(rollout.Invalid, "missing required"):
            rollout.validate_receipt(receipt, manifest)

    def test_receipt_cannot_establish_a_claim(self):
        with self.assertRaisesRegex(rollout.Invalid, "explicit claim"):
            rollout.validate_receipt(self.receipt(), self.export())

    def test_output_must_be_external_owned_empty_and_no_symlinks(self):
        with self.assertRaisesRegex(rollout.Invalid, "outside repository"):
            rollout.owned_output(ROOT / "runtime/rollout-test", ROOT, create=True)
        other = Path(self.temporary.name) / "foreign"
        other.mkdir()
        (other / "valuable.txt").write_text("preserve")
        with self.assertRaisesRegex(rollout.Invalid, "empty or initialized"):
            rollout.owned_output(other, ROOT, create=True)
        self.assertEqual((other / "valuable.txt").read_text(), "preserve")
        link = Path(self.temporary.name) / "link"
        link.symlink_to(self.output, target_is_directory=True)
        with self.assertRaisesRegex(rollout.Invalid, "symlink"):
            rollout.owned_output(link, ROOT, create=True)

    def test_duplicate_json_keys_and_non_finite_numbers_rejected(self):
        path = Path(self.temporary.name) / "bad.json"
        for text in ('{"id":1,"id":2}', '{"value":NaN}', '{"value":Infinity}'):
            path.write_text(text)
            with self.assertRaises(rollout.Invalid):
                rollout.read_json(path)

    def test_failed_atomic_replacement_preserves_previous_manifest(self):
        self.export()
        path = self.output / "rollout.json"
        before = path.read_bytes()
        real_replace = rollout.os.replace
        def fail_manifest(source, target):
            if Path(target) == path:
                raise OSError("synthetic crash before manifest publication")
            return real_replace(source, target)
        with patch.object(rollout.os, "replace", side_effect=fail_manifest):
            with self.assertRaisesRegex(OSError, "synthetic crash"):
                rollout.export(ROOT, self.output, BASE)
        self.assertEqual(path.read_bytes(), before)
        rollout.verify_capsules(self.output, rollout.read_json(path))

    def test_reexport_preserves_claim_and_old_generation_references(self):
        manifest = self.claimed()
        rollout.atomic_write(self.output / "rollout.json", rollout.json_bytes(manifest))
        again = self.export()
        self.assertEqual(again["packets"][0]["status"], "claimed")
        self.assertNotEqual(again["generation"], manifest["generation"])
        rollout.verify_capsules(self.output, manifest)
        rollout.verify_capsules(self.output, again)

    def test_capsule_corruption_blocks_mutation_before_manifest_write(self):
        manifest = self.export()
        path = self.output / "rollout.json"
        before = path.read_bytes()
        (self.output / manifest["packets"][0]["capsule_path"]).write_text("corrupt")
        code = rollout.main(["--repo", str(ROOT), "claim", "--out", str(self.output), "--packet", "P00", "--owner", "worker", "--lane", "lane", "--branch", "codex/test"])
        self.assertEqual(code, 2)
        self.assertEqual(path.read_bytes(), before)

    def test_export_has_no_dispatch_calls_and_rejects_new_base(self):
        with patch.object(rollout.subprocess, "run", side_effect=AssertionError("unexpected process")):
            self.export()
        with self.assertRaisesRegex(rollout.Invalid, "source/base/project changed"):
            rollout.export(ROOT, self.output, HEAD)


if __name__ == "__main__":
    unittest.main()
