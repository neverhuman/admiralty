import importlib.util, json, os, pathlib, subprocess, tempfile, unittest
_p = pathlib.Path(__file__).resolve().parents[1] / "tools" / "sc_audit.py"
_spec = importlib.util.spec_from_file_location("sc_audit", _p)
sc = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(sc)

def git(repo, *a):
    subprocess.run(["git", "-C", repo, *a], check=True, capture_output=True,
                   env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"})

def write(repo, path, obj):
    p = pathlib.Path(repo, path); p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(obj if isinstance(obj, str) else json.dumps(obj))

def prog(tasks):
    return {"schema_version": 1, "packet": "V5-90", "tasks": tasks}

class JoilFlip(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp(); r = self.d
        git(r, "init", "-q", "-b", "main")
        write(r, "docs/v5-plan/progress/V5-90.json", prog({
            "V5-90-01": {"status": "blocked", "note": "Cause B: measured data absent"},
            "V5-90-02": {"status": "blocked", "note": "Cause B: measured data absent"},
            "V5-90-03": {"status": "todo", "note": ""}}))
        write(r, "crates/a/src/lib.rs", "pub fn a() {}\n")
        git(r, "add", "-A"); git(r, "commit", "-qm", "base")
        sc.findings.clear()

    def flip(self, prod_lines, labelled=False, open_summary=False, own_receipt=False):
        r = self.d
        lbl = " substitute:proxy" if labelled else ""
        write(r, "docs/v5-plan/progress/V5-90.json", prog({
            "V5-90-01": {"status": "done", "note": "verified" + lbl, "evidence": [{"path": "docs/receipts/pk.json"}]},
            "V5-90-02": {"status": "done", "note": "verified" + lbl, "evidence": [{"path": "docs/receipts/pk.json"}]},
            "V5-90-03": {"status": "done", "note": "verified", "evidence": [{"path": "docs/receipts/pk.json"}]}}))
        write(r, "docs/receipts/pk.json", {"task_ids": ["V5-90-01", "V5-90-02", "V5-90-03"], "commands": []})
        if own_receipt:
            write(r, "docs/receipts/own.json", {"task_ids": ["V5-90-01"], "commands": []})
        write(r, "docs/release/v5/FLEET_ADMIRAL_DISPOSITIONS_X.json", {"decisions": [
            {"decision_id": "SC-DEC-900", "task_id": "V5-90-01", "disposition_mode": "substitute:proxy",
             "summary": "planted negatives not yet executed, so verification remains open" if open_summary else "ok"}]})
        write(r, "crates/a/src/lib.rs", "pub fn a() {}\n" + "".join(f"pub fn f{i}() {{}}\n" for i in range(prod_lines))
              + "#[cfg(test)]\nmod tests {\n" + "".join(f"fn t{i}() {{}}\n" for i in range(200)) + "}\n")
        write(r, "crates/a/tests/t.rs", "\n".join("// t" for _ in range(500)))
        git(r, "add", "-A"); git(r, "commit", "-qm", "flip")
        return sc.audit_joil_flips(r, "main", emit=True)

    def test_flip_with_only_tests_and_receipts_is_red(self):
        res = self.flip(prod_lines=3)
        self.assertIn("JOIL_DONE_STATUS_FLIP", res["flags"]["V5-90-01"])
        self.assertIn("JOIL_DONE_STATUS_FLIP", res["flags"]["V5-90-02"])
        self.assertNotIn("JOIL_DONE_STATUS_FLIP", res["flags"].get("V5-90-03", []))  # never externally blocked
        self.assertEqual(res["done_on_main"], 3)
        self.assertEqual(res["done_unflagged_on_main"], 1)
        self.assertTrue(any(f["code"] == "JOIL_DONE_STATUS_FLIP" and f["severity"] == "red" for f in sc.findings))

    def test_cfg_test_lines_do_not_count_as_production(self):
        total, per = sc.joil_prod_added_lines(self.d, "main", "main")
        self.assertEqual(total, 0)
        self.flip(prod_lines=3)
        total, per = sc.joil_prod_added_lines(self.d, "main~1", "main")
        self.assertEqual(total, 3)

    def test_real_implementation_is_not_a_flip(self):
        res = self.flip(prod_lines=60, labelled=True, own_receipt=True)
        self.assertNotIn("JOIL_DONE_STATUS_FLIP", res["flags"].get("V5-90-01", []))
        self.assertNotIn("JOIL_SUBSTITUTE_UNLABELLED", res["flags"].get("V5-90-01", []))
        self.assertNotIn("JOIL_DONE_SHARED_RECEIPT", res["flags"].get("V5-90-01", []))
        self.assertIn("JOIL_DONE_SHARED_RECEIPT", res["flags"]["V5-90-02"])

    def test_unlabelled_substitute_and_open_decision_are_red(self):
        res = self.flip(prod_lines=60, open_summary=True)
        self.assertIn("JOIL_SUBSTITUTE_UNLABELLED", res["flags"]["V5-90-01"])
        self.assertIn("JOIL_DONE_DECISION_SAYS_OPEN", res["flags"]["V5-90-01"])
        self.assertIn("V5-90-01", res["red_flagged_tasks"])

    def test_summary_only_log_detection(self):
        f = pathlib.Path(self.d, "l.log"); f.write_text("cargo test: 1 passed, 18 filtered out (1 suite, 0.00s)\n")
        self.assertTrue(sc._joil_log_is_summary(str(f)))
        f.write_text("running 1 test\ntest v5_x ... ok\n")
        self.assertFalse(sc._joil_log_is_summary(str(f)))
        self.assertIsNone(sc._joil_log_is_summary(str(f) + ".missing"))

    def test_own_receipt_with_resolution_and_full_log_waives_flip(self):
        log = pathlib.Path(self.d, "full.log"); log.write_text("running 1 test\ntest v5_90_01 ... ok\n")
        r = self.d
        write(r, "docs/receipts/own.json", {"task_ids": ["V5-90-01"], "decision_resolution": "public dataset X accessioned, sha256 ...",
              "commands": [{"command": "cargo test -p a v5_90_01 --exact", "exit_code": 0, "log_path": str(log)}]})
        git(r, "add", "-A"); git(r, "commit", "-qm", "own receipt")
        res = self.flip(prod_lines=3, labelled=True, own_receipt=False)
        self.assertNotIn("JOIL_DONE_STATUS_FLIP", res["flags"].get("V5-90-01", []))
        self.assertIn("JOIL_DONE_STATUS_FLIP", res["flags"]["V5-90-02"])
        log.write_text("cargo test: 1 passed, 18 filtered out (1 suite, 0.00s)\n")
        sc.findings.clear()
        res = sc.audit_joil_flips(r, "main", emit=False)
        self.assertIn("JOIL_DONE_STATUS_FLIP", res["flags"]["V5-90-01"])  # summary-only log does not waive

if __name__ == "__main__":
    unittest.main()
