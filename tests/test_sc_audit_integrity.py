"""Offline regression checks for audit counts and actionable lane state."""

import datetime
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "sc_audit_integrity", Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
)
sc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sc)
NOW = datetime.datetime(2026, 10, 18, 14, 0, tzinfo=datetime.timezone.utc)
OLD = "2026-10-18T12:00:00Z"
HEAD = "a" * 40


def verification(**changes):
    value = {
        "receipt": "receipts/P01/check.json",
        "revision": HEAD,
        "verifier": "independent-reviewer",
        "observed_utc": "2026-10-18T13:59:00Z",
    }
    value.update(changes)
    return value


class AdmiraltyIntegrity(unittest.TestCase):
    def audit(self, packets=(), lanes=(), screens=None, prev=None):
        sc.findings.clear()
        documents = {
            sc.ADM + "/rollout.json": {"updated_utc": NOW.isoformat(), "packets": list(packets)},
            sc.ADM + "/live-lanes.json": {"lanes": list(lanes)},
        }
        with (
            mock.patch.object(sc, "rj", side_effect=documents.get),
            mock.patch.object(sc, "hardcopy", return_value="working on bounded task"),
            mock.patch.object(sc, "run", side_effect=AssertionError("no live commands in fixture")),
        ):
            result = sc.audit_admiralty(screens or {}, prev or {}, {}, NOW)
        return result, {finding["code"] for finding in sc.findings}

    def test_truthy_nonrecords_never_count_integrated_packets(self):
        for proof in (True, 1, "verified", "false", "unknown", "unverified", ["passed"],
                      {"status": "verified"}, {"receipt": "prose"}, {}):
            with self.subTest(proof=proof):
                result, codes = self.audit([{"id": "P01", "status": "integrated", "verification": proof}])
                self.assertEqual(result["done_recomputed"], 0)
                self.assertIn("DONE_NO_EVIDENCE", codes)

    def test_complete_record_preserves_valid_positive_and_verified_is_not_done(self):
        for extra in ({}, {"status": "passed"}, {"result": "success"},
                      {"verified": True, "passed": True}):
            with self.subTest(extra=extra):
                packets = [{"id": status, "status": status, "verification": verification(**extra)}
                           for status in ("integrated", "verified", "in_progress")]
                result, codes = self.audit(packets)
                self.assertEqual(result["done_recomputed"], 1)
                self.assertEqual(result["verified_or_integrated_claimed"], 2)
                self.assertNotIn("DONE_NO_EVIDENCE", codes)
                self.assertIn("not independently verified", result["note"])

    def test_missing_malformed_and_placeholder_provenance_rejected(self):
        proofs = []
        for key in ("receipt", "revision", "verifier", "observed_utc"):
            proof = verification()
            del proof[key]
            proofs.append(proof)
            for invalid in (False, "", "unknown", "unverified", "false", "x\nforged", "x" * 1001):
                proofs.append(verification(**{key: invalid}))
        proofs.append(verification(revision="abc123"))
        for proof in proofs:
            with self.subTest(proof=proof):
                result, codes = self.audit([{"id": "P01", "status": "integrated", "verification": proof}])
                self.assertEqual(result["done_recomputed"], 0)
                self.assertIn("DONE_NO_EVIDENCE", codes)

    def test_explicit_negative_or_unknown_outcome_never_counts(self):
        for key in ("status", "result", "outcome"):
            for outcome in (False, True, "false", "unverified", "not_verified", "unknown", "pending", "failed"):
                with self.subTest(key=key, outcome=outcome):
                    result, _ = self.audit([{"id": "P01", "status": "integrated",
                                            "verification": verification(**{key: outcome})}])
                    self.assertEqual(result["done_recomputed"], 0)
        for key in ("passed", "verified", "success"):
            for outcome in (False, 1, "true", "unknown"):
                with self.subTest(key=key, outcome=outcome):
                    result, _ = self.audit([{"id": "P01", "status": "integrated",
                                            "verification": verification(**{key: outcome})}])
                    self.assertEqual(result["done_recomputed"], 0)

    def test_present_candidate_integration_and_claimant_must_agree(self):
        for fields in ({"candidate_revision": "b" * 40},
                       {"integration": {"revision": "b" * 40}},
                       {"claim": {"owner": "independent-reviewer"}}):
            with self.subTest(fields=fields):
                result, _ = self.audit([{"id": "P01", "status": "integrated",
                                        "verification": verification(), **fields}])
                self.assertEqual(result["done_recomputed"], 0)
        result, _ = self.audit([{"id": "P01", "status": "integrated", "verification": verification(),
                                 "candidate_revision": HEAD, "integration": {"revision": HEAD},
                                 "claim": {"owner": "task-worker"}}])
        self.assertEqual(result["done_recomputed"], 1)

    def test_unknown_naive_or_future_verification_time_rejected(self):
        for observed in ("not-a-time", "2026-10-18T13:00:00", "2026-10-18T14:01:00Z"):
            with self.subTest(observed=observed):
                result, _ = self.audit([{"id": "P01", "status": "integrated",
                                        "verification": verification(observed_utc=observed)}])
                self.assertEqual(result["done_recomputed"], 0)

    def test_substring_terminal_words_do_not_hide_dead_lanes(self):
        for status in ("candidate_committed_unverified", "not_verified", "unfinished", "not_closed",
                       "verified_pending", "integrated_pending", "released_pending", "unknown", False):
            with self.subTest(status=status):
                result, codes = self.audit(lanes=[{"screen": "123.worker", "status": status,
                                                  "packet_id": "P01", "updated_utc": OLD}])
                self.assertFalse(result["workers"]["worker"]["alive"])
                self.assertIn("LANE_DEAD", codes)
                if status == "candidate_committed_unverified":
                    self.assertIn("UNVERIFIED_CANDIDATES_AGING", codes)

    def test_unverified_and_unknown_live_lanes_remain_stale(self):
        for status in ("candidate_committed_unverified", "not_verified", "unknown"):
            with self.subTest(status=status):
                _, codes = self.audit(lanes=[{"screen": "123.worker", "status": status,
                                             "packet_id": "P01", "updated_utc": OLD}], screens={"worker": 123})
                self.assertIn("LANE_STALE", codes)
                self.assertNotIn("LANE_DEAD", codes)

    def test_only_exact_terminal_states_preserve_lane_closeout(self):
        for status in sc.ADM_FINISHED:
            for alive in (False, True):
                with self.subTest(status=status, alive=alive):
                    _, codes = self.audit(lanes=[{"screen": "worker", "status": f" {status.upper()} ",
                                                 "updated_utc": OLD}], screens={"worker": 123} if alive else {})
                    self.assertNotIn("LANE_DEAD", codes)
                    self.assertNotIn("LANE_STALE", codes)


