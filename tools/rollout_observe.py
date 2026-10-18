#!/usr/bin/env python3
"""Temporary read-only lane observer. Never dispatches, types input or accepts work."""
import argparse
import copy
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import sys
import tempfile
import time

MAX_BYTES = 1024 * 1024
MAX_LANES = 200
MAX_PACKETS = 2000
ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\Z")
HEAD = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
MAX_SCOPE_PATHS = 64
MAX_SCOPE_ENTRIES = 2000
MAX_SCOPE_DEPTH = 16
MAX_WORKTREE_PROBES = 32
WORKTREE_BUDGET_SECONDS = 5


class Invalid(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise Invalid(message)


def timestamp(value):
    try:
        result = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        return result.astimezone(dt.timezone.utc) if result.tzinfo else None
    except (AttributeError, ValueError, TypeError, OverflowError):
        return None


def iso(value):
    return value.isoformat(timespec="seconds")


def safe_path(raw):
    raw = Path(raw).expanduser()
    require(".." not in raw.parts and not any(ord(c) < 32 for c in str(raw)), "unsafe path")
    result = raw.absolute()
    aliases = {Path("/tmp"): Path("/private/tmp"), Path("/var"): Path("/private/var")}
    for part in [result, *result.parents]:
        if part.is_symlink():
            require(part in aliases and part.resolve() == aliases[part] and part.lstat().st_uid == 0, "symlink paths are forbidden")
    return result.resolve()


def read_owned(path):
    path = safe_path(path)
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid(), "input must be a regular owned JSON file")
        require(info.st_size <= MAX_BYTES, "input exceeds one MiB")
        data = stream.read(MAX_BYTES + 1)
    require(len(data) <= MAX_BYTES, "input exceeds one MiB")
    def pairs(items):
        value = {}
        for key, child in items:
            require(key not in value, "duplicate JSON key")
            value[key] = child
        return value
    def constant(_):
        raise Invalid("non-finite JSON is forbidden")
    try:
        value = json.loads(data, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise Invalid("input is not bounded valid JSON") from error
    return value, {"filename": path.name, "sha256": hashlib.sha256(data).hexdigest(),
                   "mtime_utc": iso(dt.datetime.fromtimestamp(info.st_mtime, dt.timezone.utc))}


def screen_snapshot(prefix):
    """Only independent probe: fixed argv screen -ls. Never runs JSON-supplied text."""
    require(isinstance(prefix, str) and ID.fullmatch(prefix), "unsafe session prefix")
    try:
        result = subprocess.run(["screen", "-ls"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                timeout=5, check=False)
        if len(result.stdout) > 65536:
            return {"available": False, "sessions": {}, "reason": "screen output exceeds bound"}
        output = result.stdout.decode("utf-8", "replace")
        sessions = {}
        for pid, name in re.findall(r"^\s*(\d+)\.([A-Za-z0-9_.-]+)\s", output, re.M):
            if name.startswith(prefix):
                sessions[name] = int(pid)
        known = result.returncode == 0 or "No Sockets found" in output or "No sockets found" in output
        return {"available": known, "sessions": sessions, "reason": "screen -ls" if known else "screen probe unavailable"}
    except (OSError, subprocess.SubprocessError):
        return {"available": False, "sessions": {}, "reason": "screen probe unavailable"}


def text(value, limit=160):
    return "".join(c for c in value if ord(c) >= 32)[:limit] if isinstance(value, str) else ""


def lane_id(value):
    require(isinstance(value, dict), "lane must be an object")
    identity = value.get("id", value.get("lane", value.get("screen")))
    require(isinstance(identity, str) and ID.fullmatch(identity), "lane requires a safe stable id")
    return identity


def lane_rows(value):
    if value is None:
        return [], None
    rows = value.get("lanes") if isinstance(value, dict) else value
    require(isinstance(rows, list) and len(rows) <= MAX_LANES, "lane report must contain at most 200 lanes")
    updated = value.get("updated_utc", value.get("observed_utc")) if isinstance(value, dict) else None
    for row in rows:
        lane_id(row)
    return rows, updated


def _owned_directory(parent, parts):
    descriptor = os.dup(parent)
    try:
        for part in parts:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
            require(os.fstat(descriptor).st_uid == os.getuid(), "directory is not owned")
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _bounded_git_head(descriptor, timeout):
    """Only Git probe: rev-parse HEAD. No status, filters, hooks or file output."""
    # Linux procfs preserves directory identity across exec. macOS /dev/fd
    # cannot be chdir'd into, so report HEAD unavailable there rather than use
    # a pathname race or an unsafe pre-exec callback.
    target = f"/proc/self/fd/{descriptor}"
    require(os.path.isdir("/proc/self/fd") and os.path.exists(target), "descriptor-bound Git unavailable on this platform")
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_SYSTEM": os.devnull,
                        "GIT_CONFIG_GLOBAL": os.devnull, "GIT_TERMINAL_PROMPT": "0",
                        "GIT_OPTIONAL_LOCKS": "0", "GIT_WORK_TREE": target, "GIT_PAGER": "cat"})
    argv = ["git", "--no-optional-locks", "-c", "core.fsmonitor=false", "-C", target, "rev-parse", "--verify", "HEAD"]
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, env=environment, pass_fds=(descriptor,))
    try:
        deadline, output = time.monotonic() + timeout, bytearray()
        os.set_blocking(process.stdout.fileno(), False)
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, "Git HEAD probe timed out")
            if not select.select([process.stdout], [], [], remaining)[0]:
                raise Invalid("Git HEAD probe timed out")
            chunk = os.read(process.stdout.fileno(), 257 - len(output))
            if not chunk:
                break
            output.extend(chunk)
            require(len(output) <= 256, "Git HEAD output exceeds bound")
        require(process.wait(timeout=max(0.01, deadline - time.monotonic())) == 0, "Git HEAD unavailable")
        value = bytes(output).decode("ascii").strip()
        require(HEAD.fullmatch(value), "Git HEAD is not a complete object identity")
        return value
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=1)
        process.stdout.close()


