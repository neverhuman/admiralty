"""Bounded, read-only bridge from a rollout manifest to the legacy dashboard.

The manifest is an observation, not an acceptance authority. Receipt references
are displayed as text and are never opened, executed, or independently verified.
"""

import datetime
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import stat
from urllib.parse import urlsplit


MAX_BYTES = 1024 * 1024
MAX_PACKETS = 2000
MAX_LANES = 200
MAX_SCREEN_PID = 2**31 - 1
STATUSES = ("planned", "ready", "claimed", "in_progress", "blocked", "review", "verified", "integrated")


class ManifestError(ValueError):
    pass


def _screen_identity(value):
    if not isinstance(value, str) or len(value) > 200:
        return None
    match = re.fullmatch(r"(?:([1-9][0-9]{0,9})\.)?([A-Za-z0-9_][A-Za-z0-9_.-]{0,127})", value)
    if not match or match[2] in {".", ".."}:
        return None
    pid = int(match[1]) if match[1] else None
    return (match[2], pid) if pid is None or pid <= MAX_SCREEN_PID else None


def _pid_fields(value):
    result = {}
    for key in ("screen_pid", "reported_screen_pid"):
        if key in value:
            pid = value[key]
            if pid is not None and (type(pid) is not int or not 1 <= pid <= MAX_SCREEN_PID):
                raise ManifestError("Screen PID fields must be null or bounded positive integers.")
            result[key] = pid
    return result


def _text(value, limit=500):
    if not isinstance(value, str):
        return ""
    return "".join(c for c in value if c == "\n" or ord(c) >= 32)[:limit]


def _timestamp(value):
    try:
        parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            return None
        return parsed.astimezone(datetime.timezone.utc)
    except (AttributeError, TypeError, ValueError, OverflowError):
        return None


def _url(value):
    value = _text(value, 2048)
    try:
        parsed = urlsplit(value)
        if parsed.scheme in ("http", "https") and parsed.hostname and not parsed.username and not parsed.password:
            return value
    except ValueError:
        pass
    return ""


def _reference(value, required):
    if not isinstance(value, dict):
        return None
    result = {key: _text(value.get(key), 1000) for key in required}
    if not all(result.values()) or not _timestamp(result.get("observed_utc")):
        return None
    return result


def _activity(value):
    """Carry bounded metadata observations without granting progress authority."""
    if not isinstance(value, dict) or value.get("source") != "owned_worktree_metadata/v1":
        return None
    state = value.get("state")
    observed = _timestamp(value.get("observed_utc"))
    if state not in {"observed", "bounded", "unsafe", "unavailable", "deferred", "not_reported"} or observed is None:
        return None
    result = {"source": "owned_worktree_metadata/v1", "state": state,
              "observed_utc": observed.isoformat(timespec="seconds"),
              "assurance": "Metadata only; no agent liveness, task progress or acceptance established."}
    for key, limit in (("owned_scope_files", 2000), ("scanned_entries", 2000),
                       ("scope_paths", 64), ("skipped_entries", 4096)):
        item = value.get(key)
        if type(item) is int and 0 <= item <= limit:
            result[key] = item
    if type(value.get("bounded")) is bool:
        result["bounded"] = value["bounded"]
    head = value.get("head_sha")
    if isinstance(head, str) and re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", head):
        result["head_sha"] = head
        if type(value.get("reported_head_matches")) is bool:
            result["reported_head_matches"] = value["reported_head_matches"]
    latest = _timestamp(value.get("latest_owned_path_mtime_utc"))
    if latest:
        result["latest_owned_path_mtime_utc"] = latest.isoformat(timespec="seconds")
    if value.get("head_state") == "unavailable":
        result["head_state"] = "unavailable"
    return result