class DopeIntegrity(unittest.TestCase):
    def setUp(self):
        sc.findings.clear()
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "verify").mkdir()

    def task(self, task_id, sub_status):
        log = self.root / "verify" / f"{task_id}.log"
        log.write_text(f"head_sha: {HEAD}\nexit_code: 0\n{task_id}\nRan 1 test in 0.001s\nOK\n")
        receipt = self.root / "verify" / f"{task_id}.json"
        receipt.write_text(json.dumps({"task_id": task_id, "status": "passed", "head_sha": HEAD,
                                       "commands": [{"cmd": "python3 -m unittest task_checks", "exit_code": 0,
                                                     "head_sha": HEAD, "log_path": str(log),
                                                     "log_sha256": hashlib.sha256(log.read_bytes()).hexdigest()}]}))
        return {"id": task_id, "status": "done", "sub_status": sub_status,
                "evidence": [str(receipt) + "#sha256=" + hashlib.sha256(receipt.read_bytes()).hexdigest()]}

    def test_blocked_partial_and_failed_cannot_count_despite_valid_receipts(self):
        tasks = [self.task("GOOD", "completed")]
        tasks.extend(self.task(f"BAD{index}", status) for index, status in enumerate(
            ("BLOCKED: needs owner input", "partial implementation", "failed final check")))
        # The receipts really do pass the independent receipt checks.
        for task in tasks:
            path, pin = sc.dope_receipt_refs(task)[0]
            self.assertEqual(sc.check_dope_receipt(task["id"], path, pin, lambda sha: sha == HEAD)[0], [])
        (self.root / "tasks.json").write_text(json.dumps({"tasks": tasks}))
        with mock.patch.object(sc, "max_claim", return_value=None):
            result = sc.audit_dope(str(self.root), on_main=lambda sha: sha == HEAD)
        self.assertEqual(result["ledger_done"], 4)
        self.assertEqual(result["done_recomputed"], 1)
        self.assertEqual(result["unverified_done"], ["BAD0", "BAD1", "BAD2"])
        for task_id in result["unverified_done"]:
            self.assertIn("contradictory_sub_status", result["receipt_problems"][task_id])
        self.assertIn("DONE_BUT_SUBSTATUS_BLOCKED", {finding["code"] for finding in sc.findings})

    def test_contradiction_reason_retained_alongside_missing_receipt(self):
        verified, problems, _ = sc.verify_dope_tasks(
            [{"id": "BAD", "status": "done", "sub_status": "blocked", "evidence": []}],
            lambda sha: True,
        )
        self.assertEqual(verified, [])
        self.assertEqual(set(problems["BAD"]), {"no_receipt", "contradictory_sub_status"})


if __name__ == "__main__":
    unittest.main()