class WorktreeWitness:
    """Opt-in metadata witness confined to an existing owned private prefix."""
    def __init__(self, raw):
        self.configured = Path(raw)
        require(self.configured.is_absolute(), "worktree root must be absolute")
        self.root = safe_path(raw)
        self.descriptor = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        info = os.fstat(self.descriptor)
        if info.st_uid != os.getuid() or info.st_mode & 0o077:
            os.close(self.descriptor)
            raise Invalid("worktree root must be owned and private (0700)")
        self.deadline, self.probes = time.monotonic() + WORKTREE_BUDGET_SECONDS, 0

    def close(self):
        os.close(self.descriptor)

    def observe(self, row, now):
        result = {"source": "owned_worktree_metadata/v1", "state": "unavailable", "observed_utc": iso(now),
                  "assurance": "Metadata only; no agent liveness, task progress or acceptance established."}
        if self.probes >= MAX_WORKTREE_PROBES or time.monotonic() >= self.deadline:
            result["state"] = "deferred"
            return result
        raw = row.get("worktree")
        if not isinstance(raw, str) or not raw:
            result["state"] = "not_reported"
            return result
        descriptor = None
        try:
            candidate = Path(raw)
            require(len(raw) <= 4096 and candidate.is_absolute() and ".." not in candidate.parts
                    and not any(ord(c) < 32 for c in raw), "unsafe worktree")
            relative = None
            for root in (self.configured, self.root):
                try:
                    relative = candidate.relative_to(root)
                    break
                except ValueError:
                    continue
            require(relative is not None, "worktree outside configured prefix")
            descriptor = _owned_directory(self.descriptor, relative.parts)
            self.probes += 1
            paths = row.get("candidate_paths", [row["candidate_path"]] if "candidate_path" in row else [])
            require(isinstance(paths, list) and len(paths) <= MAX_SCOPE_PATHS
                    and all(isinstance(path, str) for path in paths), "bounded explicit candidate paths required")
            scopes = []
            for raw_path in paths:
                recursive = raw_path.endswith("/**")
                value = raw_path[:-3] if recursive else raw_path
                require(value and len(value) <= 4096 and not any(c in value for c in "*?[]\\")
                        and not any(ord(c) < 32 for c in value), "unsafe candidate path")
                path = Path(value)
                if path.is_absolute():
                    path = path.relative_to(candidate)
                require(path.parts and len(path.parts) <= MAX_SCOPE_DEPTH
                        and all(part not in (".", "..") for part in path.parts), "candidate path escapes worktree")
                scopes.append((path.parts, recursive))
            scopes.sort(key=lambda scope: (not scope[1], len(scope[0]), scope[0]))
            files, visited, latest, skipped = set(), set(), None, 0
            bounded = False

            def scan(parent, name, relative_parts, depth, recursive):
                nonlocal latest, skipped, bounded
                if relative_parts in visited:
                    return
                if len(visited) >= MAX_SCOPE_ENTRIES or time.monotonic() >= self.deadline:
                    bounded = True
                    return
                visited.add(relative_parts)
                try:
                    info = os.stat(name, dir_fd=parent, follow_symlinks=False)
                    if info.st_uid != os.getuid() or stat.S_ISLNK(info.st_mode):
                        skipped += 1
                    elif stat.S_ISREG(info.st_mode):
                        files.add(relative_parts)
                        latest = info.st_mtime if latest is None else max(latest, info.st_mtime)
                    elif stat.S_ISDIR(info.st_mode) and recursive:
                        if depth >= MAX_SCOPE_DEPTH:
                            bounded = True
                            return
                        child = _owned_directory(parent, (name,))
                        try:
                            with os.scandir(child) as entries:
                                for entry in entries:
                                    if bounded:
                                        break
                                    scan(child, entry.name, (*relative_parts, entry.name), depth + 1, True)
                        finally:
                            os.close(child)
                    else:
                        skipped += 1
                except (OSError, Invalid):
                    skipped += 1

            for parts, recursive in scopes:
                if bounded:
                    break
                try:
                    parent = _owned_directory(descriptor, parts[:-1])
                    try:
                        scan(parent, parts[-1], parts, 0, recursive)
                    finally:
                        os.close(parent)
                except (OSError, Invalid):
                    skipped += 1
            result.update({"state": "bounded" if bounded else "observed", "owned_scope_files": len(files),
                           "scanned_entries": len(visited), "scope_paths": len(scopes), "skipped_entries": skipped,
                           "bounded": bounded, "latest_owned_path_mtime_utc":
                           iso(dt.datetime.fromtimestamp(latest, dt.timezone.utc)) if latest is not None else None})
            # Presence of a local .git marker prevents repository discovery from
            # silently using an unrelated parent checkout. Never follow a link.
            try:
                marker = os.stat(".git", dir_fd=descriptor, follow_symlinks=False)
                require(marker.st_uid == os.getuid() and (stat.S_ISREG(marker.st_mode) or stat.S_ISDIR(marker.st_mode)), "unsafe git marker")
                remaining = self.deadline - time.monotonic()
                require(remaining > 0, "worktree observation budget exhausted")
                head = _bounded_git_head(descriptor, min(1.0, remaining))
                reported = row.get("head_sha")
                result.update({"head_sha": head, "reported_head_matches":
                               head == reported if isinstance(reported, str) and HEAD.fullmatch(reported) else None})
            except (OSError, Invalid, UnicodeError, subprocess.SubprocessError):
                result["head_state"] = "unavailable"
            return result
        except (OSError, Invalid, ValueError, TypeError):
            result["state"] = "unsafe"
            return result
        finally:
            if descriptor is not None:
                os.close(descriptor)