def _packet(value):
    if not isinstance(value, dict) or not _text(value.get("id"), 100):
        raise ManifestError("Each packet requires a nonempty string id.")
    reported = _text(value.get("status"), 40)
    status = "in_progress" if reported == "active" else reported
    warnings = []
    if status not in STATUSES:
        raise ManifestError("Packet status is not supported.")
    verification = _reference(value.get("verification"), ("receipt", "revision", "verifier", "observed_utc"))
    integration = _reference(value.get("integration"), ("receipt", "revision", "target_ref", "observed_utc"))
    if status in ("verified", "integrated") and not verification:
        status = "claimed"
        warnings.append("Verification provenance missing; displayed as claimed.")
    if status == "integrated" and not integration:
        status = "verified"
        warnings.append("Integration provenance missing; displayed as verification recorded.")
    candidate = _text(value.get("candidate_revision"), 200)
    if verification and candidate and verification["revision"] != candidate:
        status = "claimed" if status in ("verified", "integrated") else status
        warnings.append("Verification revision differs from candidate revision.")
    if integration and verification and integration["revision"] != verification["revision"]:
        status = "verified" if status == "integrated" else status
        warnings.append("Integration revision differs from verification revision.")
    claim = value.get("claim")
    if isinstance(claim, dict):
        claim = {key: _text(claim.get(key), 200) for key in ("owner", "node", "observed_utc", "expires_utc")}
        if verification and claim.get("owner") and verification["verifier"] == claim["owner"]:
            status = "claimed" if status in ("verified", "integrated") else status
            warnings.append("Verifier matches claimant; independent acceptance is not established.")
    else:
        claim = None
    dependencies = value.get("depends_on", [])
    if not isinstance(dependencies, list) or len(dependencies) > MAX_PACKETS:
        raise ManifestError("Packet dependencies must be a bounded list.")
    evidence = value.get("evidence", [])
    if not isinstance(evidence, list) or len(evidence) > 100:
        raise ManifestError("Packet evidence must be a bounded list.")
    return {
        "id": _text(value["id"], 100), "title": _text(value.get("title")),
        "status": status, "reported_status": reported, "lane": _text(value.get("lane"), 200),
        "depends_on": [_text(item, 100) for item in dependencies if isinstance(item, str)],
        "claim": claim, "candidate_revision": candidate,
        "verification": verification, "integration": integration, "warnings": warnings,
        "evidence": [{key: _text(item.get(key), 1000) for key in ("kind", "path", "sha256", "revision", "reported_status")}
                     for item in evidence if isinstance(item, dict)],
        "capsule_path": _text(value.get("capsule_path"), 1000),
    }


def _normalize(value, now, source):
    if not isinstance(value, dict) or type(value.get("schema_version")) is not int or value["schema_version"] != 1:
        raise ManifestError("Expected rollout schema_version 1.")
    project = value.get("project")
    if not isinstance(project, dict) or not _text(project.get("id"), 100):
        raise ManifestError("Project requires a nonempty string id.")
    updated = _timestamp(value.get("updated_utc"))
    if updated is None:
        raise ManifestError("updated_utc requires an ISO timestamp with timezone.")
    raw_packets, raw_lanes = value.get("packets"), value.get("lanes", [])
    if not isinstance(raw_packets, list) or len(raw_packets) > MAX_PACKETS:
        raise ManifestError("packets must be a list of at most 2000 entries.")
    if not isinstance(raw_lanes, list) or len(raw_lanes) > MAX_LANES:
        raise ManifestError("lanes must be a list of at most 200 entries.")
    packets = [_packet(packet) for packet in raw_packets]
    if len({packet["id"] for packet in packets}) != len(packets):
        raise ManifestError("Packet ids must be unique.")
    lanes = []
    for lane in raw_lanes:
        if not isinstance(lane, dict) or not _text(lane.get("id"), 200):
            raise ManifestError("Each lane requires a nonempty string id.")
        lanes.append({key: _text(lane.get(key), 200) for key in (
            "id", "owner", "node", "branch", "status", "screen", "model", "effort", "tier", "packet_id",
            "source_updated_utc", "observed_utc", "screen_state",
        )})
        # Preserve pins rather than dropping them and rebinding a recycled name.
        lanes[-1].update(_pid_fields(lane))
        lanes[-1]["screen_identity_valid"] = _screen_identity(lane.get("screen")) is not None
        activity = _activity(lane.get("activity"))
        if activity:
            lanes[-1]["activity"] = activity
    if len({lane["id"] for lane in lanes}) != len(lanes):
        raise ManifestError("Lane ids must be unique.")
    threshold = value.get("stale_after_s", 300)
    if type(threshold) is not int or not 30 <= threshold <= 86400:
        raise ManifestError("stale_after_s must be an integer from 30 to 86400.")
    age = round((now - updated).total_seconds())
    freshness = "future" if age < -30 else "stale" if age > threshold else "fresh"
    pr = value.get("pr") if isinstance(value.get("pr"), dict) else {}
    normalized_project = {"id": _text(project["id"], 100), "name": _text(project.get("name")) or _text(project["id"], 100),
                          "repository_url": _url(project.get("repository_url"))}
    if "captain_screen" in project:
        if _screen_identity(project["captain_screen"]) is None:
            raise ManifestError("Captain screen requires a bounded exact screen identity.")
        normalized_project["captain_screen"] = project["captain_screen"]
    for key in ("captain_screen_pid", "reported_captain_screen_pid"):
        if key in project:
            validated = _pid_fields({"screen_pid": project[key]})
            normalized_project[key] = validated["screen_pid"]
    return {
        "schema_version": 1, "state": "ready", "read_only": True,
        "project": normalized_project,
        "updated_utc": updated.isoformat(timespec="seconds"),
        "producer": _text(value.get("producer"), 200) or "unspecified",
        "source": source,
        "freshness": {"state": freshness, "age_s": age, "stale_after_s": threshold},
        "packets": packets, "lanes": lanes,
        "counts": {status: sum(packet["status"] == status for packet in packets) for status in STATUSES},
        "total": len(packets), "pr": {"url": _url(pr.get("url")), "head": _text(pr.get("head"), 200)},
        "assurance": "Manifest observation only. Receipt references are not independently validated by this collector.",
    }


