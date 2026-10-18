"""Offline identity joins; collector/extras functions are AST-loaded without I/O."""
import ast
import copy
import datetime as dt
import hashlib
import importlib.util
from pathlib import Path
import re
import sys
import types
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rollout_live_bridge", ROOT / "dashboard/rollout.py")
rollout = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rollout)
NOW = dt.datetime(2026, 10, 18, 12, tzinfo=dt.timezone.utc)


def functions(path, names, namespace):
    source = ast.parse((ROOT / path).read_text())
    nodes = [node for node in source.body if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name in names]
    assert len(nodes) == len(names), names
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(path), "exec"), namespace)
    return namespace


def manifest(lanes=None, project=None):
    value = {"schema_version": 1, "project": project or {"id": "custom-build", "name": "Custom build"},
             "updated_utc": NOW.isoformat(), "packets": [{"id": "P01", "status": "claimed"}],
             "lanes": lanes if lanes is not None else [{"id": "worker-one", "screen": "custom_worker", "packet_id": "P01"}]}
    return rollout._normalize(value, NOW, {"loaded_utc": NOW.isoformat()})


def screen(pid=101, name="custom_worker"):
    return {f"{pid}.{name}": {"pid": pid, "name": name, "state": "Detached", "started": "observed start"}}


def info(sid, record, do_hc, project):
    return {"id": sid, "name": record["name"], "alive": True, "agent_running": True,
            "agent": "codex", "model": "gpt-6-luna", "effort": "medium",
            "screen_changed_utc": (NOW - dt.timedelta(seconds=5)).isoformat(), "tail": ["safe observed output"]}


