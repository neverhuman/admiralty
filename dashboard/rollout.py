"""Bounded, read-only bridge from a rollout manifest to the legacy dashboard.

The manifest is an observation, not an acceptance authority. Receipt references
are displayed as text and are never opened, executed, or independently verified.
"""

import datetime
import hashlib
import json
import os
from pathlib import Path
import stat
from urllib.parse import urlsplit


MAX_BYTES = 1024 * 1024
MAX_PACKETS = 2000
MAX_LANES = 200
STATUSES = ("planned", "ready", "claimed", "in_progress", "blocked", "review", "verified", "integrated")


class ManifestError(ValueError):
    pass


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
        lanes.append({key: _text(lane.get(key), 200) for key in ("id", "owner", "node", "branch", "status")})
    if len({lane["id"] for lane in lanes}) != len(lanes):
        raise ManifestError("Lane ids must be unique.")
    threshold = value.get("stale_after_s", 300)
    if type(threshold) is not int or not 30 <= threshold <= 86400:
        raise ManifestError("stale_after_s must be an integer from 30 to 86400.")
    age = round((now - updated).total_seconds())
    freshness = "future" if age < -30 else "stale" if age > threshold else "fresh"
    pr = value.get("pr") if isinstance(value.get("pr"), dict) else {}
    return {
        "schema_version": 1, "state": "ready", "read_only": True,
        "project": {"id": _text(project["id"], 100), "name": _text(project.get("name")) or _text(project["id"], 100),
                    "repository_url": _url(project.get("repository_url"))},
        "updated_utc": updated.isoformat(timespec="seconds"),
        "producer": _text(value.get("producer"), 200) or "unspecified",
        "source": source,
        "freshness": {"state": freshness, "age_s": age, "stale_after_s": threshold},
        "packets": packets, "lanes": lanes,
        "counts": {status: sum(packet["status"] == status for packet in packets) for status in STATUSES},
        "total": len(packets), "pr": {"url": _url(pr.get("url")), "head": _text(pr.get("head"), 200)},
        "assurance": "Manifest observation only. Receipt references are not independently validated by this collector.",
    }


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
