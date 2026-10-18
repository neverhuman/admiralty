"""Unit tests for JopeDime major-release (2026-10-18) tracker adapter in sc_audit.py."""

import datetime
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "sc_audit_mr", Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
)
sc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sc)

MAIN = "a2759e9d375d0697ed40a32c23ac297b5de42347"


class JopeDimeMajorReleaseAuditTest(unittest.TestCase):
    def setUp(self):
        sc.findings.clear()

    def test_valid_receipt_and_dependency_enforcement(self):
        with tempfile.TemporaryDirectory() as tmp:
            vdir = Path(tmp) / "verify"
            vdir.mkdir()
            log_file = Path(tmp) / "mr01.log"
            log_bytes = b"test result: ok. 1 passed; 0 failed\n"
            log_file.write_bytes(log_bytes)
            log_sha = hashlib.sha256(log_bytes).hexdigest()

            tracker = {
                "schema": "jope.major_release_tracker.v1",
                "verify_dir": str(vdir),
                "tasks": [
                    {
                        "id": "MR01",
                        "owner_screen": "2830191.jopedime_agy",
                        "deps": [],
                        "gate": "G00",
                        "exact_acceptance_test": "cargo test -p jope-records --test i53_i62_executable_receipts -- --exact i53_and_i62_contracts_execute",
                        "acceptance_cases": ["i53_executable_contract_test"],
                        "receipt_format": {"schema": "jope.major_release_receipt.v1"},
                    },
                    {
                        "id": "MR02",
                        "owner_screen": "2724936.captain_jopedime",
                        "deps": ["MR01"],
                        "gate": "G00",
                        "exact_acceptance_test": "cargo test -p jope-engine --test mr02 -- --exact mr02_case",
                        "acceptance_cases": ["mr02_case"],
                        "receipt_format": {"schema": "jope.major_release_receipt.v1"},
                    },
                ],
            }
            tpath = Path(tmp) / "tasks.json"
            tpath.write_text(json.dumps(tracker))

            r1 = {
                "id": "MR01",
                "status": "PASS",
                "sha": MAIN,
                "checkout_sha": MAIN,
                "verified_at_utc": "2026-10-18T16:30:00Z",
                "case_to_test_map": [
                    {
                        "acceptance_case": "i53_executable_contract_test",
                        "command": "cargo test -p jope-records --test i53_i62_executable_receipts -- --exact i53_and_i62_contracts_execute",
                        "test_or_code": "i53_and_i62_contracts_execute",
                        "exit_code": 0,
                        "passed": True,
                        "log_path": str(log_file),
                        "log_sha256": log_sha,
                    }
                ],
            }
            (vdir / "MR01.json").write_text(json.dumps(r1))

            with (
                mock.patch.object(sc, "JE_MR", tmp),
                mock.patch.object(sc, "commit_time", return_value=datetime.datetime(2026, 10, 18, 15, 0, tzinfo=datetime.timezone.utc)),
            ):
                res = sc.audit_jopedime_major_release("/nonexistent-repo", MAIN)
                self.assertEqual(res["total"], 2)
                self.assertEqual(res["done_recomputed"], 1)
                self.assertEqual(res["suspect"], {})

                # Now remove MR01 and add MR02 (whose dep MR01 is not passed)
                (vdir / "MR01.json").unlink()
                r2 = dict(r1, id="MR02")
                r2["case_to_test_map"] = [
                    dict(
                        r1["case_to_test_map"][0],
                        acceptance_case="mr02_case",
                        command="cargo test -p jope-engine --test mr02 -- --exact mr02_case",
                    )
                ]
                (vdir / "MR02.json").write_text(json.dumps(r2))
                sc.findings.clear()
                res2 = sc.audit_jopedime_major_release("/nonexistent-repo", MAIN)
                self.assertEqual(res2["done_recomputed"], 0)
                self.assertIn("MR02", res2["suspect"].get("deps_not_passed", []))


if __name__ == "__main__":
    unittest.main()