def observe_rollout_workers(rollout, screens, projects, screen_info, *, do_hc=False,
                            inventory_available=True, local_node=None, sanitize_tail=None):
    """Join bounded manifest identities to a caller-supplied local inventory.

    No I/O occurs except the trusted read-only screen_info callback, invoked at
    most once for each noncolliding actual session. Manifest strings never
    become command arguments; only verified inventory IDs reach that callback.
    Returns additive extras wInfo plus separate observational records, never
    producer projects, packet counts, claims, or acceptance.
    """
    result = {"winfo": {}, "observation": {"source": "screen_process_observation/v1",
              "captain": None, "workers": [], "warnings": [],
              "assurance": "Process and screen observations only; no task progress or acceptance established."}}
    observed = result["observation"]
    if not isinstance(rollout, dict) or rollout.get("state") != "ready":
        return result
    project = rollout.get("project") or {}
    if not isinstance(project, dict) or not isinstance(projects, dict):
        return result
    project_id = project.get("id")
    if not isinstance(project_id, str) or not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,99}", project_id):
        observed["warnings"].append("Rollout project identity cannot be joined to local observations.")
        return result
    observed["project_id"] = project_id
    # A manifest cannot adopt or overwrite a producer-owned project identity.
    if project_id in projects:
        observed["warnings"].append("Rollout project collides with an existing producer project; live join omitted.")
        return result
    lanes = rollout.get("lanes")
    if not isinstance(lanes, list) or len(lanes) > MAX_LANES:
        observed["warnings"].append("Rollout lane observation bound exceeded.")
        return result
    occupied_ids, occupied_names = set(), set()
    for existing in projects.values():
        if not isinstance(existing, dict):
            continue
        workers = existing.get("workers") or []
        for worker in [existing.get("captain") or {}] + (workers if isinstance(workers, list) else []):
            if not isinstance(worker, dict):
                continue
            identity = worker.get("id")
            if isinstance(identity, str):
                occupied_ids.add(identity)
                parsed = _screen_identity(identity)
                if parsed:
                    occupied_names.add(parsed[0])
            if isinstance(worker.get("name"), str):
                occupied_names.add(worker["name"])
    inventory = {}
    for sid, screen in (screens.items() if isinstance(screens, dict) else []):
        parsed = _screen_identity(sid)
        if not parsed or parsed[1] is None or not isinstance(screen, dict):
            continue
        if type(screen.get("pid")) is not int or screen["pid"] != parsed[1] or screen.get("name") != parsed[0]:
            continue
        inventory.setdefault(parsed[0], []).append((sid, screen))
    captain_ref = project.get("captain_screen", "captain_" + project_id)
    captain_name = _screen_identity(captain_ref)
    specs = []
    for lane in lanes:
        if not isinstance(lane, dict):
            continue
        parsed = _screen_identity(lane.get("screen"))
        is_captain = parsed is not None and captain_name is not None and parsed[0] == captain_name[0]
        specs.append(("captain" if is_captain else "worker", lane))
    if captain_name and not any(role == "captain" for role, _ in specs):
        specs.insert(0, ("captain", {"id": "captain", "screen": captain_ref,
                     "screen_pid": project.get("captain_screen_pid"),
                     "reported_screen_pid": project.get("reported_captain_screen_pid")}))
    resolved = []
    for role, lane in specs:
        record = {"lane_id": _text(lane.get("id"), 200), "name": _text(lane.get("screen"), 200),
                  "role": role, "alive": None, "agent_running": None, "identity_state": "unknown",
                  "task": _text(lane.get("packet_id"), 200) or None,
                  "task_source": "rollout manifest assignment (reported)", "tail": []}
        if role == "captain":
            if observed["captain"] is None:
                observed["captain"] = record
            else:
                observed["workers"].append(record)
        else:
            observed["workers"].append(record)
        parsed = _screen_identity(lane.get("screen"))
        if parsed is None or lane.get("screen_identity_valid") is False:
            record["identity_state"] = "invalid" if lane.get("screen") else "not_reported"
            continue
        node = lane.get("node")
        if node and local_node and node != local_node:
            record["identity_state"] = "remote_not_observed"
            continue
        if not inventory_available or not isinstance(screens, dict):
            continue
        pins = [pid for pid in (parsed[1], lane.get("screen_pid"), lane.get("reported_screen_pid")) if pid is not None]
        if role == "captain":
            pins += [pid for pid in (project.get("captain_screen_pid"), project.get("reported_captain_screen_pid")) if pid is not None]
        if any(type(pid) is not int or not 1 <= pid <= MAX_SCREEN_PID for pid in pins) or len(set(pins)) > 1:
            record["identity_state"] = "identity_mismatch"
            continue
        candidates = inventory.get(parsed[0], [])
        if not candidates:
            record.update(identity_state="absent", alive=False, agent_running=False)
            continue
        if pins:
            matching = [(sid, screen) for sid, screen in candidates if screen["pid"] == pins[0]]
            if not matching:
                record["identity_state"] = "identity_mismatch"
                continue
            candidates = matching
        if len(candidates) != 1:
            record["identity_state"] = "ambiguous"
            continue
        sid, screen = candidates[0]
        if sid in occupied_ids or screen["name"] in occupied_names:
            record["identity_state"] = "producer_collision"
            continue
        resolved.append((sid, screen, role, record))
    aliases = Counter(sid for sid, _, _, _ in resolved)
    for sid, screen, role, record in resolved:
        if aliases[sid] != 1:
            record["identity_state"] = "aliased"
            continue
        try:
            info = screen_info(sid, screen, bool(do_hc), project_id)
        except Exception:
            record["identity_state"] = "observation_unavailable"
            continue
        if not isinstance(info, dict) or info.get("id") != sid or info.get("alive") is not True:
            record["identity_state"] = "observation_unavailable"
            continue
        running = info.get("agent_running") if type(info.get("agent_running")) is bool else None
        if info.get("agent") == "supervised":
            running = None
        record.update(id=sid, name=screen["name"], pid=screen["pid"], alive=True,
                      agent_running=running, identity_state="observed", agent=_text(info.get("agent"), 40),
                      model=_text(info.get("model"), 200) or None, effort=_text(info.get("effort"), 40) or None)
        changed = _timestamp(info.get("screen_changed_utc"))
        source = rollout.get("source")
        capture = _timestamp(source.get("loaded_utc")) if isinstance(source, dict) else None
        record["screen_changed_utc"] = changed.isoformat(timespec="seconds") if changed and (capture is None or changed <= capture) else None
        if callable(sanitize_tail) and isinstance(info.get("tail"), list):
            for line in info["tail"][-(8 if role == "captain" else 3):]:
                if isinstance(line, str):
                    record["tail"].append(_text(sanitize_tail(line), 240))
        if running is not None:
            result["winfo"][sid] = (project_id, record)
    return result


