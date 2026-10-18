#!/usr/bin/env python3
"""Read-only verification of the pinned legacy sources and synthetic regressions.

Run from any directory. Never imports the legacy collectors: importing them can
open runtime databases, create directories, probe hosts or start an HTTP server.
"""

import ast
import csv
import datetime
import hashlib
import io
import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).resolve().parent


def rows_for(path, data=None):
    text = path.read_text() if data is None else data.decode("utf-8")
    if path.suffix == ".csv":
        return list(csv.DictReader(io.StringIO(text, newline="")))
    if path.suffix in (".jsonl", ".ndjson"):
        return [json.loads(line) for line in text.splitlines() if line.strip()]
    return None


def utc(value):
    if value in (None, ""):
        return None
    try:
        if isinstance(value, (int, float)) or re.fullmatch(r"\d+(\.\d+)?", str(value)):
            instant = datetime.datetime.fromtimestamp(float(value), datetime.timezone.utc)
        else:
            instant = datetime.datetime.fromisoformat(str(value).replace("Z", "+00:00"))
        return instant.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    except (ValueError, TypeError, OverflowError):
        return None


def describe(path, data=None):
    data = path.read_bytes() if data is None else data
    rows = rows_for(path, data)
    entry = {
        "path": str(path.relative_to(ROOT)),
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "physical_lines": len(data.splitlines()),
    }
    if rows is not None:
        times = sorted(
            instant
            for row in rows
            for key in ("ts", "ts_utc", "hour_utc", "generated_utc", "start_utc", "end_utc")
            if (instant := utc(row.get(key))) is not None
        )
        entry.update(
            records=len(rows),
            exact_duplicate_records=len(rows)
            - len({json.dumps(row, sort_keys=True, separators=(",", ":")) for row in rows}),
            observed_time_min_utc=times[0] if times else None,
            observed_time_max_utc=times[-1] if times else None,
        )
    return entry


def verify():
    manifest = json.loads((FIXTURES / "source-manifest.json").read_text())
    base = manifest["repository"]["base_sha"]
    # Check the object, not HEAD: implementation commits may legitimately advance HEAD.
    actual = subprocess.check_output(
        ["git", "rev-parse", f"{base}^{{commit}}"], cwd=ROOT, text=True
    ).strip()
    assert actual == base, "Pinned source commit is unavailable"
    working_tree_drift = []
    pinned = {}
    for expected in manifest["sources"]:
        path = ROOT / expected["path"]
        pinned[expected["path"]] = subprocess.check_output(
            ["git", "show", f"{base}:{expected['path']}"], cwd=ROOT
        )
        assert describe(path, pinned[expected["path"]]) == expected, f"Pinned source drift: {expected['path']}"
        if not path.is_file() or describe(path) != expected:
            working_tree_drift.append(expected["path"])
    if "--working-tree" in sys.argv:
        assert not working_tree_drift, f"Working-tree source drift: {working_tree_drift}"

    def pinned_rows(name):
        return rows_for(ROOT / name, pinned[name])

    ledger = pinned_rows("data/history/ledger/model_usage_2026-10-18.jsonl")
    ledger_csv = pinned_rows("data/history/ledger/model_usage.csv")
    common = manifest["relationships"][0]["equality_fields"]
    normalized = lambda row: tuple("" if row.get(k) is None else str(row[k]) for k in common)
    assert sorted(map(normalized, ledger)) == sorted(map(normalized, ledger_csv)), "Ledger mirror drift"
    history = pinned_rows("data/history/timeseries/history_2026-10-18.jsonl")
    history_csv = pinned_rows("data/history/timeseries/history.csv")
    assert [r["ts_utc"] for r in history] == [r["ts_utc"] for r in history_csv], "History mirror drift"

    for path in (ROOT / "tools/export_history.py",):
        # Extract only the literal regex list; do not execute exporter module code.
        tree = ast.parse(pinned[str(path.relative_to(ROOT))].decode("utf-8"))
        assignment = next(
            node for node in tree.body
            if isinstance(node, ast.Assign)
            and any(isinstance(target, ast.Name) and target.id == "SECRET_RES" for target in node.targets)
        )
        patterns = [re.compile(p) for p in ast.literal_eval(assignment.value.generators[0].iter)]
    json_paths = sorted(FIXTURES.rglob("*.json"))
    for path in json_paths:
        text = path.read_text()
        json.loads(text)
        assert not any(pattern.search(text) for pattern in patterns), f"Secret-pattern finding: {path.name}"
        assert not re.search(r"/(?:home|Users)/", text), f"Absolute home path in fixture: {path.name}"
    cases = [json.loads(path.read_text()) for path in sorted((FIXTURES / "cases").glob("*.json"))]
    assert len(cases) == 11
    assert len({case["id"] for case in cases}) == len(cases)
    assert all(case["provenance"]["kind"] == "synthetic_reconstruction" for case in cases)
    print(json.dumps({
        "base_sha": base,
        "verified_sources": len(manifest["sources"]),
        "synthetic_cases": len(cases),
        "secret_scan": "pass",
        "mirror_checks": "pass",
        "working_tree_drift": working_tree_drift,
    }, sort_keys=True))


if __name__ == "__main__":
    verify()
