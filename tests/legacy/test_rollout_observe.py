import copy
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rollout_observe", ROOT / "tools/rollout_observe.py")
observer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(observer)
NOW = dt.datetime(2026, 10, 18, 12, 0, tzinfo=dt.timezone.utc)


class ObserverTests(unittest.TestCase):
    def template(self):
        return {"schema_version": 1, "project": {"id": "arbitrary-project"},
                "updated_utc": "2026-10-18T10:00:00Z", "producer": "integration-owner",
                "packets": [{"id": "P00", "status": "integrated", "verification": {"receipt": "real"}, "integration": {"receipt": "real"}},
                            {"id": "P01", "status": "planned", "verification": None, "integration": None}],
                "lanes": [{"id": "pending-lane", "owner": "captain", "status": "claimed"}]}

    def merge(self, reports, screens=None):
        return observer.merge(self.template(), reports, {}, {}, now=NOW, prefix="adm_lane_", screens=screens)

    def test_live_lanes_do_not_promote_packets_or_freshen_source(self):
        template = self.template()
        report = {"updated_utc": "2026-10-18T12:00:00Z", "lanes": [{"id": "worker", "screen": "123.adm_lane_core", "packet": "P01", "status": "integrated", "verification": {"receipt": "forged"}}]}
        result = self.merge(report, {"available": True, "sessions": {"adm_lane_core": 123}})
        self.assertEqual(result["packets"], template["packets"])
        self.assertEqual(result["updated_utc"], template["updated_utc"])
        self.assertEqual(result["producer"], template["producer"])
        lane = next(row for row in result["lanes"] if row["id"] == "worker")
        self.assertEqual(lane["screen_state"], "present")
        self.assertEqual(lane["observation_state"], "fresh")
        self.assertEqual(result["observer"]["observed_utc"], "2026-10-18T12:00:00+00:00")

    def test_stale_orphaned_pending_and_unknown_are_explicit(self):
        reports = [{"id": "old", "updated_utc": "2026-10-18T10:00:00Z", "screen": "adm_lane_old", "status": "running"},
                   {"id": "gone", "updated_utc": "2026-10-18T12:00:00Z", "screen": "adm_lane_gone"}]
        result = self.merge(reports, {"available": True, "sessions": {"adm_lane_old": 10}})
        rows = {row["id"]: row for row in result["lanes"]}
        self.assertEqual(rows["old"]["observation_state"], "stale")
        self.assertEqual(rows["gone"]["observation_state"], "orphaned")
        self.assertEqual(rows["pending-lane"]["observation_state"], "pending")
        result = self.merge(reports, {"available": False, "sessions": {}})
        self.assertTrue(all(row["screen_state"] != "absent" for row in result["lanes"]))

    def test_pid_mismatch_and_out_of_scope_sessions(self):
        result = self.merge([{"id": "wrong", "screen": "12.adm_lane_core"}, {"id": "foreign", "screen": "captain_other"}], {"available": True, "sessions": {"adm_lane_core": 13, "captain_other": 14}})
        rows = {row["id"]: row for row in result["lanes"]}
        self.assertEqual(rows["wrong"]["screen_state"], "identity_mismatch")
        self.assertEqual(rows["foreign"]["screen_state"], "out_of_scope")

    def test_duplicate_lanes_are_stable_and_conflicts_do_not_look_healthy(self):
        rows = [{"id": "worker", "status": "running"}, {"id": "worker", "status": "done"}]
        first = self.merge(rows)
        second = self.merge(list(reversed(rows)))
        self.assertEqual(first["lanes"], second["lanes"])
        worker = next(row for row in first["lanes"] if row["id"] == "worker")
        self.assertEqual(worker["observation_state"], "pending")
        self.assertIn("conflicting duplicate", worker["status"])
        self.assertEqual(len(self.merge([rows[0], rows[0]])["lanes"]), 2)

    def test_invalid_shapes_ids_and_limits(self):
        for bad in ({"lanes": {}}, [{"id": "../escape"}], [{"id": "x", "screen_pid": -1}], [dict(id=f"x{i}") for i in range(201)]):
            with self.subTest(bad=str(bad)[:50]), self.assertRaises(observer.Invalid):
                self.merge(bad)

    def test_json_inputs_are_regular_bounded_owned_and_no_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source = directory / "source.json"
            source.write_text(json.dumps(self.template()))
            observer.read_owned(source)
            link = directory / "link.json"
            link.symlink_to(source)
            with self.assertRaises(observer.Invalid):
                observer.read_owned(link)
            with self.assertRaises(observer.Invalid):
                observer.read_owned(directory / ".." / "source.json")
            source.write_bytes(b" " * (observer.MAX_BYTES + 1))
            with self.assertRaises(observer.Invalid):
                observer.read_owned(source)
            source.write_text('{"a":1,"a":2}')
            with self.assertRaises(observer.Invalid):
                observer.read_owned(source)

    def test_provider_internal_sessions_remain_unknown_and_cpu_reservation_is_not_pinning(self):
        reports = {"launch_method": "provider-internal subagents", "updated_utc": "2026-10-18T12:00:00Z", "lanes": [
            {"lane": "adm_lane_1_ui", "task": "ADM-P12A", "model": "gpt-6-luna", "effort": "medium", "tier": "T0",
             "screen": None, "pid": None, "agent_session": "/root/adm_lane_1_ui", "status": "candidate_committed",
             "reserved_cpus": [104, 105], "cpus_pinned": False}]}
        result = self.merge(reports, {"available": True, "sessions": {}})
        lane = next(row for row in result["lanes"] if row["id"] == "adm_lane_1_ui")
        self.assertEqual(lane["screen_state"], "not_exposed")
        self.assertNotEqual(lane["observation_state"], "orphaned")
        self.assertIn("candidate_committed", lane["status"])
        self.assertIn("NOT pinned", lane["status"])
        self.assertEqual(lane["packet_id"], "ADM-P12A")
        self.assertIsNone(lane["screen_pid"])
        self.assertEqual(result["packets"], self.template()["packets"])
    def test_missing_lane_source_is_pending_without_subprocess(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source, lanes, output = (directory / name for name in ("source.json", "lanes.json", "rollout.json"))
            source.write_text(json.dumps(self.template()))
            with patch.object(observer.subprocess, "run", side_effect=AssertionError("no screen probe expected")):
                result = observer.observe(source, lanes, output)
            self.assertEqual(result["observer"]["reports_state"], "pending")
            self.assertEqual(result["lanes"][0]["observation_state"], "pending")
            self.assertEqual(json.loads(output.read_text()), result)

    def test_safe_atomic_output_preserves_previous_on_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source, output = directory / "source.json", directory / "rollout.json"
            source.write_text("{}")
            output.write_text("previous")
            with self.assertRaises(observer.Invalid):
                observer.atomic_output(source, {}, {observer.safe_path(source)})
            with patch.object(observer.os, "replace", side_effect=OSError("synthetic crash")):
                with self.assertRaises(OSError):
                    observer.atomic_output(output, {}, {observer.safe_path(source)})
            self.assertEqual(output.read_text(), "previous")
            link = directory / "unsafe.json"
            link.symlink_to(source)
            with self.assertRaises(observer.Invalid):
                observer.atomic_output(link, {}, set())

    def test_input_objects_never_mutated(self):
        template, reports = self.template(), [{"id": "worker", "status": "running"}]
        before = copy.deepcopy((template, reports))
        observer.merge(template, reports, {}, {}, now=NOW)
        self.assertEqual((template, reports), before)

    def witness_fixture(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name) / "owned"
        root.mkdir(mode=0o700)
        worktree = root / "lane"
        worktree.mkdir(mode=0o700)
        (worktree / ".git").mkdir(mode=0o700)
        (worktree / "src").mkdir()
        (worktree / "src" / "one.rs").write_text("PRIVATE SOURCE MUST NEVER BE RETURNED")
        return root, worktree

    def test_worktree_witness_is_opt_in_and_never_refreshes_report_or_packet_proof(self):
        root, worktree = self.witness_fixture()
        reports = [{"id": "worker", "worktree": str(worktree), "candidate_paths": ["src/**"],
                    "head_sha": "a" * 40, "status": "verified", "updated_utc": "2026-10-18T10:00:00Z"}]
        with patch.object(observer, "_bounded_git_head", return_value="a" * 40) as head:
            disabled = observer.merge(self.template(), reports, {}, {}, now=NOW)
            head.assert_not_called()
            observed = observer.merge(self.template(), reports, {}, {}, now=NOW, worktree_root=root)
        row = next(lane for lane in observed["lanes"] if lane["id"] == "worker")
        self.assertNotIn("activity", next(lane for lane in disabled["lanes"] if lane["id"] == "worker"))
        self.assertEqual(row["observation_state"], "stale")
        self.assertEqual(row["source_updated_utc"], "2026-10-18T10:00:00Z")
        self.assertEqual(row["activity"]["owned_scope_files"], 1)
        self.assertEqual(row["activity"]["reported_head_matches"], True)
        self.assertIn("owned scope files 1", row["status"])
        self.assertEqual(observed["packets"], self.template()["packets"])
        self.assertEqual(observed["updated_utc"], self.template()["updated_utc"])
        self.assertNotIn("PRIVATE SOURCE", json.dumps(observed))

    def test_scope_metadata_never_follows_directory_or_file_symlinks(self):
        root, worktree = self.witness_fixture()
        outside = root.parent / "outside"
        outside.mkdir()
        (outside / "never-read.rs").write_text("OUTSIDE PRIVATE SOURCE")
        (worktree / "src" / "escape").symlink_to(outside, target_is_directory=True)
        (worktree / "src" / "escape.rs").symlink_to(outside / "never-read.rs")
        reports = [{"id": "worker", "worktree": str(worktree), "candidate_paths": ["src/**", "src/escape/never-read.rs"]}]
        with patch.object(observer, "_bounded_git_head", return_value="b" * 40), patch.object(Path, "read_text", side_effect=AssertionError("no source content reads")):
            result = observer.merge(self.template(), reports, {}, {}, now=NOW, worktree_root=root)
        activity = next(lane for lane in result["lanes"] if lane["id"] == "worker")["activity"]
        self.assertEqual(activity["owned_scope_files"], 1)
        self.assertGreaterEqual(activity["skipped_entries"], 2)
        self.assertNotIn("OUTSIDE PRIVATE SOURCE", json.dumps(result))

    def test_outside_symlink_and_unbounded_scope_reports_fail_closed(self):
        root, worktree = self.witness_fixture()
        (root / "alias").symlink_to(worktree, target_is_directory=True)
        variants = [(str(root.parent), ["src/one.rs"]), (str(root / "alias"), ["src/one.rs"])]
        variants += [(str(worktree), [path]) for path in ("**", "./**", "../outside", "src/*.rs", str(root.parent / "outside"))]
        with patch.object(observer, "_bounded_git_head", side_effect=AssertionError("unsafe report must not invoke Git")):
            for path, scopes in variants:
                with self.subTest(worktree=path, scopes=scopes):
                    result = observer.merge(self.template(), [{"id": "worker", "worktree": path, "candidate_paths": scopes}], {}, {}, now=NOW, worktree_root=root)
                    self.assertEqual(next(lane for lane in result["lanes"] if lane["id"] == "worker")["activity"]["state"], "unsafe")
        root.chmod(0o755)
        with self.assertRaises(observer.Invalid):
            observer.WorktreeWitness(root)
        with self.assertRaises(observer.Invalid):
            observer.WorktreeWitness(Path("relative-root"))

    def test_recursive_metadata_has_entry_and_depth_bounds_and_deduplicates_paths(self):
        root, worktree = self.witness_fixture()
        for number in range(8):
            (worktree / "src" / f"{number}.rs").write_text("fixture")
        reports = [{"id": "worker", "worktree": str(worktree), "candidate_paths": ["src/one.rs", "src/**"]}]
        with patch.object(observer, "_bounded_git_head", return_value="c" * 40):
            complete = observer.merge(self.template(), reports, {}, {}, now=NOW, worktree_root=root)
            with patch.object(observer, "MAX_SCOPE_ENTRIES", 3):
                bounded = observer.merge(self.template(), reports, {}, {}, now=NOW, worktree_root=root)
        full = next(lane for lane in complete["lanes"] if lane["id"] == "worker")["activity"]
        partial = next(lane for lane in bounded["lanes"] if lane["id"] == "worker")["activity"]
        self.assertEqual(full["owned_scope_files"], 9)
        self.assertEqual(partial["state"], "bounded")
        self.assertLessEqual(partial["scanned_entries"], 3)
        nested = worktree / "nested"
        nested.mkdir()
        (nested / "child").mkdir()
        (nested / "child" / "deep").mkdir()
        with patch.object(observer, "_bounded_git_head", return_value="c" * 40), patch.object(observer, "MAX_SCOPE_DEPTH", 1):
            result = observer.merge(self.template(), [{"id": "worker", "worktree": str(worktree), "candidate_path": "nested/**"}], {}, {}, now=NOW, worktree_root=root)
        self.assertEqual(next(lane for lane in result["lanes"] if lane["id"] == "worker")["activity"]["state"], "bounded")

    def test_head_only_suffix_requires_full_matching_observed_reference(self):
        root, worktree = self.witness_fixture()
        report = {"id": "worker", "worktree": str(worktree), "head_sha": "d" * 40}
        with patch.object(observer, "_bounded_git_head", return_value="d" * 40):
            matching = observer.merge(self.template(), [report], {}, {}, now=NOW, worktree_root=root)
        row = next(lane for lane in matching["lanes"] if lane["id"] == "worker")
        self.assertIn("candidate HEAD dddddddddddd", row["status"])
        with patch.object(observer, "_bounded_git_head", return_value="e" * 40):
            mismatch = observer.merge(self.template(), [report], {}, {}, now=NOW, worktree_root=root)
        row = next(lane for lane in mismatch["lanes"] if lane["id"] == "worker")
        self.assertFalse(row["activity"]["reported_head_matches"])
        self.assertNotIn("candidate HEAD", row["status"])

    def test_git_probe_is_fixed_head_only_descriptor_bound_and_output_bounded(self):
        root, _ = self.witness_fixture()
        descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, descriptor)
        for method in ("isdir", "exists"):
            available = patch.object(observer.os.path, method, return_value=True)
            available.start()
            self.addCleanup(available.stop)

        class Process:
            def __init__(self, output):
                reader, writer = os.pipe()
                os.write(writer, output)
                os.close(writer)
                self.stdout, self.returncode, self.killed = os.fdopen(reader, "rb"), None, False

            def wait(self, timeout=None):
                self.returncode = 0 if self.returncode is None else self.returncode
                return self.returncode

            def poll(self):
                return self.returncode

            def kill(self):
                self.killed, self.returncode = True, -9

        process = Process(b"a" * 40 + b"\n")
        with patch.dict(observer.os.environ, {"GIT_DIR": "/foreign", "GIT_WORK_TREE": "/foreign"}), patch.object(observer.subprocess, "Popen", return_value=process) as launch:
            self.assertEqual(observer._bounded_git_head(descriptor, 1), "a" * 40)
        argv, kwargs = launch.call_args.args[0], launch.call_args.kwargs
        self.assertEqual(argv[:5], ["git", "--no-optional-locks", "-c", "core.fsmonitor=false", "-C"])
        self.assertEqual(argv[-3:], ["rev-parse", "--verify", "HEAD"])
        self.assertNotIn("status", argv)
        self.assertEqual(kwargs["pass_fds"], (descriptor,))
        self.assertEqual(kwargs["env"]["GIT_WORK_TREE"], argv[5])
        self.assertNotIn("GIT_DIR", kwargs["env"])
        oversized = Process(b"f" * 257)
        with patch.object(observer.subprocess, "Popen", return_value=oversized), self.assertRaises(observer.Invalid):
            observer._bounded_git_head(descriptor, 1)
        self.assertTrue(oversized.killed)
        timed_out = Process(b"a" * 40)
        with patch.object(observer.subprocess, "Popen", return_value=timed_out), patch.object(observer.select, "select", return_value=([], [], [])), self.assertRaises(observer.Invalid):
            observer._bounded_git_head(descriptor, 1)
        self.assertTrue(timed_out.killed)

    @unittest.skipUnless(shutil.which("git") and Path("/proc/self/fd").is_dir(), "Linux procfs and Git required for descriptor-bound HEAD check")
    def test_real_git_head_probe_uses_only_disposable_worktree_metadata(self):
        root, worktree = self.witness_fixture()
        metadata = worktree / ".git"
        (metadata / "objects").mkdir()
        (metadata / "refs" / "heads").mkdir(parents=True)
        (metadata / "HEAD").write_text("ref: refs/heads/main\n")
        (metadata / "refs" / "heads" / "main").write_text("f" * 40 + "\n")
        # This synthetic ref names no accepted commit or task; the witness only
        # corroborates repository HEAD metadata and never grants acceptance.
        report = {"id": "worker", "worktree": str(worktree), "head_sha": "f" * 40}
        result = observer.merge(self.template(), [report], {}, {}, now=NOW, worktree_root=root)
        activity = next(lane for lane in result["lanes"] if lane["id"] == "worker")["activity"]
        self.assertEqual(activity["head_sha"], "f" * 40)
        self.assertTrue(activity["reported_head_matches"])


if __name__ == "__main__":
    unittest.main()