def merge(template, reports, source_meta, lane_meta, *, now, prefix=None, screens=None, stale_after=120, worktree_root=None):
    witness = WorktreeWitness(worktree_root) if worktree_root is not None else None
    try:
        return _merge(template, reports, source_meta, lane_meta, now=now, prefix=prefix,
                      screens=screens, stale_after=stale_after, witness=witness)
    finally:
        if witness is not None:
            witness.close()


def _merge(template, reports, source_meta, lane_meta, *, now, prefix=None, screens=None, stale_after=120, witness=None):
    require(isinstance(template, dict) and template.get("schema_version") == 1, "template schema_version 1 required")
    require(isinstance(template.get("project"), dict) and template["project"].get("id"), "template project required")
    require(timestamp(template.get("updated_utc")) is not None, "template updated_utc requires timezone")
    packets = template.get("packets")
    require(isinstance(packets, list) and len(packets) <= MAX_PACKETS, "bounded template packets required")
    packet_ids = []
    for packet in packets:
        require(isinstance(packet, dict) and isinstance(packet.get("id"), str) and ID.fullmatch(packet["id"]), "unsafe packet id")
        packet_ids.append(packet["id"])
    require(len(set(packet_ids)) == len(packet_ids), "duplicate packet id")
    original = template.get("lanes", [])
    require(isinstance(original, list) and len(original) <= MAX_LANES, "bounded template lanes required")
    rows, report_time = lane_rows(reports)
    report_time = report_time or (lane_meta or {}).get("mtime_utc")
    source_time = template["updated_utc"]
    candidates = {}
    duplicate_ids = set()
    # Template supplies pending lanes; reports override only observational fields.
    for row in original:
        identity = lane_id(row)
        require(identity not in candidates, "duplicate template lane id")
        candidates[identity] = (False, row)
    selected = {}
    for row in rows:
        identity = lane_id(row)
        old = selected.get(identity)
        if old is not None and old != row:
            duplicate_ids.add(identity)
        # Prefer newest explicit source time, then deterministic bytes for ties.
        key = lambda item: (iso(timestamp(item.get("updated_utc", item.get("observed_utc"))) or dt.datetime.min.replace(tzinfo=dt.timezone.utc)), json.dumps(item, sort_keys=True))
        if old is None or key(row) > key(old):
            selected[identity] = row
    candidates.update({identity: (True, row) for identity, row in selected.items()})
    require(len(candidates) <= MAX_LANES, "combined lanes exceed bound")
    output = copy.deepcopy(template)
    output["lanes"] = []
    for identity, (reported, row) in sorted(candidates.items()):
        raw_time = row.get("updated_utc", row.get("observed_utc", report_time)) if reported else row.get("updated_utc", source_time)
        source_updated = timestamp(raw_time)
        age = (now - source_updated).total_seconds() if source_updated else None
        freshness = "unknown" if age is None else "future" if age < -30 else "stale" if age > stale_after else "fresh"
        state = "pending" if not reported else freshness
        session = row.get("screen", row.get("session", "" if row.get("agent_session") else identity))
        session = session if isinstance(session, str) else ""
        match = re.fullmatch(r"(?:(\d+)\.)?([A-Za-z0-9_.-]+)", session)
        name = match[2] if match else ""
        reported_pid = row.get("screen_pid")
        if reported_pid is None and match and match[1]:
            reported_pid = int(match[1])
        if reported_pid is not None:
            require(type(reported_pid) is int and 0 < reported_pid <= 2**31 - 1, "invalid screen PID")
        screen_state, actual_pid = "not_checked", None
        if not session:
            screen_state = "not_exposed"
        elif prefix and screens is not None:
            if not name.startswith(prefix):
                screen_state = "out_of_scope"
            elif not screens.get("available"):
                screen_state = "unknown"
            else:
                actual_pid = screens.get("sessions", {}).get(name)
                if actual_pid is None:
                    screen_state = "absent"
                elif reported_pid is not None and reported_pid != actual_pid:
                    screen_state = "identity_mismatch"
                else:
                    screen_state = "present"
                if reported and screen_state in {"absent", "identity_mismatch"}:
                    state = "orphaned"
        if identity in duplicate_ids:
            state = "pending"
        reported_status = text(row.get("status", row.get("state"))) or "unknown"
        status = f"{state}; reported {reported_status}; screen {screen_state}"
        model, effort = text(row.get("model"), 80), text(row.get("effort"), 30)
        if model:
            status += f"; {model} {effort}".rstrip()
        reserved = row.get("reserved_cpus", [])
        require(isinstance(reserved, list) and len(reserved) <= 128
                and all(type(cpu) is int and 0 <= cpu < 1048576 for cpu in reserved), "invalid reserved CPU list")
        pinned = row.get("cpus_pinned")
        if reserved:
            status += "; CPU reserved " + ",".join(map(str, reserved[:16]))
            status += " pinned" if pinned is True else " NOT pinned" if pinned is False else " pinning unknown"
        if freshness == "stale" and state != "stale":
            status += "; source stale"
        if identity in duplicate_ids:
            status += "; conflicting duplicate reports"
        activity = witness.observe(row, now) if witness is not None and reported else None
        if activity:
            suffix = ""
            if activity.get("scope_paths") and "owned_scope_files" in activity:
                suffix = f"; owned scope files {activity['owned_scope_files']}"
                if activity.get("latest_owned_path_mtime_utc"):
                    suffix += "; latest " + activity["latest_owned_path_mtime_utc"]
                if activity.get("bounded"):
                    suffix += " (bounded sample)"
            elif activity.get("reported_head_matches") is True:
                suffix = "; candidate HEAD " + activity["head_sha"][:12]
            if suffix:
                status = status[:max(0, 200 - len(suffix))] + suffix
        output["lanes"].append({
            "id": identity, "owner": text(row.get("owner", row.get("agent_session"))), "node": text(row.get("node", row.get("host"))),
            "branch": text(row.get("branch")), "status": status[:200], "reported_status": reported_status,
            "packet_id": text(row.get("packet_id", row.get("packet", row.get("task")))), "observation_state": state,
            "freshness": freshness, "source_updated_utc": raw_time, "observed_utc": iso(now),
            "screen": text(session), "screen_state": screen_state, "screen_pid": actual_pid,
            "reported_screen_pid": reported_pid, "self_reported": reported,
            "agent_session": text(row.get("agent_session")), "model": model, "effort": effort,
            "tier": text(row.get("tier")), "head_sha": text(row.get("head_sha")),
            "worktree": text(row.get("worktree"), 500), "reserved_cpus": reserved, "cpus_pinned": pinned if type(pinned) is bool else None,
        })
        if activity:
            output["lanes"][-1]["activity"] = activity
    output["observer"] = {
        "producer": "admiralty.legacy_rollout_observer/v1", "observed_utc": iso(now),
        "source_updated_utc": source_time, "lanes_source_updated_utc": report_time,
        "source": source_meta, "lanes_source": lane_meta, "reports_state": "pending" if reports is None else "available",
        "session_prefix": prefix, "screen_probe": "disabled" if screens is None else "available" if screens.get("available") else "unavailable",
        "stale_after_s": stale_after,
        "launch_method": text(reports.get("launch_method"), 500) if isinstance(reports, dict) else "",
        "assurance": "Read-only observation. Lane self-reports and process liveness do not establish work progress, verification or integration.",
    }
    # Deliberately preserve producer, updated_utc and every packet/proof byte-value.
    return output