class RolloutLiveWorkerTests(unittest.TestCase):
    def observe(self, state=None, inventory=None, **kwargs):
        return rollout.observe_rollout_workers(state or manifest(), screen() if inventory is None else inventory,
            kwargs.pop("projects", {}), kwargs.pop("observer", info), sanitize_tail=lambda value: value.replace("secret-value", "[redacted]"), **kwargs)

    def test_normalization_preserves_pid_pins_and_rejects_bad_pid_values(self):
        for bad in [True, 0, -1, 2**31, "101"]:
            for key in ["screen_pid", "reported_screen_pid"]:
                with self.subTest(bad=bad, key=key), self.assertRaises(rollout.ManifestError):
                    manifest([{"id": "worker", "screen": "custom_worker", key: bad}])
        state = manifest([{"id": "worker", "screen": "101.custom_worker", "screen_pid": 101, "reported_screen_pid": 101}])
        self.assertEqual(state["lanes"][0]["screen_pid"], 101)
        self.assertEqual(state["lanes"][0]["reported_screen_pid"], 101)
        self.assertEqual(len(self.observe(state)["winfo"]), 1)

    def test_actual_observation_assigns_project_without_mutating_counts_claims_or_existing_projects(self):
        state = manifest()
        before = copy.deepcopy(state)
        projects = {name: {"captain": {"id": f"{index}.captain_{name}"}, "counts": {"done": index}}
                    for index, name in enumerate(["alpha", "beta", "gamma", "delta"], 10)}
        original_projects = copy.deepcopy(projects)
        result = self.observe(state, projects=projects, do_hc=True)
        project, worker = result["winfo"]["101.custom_worker"]
        self.assertEqual(project, "custom-build")
        self.assertTrue(worker["alive"])
        self.assertTrue(worker["agent_running"])
        self.assertEqual(worker["task"], "P01")
        self.assertEqual(worker["task_source"], "rollout manifest assignment (reported)")
        self.assertEqual(worker["effort"], "medium")
        self.assertEqual(state, before)
        self.assertEqual(projects, original_projects)
        self.assertNotIn("accepted", worker)

    def test_unknown_absent_and_identity_mismatch_never_capture_or_fake_an_agent(self):
        observer = Mock(side_effect=AssertionError("must not capture mismatched identities"))
        unknown = self.observe(observer=observer, inventory_available=False)
        self.assertIsNone(unknown["observation"]["workers"][0]["agent_running"])
        absent = self.observe(inventory={}, observer=observer)
        self.assertFalse(absent["observation"]["workers"][0]["alive"])
        for lane in [
            {"id": "worker", "screen": "102.custom_worker"},
            {"id": "worker", "screen": "custom_worker", "screen_pid": 102},
            {"id": "worker", "screen": "custom_worker", "screen_pid": 101, "reported_screen_pid": 102},
            {"id": "worker", "screen": "custom_worker", "node": "different-node"},
            {"id": "worker", "screen": "custom_worker; touch /tmp/no"},
        ]:
            result = self.observe(manifest([lane]), observer=observer, local_node="this-node")
            self.assertEqual(result["winfo"], {})
            self.assertIsNone(result["observation"]["workers"][0]["agent_running"])
        observer.assert_not_called()

    def test_aliases_duplicate_inventory_names_and_producer_collisions_are_not_adopted(self):
        observer = Mock(side_effect=AssertionError("ambiguous names are not captured"))
        aliases = manifest([{"id": "a", "screen": "custom_worker"}, {"id": "b", "screen": "101.custom_worker"}])
        result = self.observe(aliases, observer=observer)
        self.assertEqual(result["winfo"], {})
        self.assertEqual([row["identity_state"] for row in result["observation"]["workers"]], ["aliased", "aliased"])
        ambiguous = self.observe(inventory={**screen(), **screen(102)}, observer=observer)
        self.assertEqual(ambiguous["winfo"], {})
        collision = self.observe(projects={"custom-build": {"counts": {"done": 32}}}, observer=observer)
        self.assertEqual(collision["winfo"], {})
        collision = self.observe(projects={"old": {"workers": [{"id": "999.custom_worker", "name": "custom_worker"}]}}, observer=observer)
        self.assertEqual(collision["winfo"], {})
        observer.assert_not_called()
        pinned = self.observe(manifest([{"id": "pinned", "screen": "custom_worker", "screen_pid": 102}]), inventory={**screen(), **screen(102)})
        self.assertEqual(set(pinned["winfo"]), {"102.custom_worker"})

    def test_captain_is_configurable_and_no_process_or_failed_observation_stays_honest(self):
        state = manifest(project={"id": "custom-build", "captain_screen": "201.custom_captain", "captain_screen_pid": 201})
        result = self.observe(state, inventory={**screen(), **screen(201, "custom_captain")})
        self.assertEqual(result["observation"]["captain"]["id"], "201.custom_captain")
        self.assertEqual(len(result["winfo"]), 2)
        no_process = lambda *args: {**info(*args), "agent_running": False, "agent": None}
        stopped = self.observe(observer=no_process)
        self.assertFalse(stopped["winfo"]["101.custom_worker"][1]["agent_running"])
        supervised = self.observe(observer=lambda *args: {**info(*args), "agent": "supervised"})
        self.assertEqual(supervised["winfo"], {})
        failed = self.observe(observer=Mock(side_effect=OSError("private path must not be exposed")))
        self.assertEqual(failed["winfo"], {})
        self.assertEqual(failed["observation"]["workers"][0]["identity_state"], "observation_unavailable")

    def test_tail_is_cleaned_bounded_and_never_copied_without_a_sanitizer(self):
        observer = lambda *args: {**info(*args), "tail": ["secret-value " + "x" * 500] * 20}
        result = self.observe(observer=observer)
        tail = result["winfo"]["101.custom_worker"][1]["tail"]
        self.assertEqual(len(tail), 3)
        self.assertTrue(all(len(line) <= 240 and "secret-value" not in line for line in tail))
        without = rollout.observe_rollout_workers(manifest(), screen(), {}, observer)
        self.assertEqual(without["winfo"]["101.custom_worker"][1]["tail"], [])

    def test_screen_first_frame_is_not_activity_and_process_probe_is_shared_per_cycle(self):
        captures = iter(["baseline", "baseline", "changed"])
        state = {"hc_hash": {}, "hc_changed": {}, "worker_info": {}, "agent_processes": {}}
        probe = Mock(return_value={"kind": "codex", "model": "gpt-6-luna", "effort": "medium"})
        namespace = functions("dashboard/collector.py", ["agent_proc", "screen_info"], {
            "STATE": state, "_agent_proc_uncached": probe, "hashlib": hashlib,
            "hardcopy": lambda sid: next(captures), "now_iso": lambda: NOW.isoformat(),
            "MODEL_RE": re.compile(r"never-match"), "TASK_RE": {}, "meaningful": lambda value, *args: [value],
        })
        scr = screen()["101.custom_worker"]
        read = namespace["screen_info"]
        self.assertIsNone(read("101.custom_worker", scr, True, "custom-build", require_change=True)["screen_changed_utc"])
        self.assertIsNone(read("101.custom_worker", scr, True, "custom-build", require_change=True)["screen_changed_utc"])
        self.assertEqual(read("101.custom_worker", scr, True, "custom-build", require_change=True)["screen_changed_utc"], NOW.isoformat())
        self.assertEqual(probe.call_count, 1)
        state["agent_processes"] = {}
        namespace["agent_proc"](101)
        self.assertEqual(probe.call_count, 2)

    def test_extras_merge_keeps_producer_records_and_actual_worker_state(self):
        namespace = functions("dashboard/extras.py", ["merge_extra_winfo", "worker_state"], {
            "re": re, "time": types.SimpleNamespace(time=lambda: NOW.timestamp()),
            "parse_ts": lambda value: dt.datetime.fromisoformat(value).timestamp() if value else None,
        })
        winfo = {"101.custom_worker": ("original-project", {"id": "101.custom_worker"})}
        joined = self.observe()["winfo"]
        namespace["merge_extra_winfo"](winfo, screen(), joined)
        self.assertEqual(winfo["101.custom_worker"][0], "original-project")
        winfo = {}
        namespace["merge_extra_winfo"](winfo, screen(), joined)
        worker = winfo["101.custom_worker"][1]
        self.assertEqual(namespace["worker_state"](worker), "active")
        worker["screen_changed_utc"] = None
        self.assertEqual(namespace["worker_state"](worker), "idle")
        worker["agent_running"] = False
        self.assertEqual(namespace["worker_state"](worker), "finished")

    def test_cycle_loads_manifest_and_joins_before_extras_without_adding_projects(self):
        order, output = [], {}
        source = manifest()
        legacy = {key: f"{index}.captain_{key}" for index, key in enumerate(["alpha", "beta", "gamma", "delta"], 10)}
        state = {"cycle": 0, "worker_info": {}, "screen_inventory_available": True}
        def extra_collect(out, screens, helpers):
            order.append("extras")
            self.assertEqual(set(out["projects"]), set(legacy))
            self.assertEqual(helpers["extra_winfo"]["101.custom_worker"][0], "custom-build")
            return {}
        namespace = functions("dashboard/collector.py", ["cycle"], {
            "STATE": state, "WORKER_HC_EVERY": 3, "screen_list": screen, "now_iso": lambda: NOW.isoformat(),
            "INTERVAL": 45, "CAPTAINS": legacy, "COLLECT": {key: lambda *_: {"counts": {"done": 32}} for key in legacy},
            "NAMES": legacy, "overlay_active": lambda *_: None, "screen_info": lambda sid, scr, *args, **kwargs:
                info(sid, scr, True, "custom-build") if scr else {"id": sid, "alive": False},
            "project_of": lambda _: None, "flags_for": lambda *_: [], "BASE": "/unused/runtime",
            "collect_rollout": lambda _: (order.append("manifest") or source), "observe_rollout_workers": rollout.observe_rollout_workers,
            "clean": lambda value: value, "agent_proc": lambda _: None, "descendants": lambda _: [],
            "HIST": "history", "HISTORY": {}, "OUT": "status", "wj": lambda path, value: output.update({path: value}),
        })
        with patch.dict(sys.modules, {"extras": types.SimpleNamespace(collect=extra_collect)}):
            namespace["cycle"]()
        self.assertEqual(order, ["manifest", "extras"])
        self.assertEqual(output["status"]["rollout"]["counts"], source["counts"])
        self.assertTrue(all(project["counts"]["done"] == 32 for project in output["status"]["projects"].values()))


if __name__ == "__main__":
    unittest.main()