def collect_rollout(runtime_dir, now=None, environ=None):
    """Return normalized optional rollout state; never read referenced paths.

    Missing default runtime/rollout.json hides the panel. Explicit configuration
    errors stay visible so a broken path cannot silently resemble healthy state.
    The only opened file is the configured manifest, bounded to one MiB. FIFO,
    device, directory and oversized inputs fail without executing external code.
    """
    env = os.environ if environ is None else environ
    configured = env.get("ADMIRALTY_ROLLOUT_MANIFEST")
    path = Path(configured) if configured else Path(runtime_dir) / "rollout.json"
    now = now or datetime.datetime.now(datetime.timezone.utc)
    if now.tzinfo is None:
        now = now.replace(tzinfo=datetime.timezone.utc)
    source = {"filename": path.name, "loaded_utc": now.isoformat(timespec="seconds")}
    try:
        # Nonblocking open prevents special files from hanging the collector.
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
        with os.fdopen(fd, "rb") as handle:
            info = os.fstat(handle.fileno())
            if not stat.S_ISREG(info.st_mode):
                raise ManifestError("Manifest must be a regular file.")
            if info.st_size > MAX_BYTES:
                raise ManifestError("Manifest exceeds one MiB.")
            data = handle.read(MAX_BYTES + 1)
        if len(data) > MAX_BYTES:
            raise ManifestError("Manifest exceeds one MiB.")
        source.update({"sha256": hashlib.sha256(data).hexdigest(),
                       "mtime_utc": datetime.datetime.fromtimestamp(info.st_mtime, datetime.timezone.utc).isoformat(timespec="seconds")})
        return _normalize(json.loads(data), now, source)
    except FileNotFoundError:
        if not configured:
            return None
        return {"state": "unavailable", "read_only": True, "source": source, "error": "Configured rollout manifest is missing."}
    except (ManifestError, ValueError, UnicodeDecodeError, RecursionError) as error:
        message = str(error) if isinstance(error, ManifestError) else "Manifest is not valid bounded JSON."
        return {"state": "invalid", "read_only": True, "source": source, "error": message}
    except OSError:
        return {"state": "unavailable", "read_only": True, "source": source, "error": "Rollout manifest cannot be read."}