def atomic_output(raw, value, inputs):
    output = safe_path(raw)
    require(output.suffix == ".json" and output.name not in {"package.json", "tsconfig.json"}, "output must be a dedicated runtime JSON filename")
    require(output.parent.is_dir() and output.parent.stat().st_uid == os.getuid(), "output parent must be an existing owned directory")
    require(output not in inputs, "output cannot replace an input")
    if output.exists():
        info = output.stat()
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid(), "output must be a regular owned file")
        require(not any(path.exists() and os.path.samefile(output, path) for path in inputs), "output aliases an input")
    data = (json.dumps(value, sort_keys=True, indent=2, allow_nan=False) + "\n").encode()
    require(len(data) <= MAX_BYTES, "combined output exceeds dashboard one MiB bound")
    fd, name = tempfile.mkstemp(prefix=".rollout-observe-", dir=output.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, output)
        descriptor = os.open(output.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def observe(source, lanes, output, *, prefix=None, screens=None, worktree_root=None):
    template, source_meta = read_owned(source)
    try:
        reports, lane_meta = read_owned(lanes)
    except FileNotFoundError:
        reports, lane_meta = None, {"filename": Path(lanes).name, "state": "missing"}
    if prefix and screens is None:
        screens = screen_snapshot(prefix)
    result = merge(template, reports, source_meta, lane_meta, now=dt.datetime.now(dt.timezone.utc), prefix=prefix, screens=screens,
                   worktree_root=worktree_root)
    atomic_output(output, result, {safe_path(source), safe_path(lanes)})
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--lanes", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--interval", type=int, default=15)
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--session-prefix")
    parser.add_argument("--worktree-root", type=Path, help="Opt-in existing owned 0700 prefix for reported worktree HEAD and explicit candidate-path metadata only")
    args = parser.parse_args(argv)
    if not 1 <= args.interval <= 3600 or (args.session_prefix is not None and not ID.fullmatch(args.session_prefix)):
        parser.error("interval must be 1..3600 seconds and session prefix must be a safe identifier")
    while True:
        try:
            result = observe(args.source, args.lanes, args.output, prefix=args.session_prefix, worktree_root=args.worktree_root)
            if args.once:
                print(json.dumps({"lanes": len(result["lanes"]), "observed_utc": result["observer"]["observed_utc"], "source_updated_utc": result["updated_utc"]}))
                return 0
        except (Invalid, OSError, ValueError, TypeError, KeyError) as error:
            print(f"rollout_observe: {error}", file=sys.stderr)
            if args.once:
                return 2
        try:
            time.sleep(args.interval)
        except KeyboardInterrupt:
            return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(0)
