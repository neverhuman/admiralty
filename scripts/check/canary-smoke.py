#!/usr/bin/env python3
"""Bounded offline HTTP smoke checks for explicitly supplied Admiralty binaries."""

from __future__ import annotations

import argparse
import hashlib
import http.client
import ipaddress
import json
import os
import secrets
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit


MAX_JSON_BYTES = 1024 * 1024
MAX_STRING_BYTES = 4096
MAX_ITEMS = 1024
MAX_OUTPUT_BYTES = 64 * 1024
MAX_HTTP_BYTES = 1024 * 1024
MAX_STARTUP_SECONDS = 15.0
MAX_REQUEST_SECONDS = 4.0
MAX_TOTAL_SECONDS = 45.0
TERM_WAIT_SECONDS = 1.0
FINAL_WAIT_SECONDS = 1.0
PROTECTED_PORTS = frozenset({18890, 18891})
LISTEN_PREFIX = b"Admiralty listening on http://"


class SmokeError(Exception):
    """A bounded check failed without implying qualification."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SmokeError(message)


def require_status(actual: int, expected: int, label: str) -> None:
    require(actual == expected, f"{label} returned HTTP {actual}, expected {expected}")


def validate_absolute_owned_file(value: str, *, executable: bool = False) -> Path:
    require(len(value.encode("utf-8")) <= MAX_STRING_BYTES, "binary path exceeds string bound")
    path = Path(value)
    require(path.is_absolute(), "binary path must be absolute")
    require(".." not in path.parts, "binary path cannot contain parent traversal")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        try:
            mode = current.lstat().st_mode
        except OSError as error:
            raise SmokeError("binary path component is unavailable") from error
        require(not stat.S_ISLNK(mode), "binary path contains a symlink")
    metadata = path.lstat()
    require(stat.S_ISREG(metadata.st_mode), "binary must be a regular file")
    require(metadata.st_uid == os.getuid(), "binary must be owned by the invoking user")
    if executable:
        require(os.access(path, os.X_OK), "binary is not executable")
    return path


def validate_scratch_root(value: str) -> Path:
    require(len(value.encode("utf-8")) <= MAX_STRING_BYTES, "scratch path exceeds string bound")
    path = Path(value)
    require(path.is_absolute(), "scratch root must be absolute")
    require(".." not in path.parts, "scratch root cannot contain parent traversal")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        try:
            mode = current.lstat().st_mode
        except OSError as error:
            raise SmokeError("scratch root component is unavailable") from error
        require(not stat.S_ISLNK(mode), "scratch root contains a symlink")
    metadata = path.lstat()
    require(stat.S_ISDIR(metadata.st_mode), "scratch root must be a directory")
    require(metadata.st_uid == os.getuid(), "scratch root must be owned by the invoking user")
    require(stat.S_IMODE(metadata.st_mode) & 0o077 == 0, "scratch root must not be group/world accessible")
    return path


def validate_listener_line(line: bytes) -> tuple[str, int]:
    line = line.rstrip(b"\r\n")
    require(line.startswith(LISTEN_PREFIX), "missing supported child listener announcement")
    rest = line[len(LISTEN_PREFIX) :]
    url_bytes, separator, suffix = rest.partition(b" ")
    require(separator and suffix == b"(read-only; native execution disabled)",
            "unsupported or future listener announcement")
    require(len(url_bytes) <= MAX_STRING_BYTES, "listener announcement is oversized")
    try:
        url = "http://" + url_bytes.decode("ascii")
    except UnicodeDecodeError as error:
        raise SmokeError("listener URL is not ASCII") from error
    parts = urlsplit(url)
    require(parts.scheme == "http" and parts.path in ("", "/") and not parts.query and not parts.fragment,
            "listener must be a bare HTTP URL")
    require(parts.username is None and parts.password is None, "listener URL cannot contain user information")
    require(parts.hostname is not None, "listener URL has no host")
    try:
        address = ipaddress.ip_address(parts.hostname)
        port = parts.port
    except ValueError as error:
        raise SmokeError("listener must use a literal loopback address and valid port") from error
    require(address.is_loopback, "child listener is not loopback")
    require(port is not None and 1 <= port <= 65535, "child listener port is invalid")
    require(port not in PROTECTED_PORTS, "protected legacy port is forbidden")
    return address.compressed, port


@dataclass
class BoundedCapture:
    limit: int = MAX_OUTPUT_BYTES

    def __post_init__(self) -> None:
        self._lock = threading.Lock()
        self._data = bytearray()
        self.total_bytes = 0
        self.oversized = False
        self.eof = False

    def append(self, data: bytes) -> None:
        with self._lock:
            self.total_bytes += len(data)
            remaining = self.limit - len(self._data)
            if remaining > 0:
                self._data.extend(data[:remaining])
            if self.total_bytes > self.limit:
                self.oversized = True

    def snapshot(self) -> bytes:
        with self._lock:
            return bytes(self._data)


def drain_stderr(child: subprocess.Popen[bytes], capture: BoundedCapture) -> threading.Thread:
    require(child.stderr is not None, "child stderr pipe was not created")

    def reader() -> None:
        try:
            while True:
                chunk = os.read(child.stderr.fileno(), 4096)
                if not chunk:
                    break
                capture.append(chunk)
        except OSError:
            pass
        finally:
            capture.eof = True

    thread = threading.Thread(target=reader, name="canary-stderr-reader", daemon=True)
    thread.start()
    return thread


def drain_pipe(pipe: Any, capture: BoundedCapture, name: str) -> threading.Thread:
    def reader() -> None:
        try:
            while True:
                chunk = os.read(pipe.fileno(), 4096)
                if not chunk:
                    break
                capture.append(chunk)
        except OSError:
            pass
        finally:
            capture.eof = True

    thread = threading.Thread(target=reader, name=name, daemon=True)
    thread.start()
    return thread


def redact(data: bytes, secret: str) -> str:
    text = data.decode("utf-8", errors="replace")
    if secret:
        text = text.replace(secret, "[redacted]")
    return text[-4096:]


def find_listener(capture: BoundedCapture) -> tuple[str, int] | None:
    if capture.oversized:
        raise SmokeError("child stderr exceeded bounded output storage")
    # A pipe read is not a record boundary. Only LF terminates the daemon's
    # announcement; retain an incomplete tail for the next bounded snapshot.
    # In particular, splitlines() includes its unterminated final fragment.
    for line in capture.snapshot().split(b"\n")[:-1]:
        if line.startswith(LISTEN_PREFIX):
            return validate_listener_line(line)
    return None


def wait_for_listener(
    child: subprocess.Popen[bytes], capture: BoundedCapture, deadline: float
) -> tuple[str, int]:
    while time.monotonic() < deadline:
        if capture.oversized:
            raise SmokeError("child stderr exceeded bounded output storage")
        listener = find_listener(capture)
        if listener is not None:
            return listener
        code = child.poll()
        if code is not None:
            raise SmokeError(f"child exited before listener announcement (exit {code})")
        time.sleep(0.02)
    raise SmokeError("child startup deadline elapsed before listener announcement")


def wait_for_child_listener(
    child: subprocess.Popen[bytes], capture: BoundedCapture, deadline: float, secret: str
) -> tuple[str, int]:
    try:
        return wait_for_listener(child, capture, deadline)
    except SmokeError as error:
        diagnostic = redact(capture.snapshot(), secret).strip()
        suffix = f"; child_stderr={diagnostic}" if diagnostic else ""
        raise SmokeError(str(error) + suffix) from error


def cleanup_owned_child(child: subprocess.Popen[bytes]) -> str:
    """Terminate only this Popen child; never signal a PID or process group."""
    if child.poll() is not None:
        return "already_exited"
    try:
        child.terminate()
    except ProcessLookupError:
        child.wait(timeout=FINAL_WAIT_SECONDS)
        return "already_exited"
    try:
        child.wait(timeout=TERM_WAIT_SECONDS)
        return "terminated"
    except subprocess.TimeoutExpired:
        if child.poll() is not None:
            return "already_exited"
    try:
        child.kill()
    except ProcessLookupError:
        child.wait(timeout=FINAL_WAIT_SECONDS)
        return "already_exited"
    try:
        child.wait(timeout=FINAL_WAIT_SECONDS)
    except subprocess.TimeoutExpired as error:
        raise SmokeError("owned child did not exit after handle-bound kill") from error
    return "killed_after_term_timeout"


def bounded_http(
    host: str,
    port: int,
    method: str,
    path: str,
    *,
    headers: dict[str, str] | None = None,
    body: bytes | None = None,
    deadline: float,
) -> tuple[int, dict[str, str], bytes]:
    address = ipaddress.ip_address(host)
    require(address.is_loopback, "HTTP destination is not loopback")
    require(port not in PROTECTED_PORTS, "protected legacy port is forbidden")
    require(path.startswith("/") and "\r" not in path and "\n" not in path, "invalid fixed API path")
    request_deadline = min(deadline, time.monotonic() + MAX_REQUEST_SECONDS)
    remaining = request_deadline - time.monotonic()
    require(remaining > 0, "whole-smoke deadline elapsed")
    connection = http.client.HTTPConnection(address.compressed, port, timeout=remaining)
    watchdog: threading.Timer | None = None
    expired = threading.Event()
    response: http.client.HTTPResponse | None = None

    def check_deadline() -> float:
        remaining = request_deadline - time.monotonic()
        require(not expired.is_set() and remaining > 0, "per-request HTTP deadline elapsed")
        return remaining

    try:
        connection.connect()
        check_deadline()
        wire_socket = connection.sock
        require(wire_socket is not None, "HTTP socket was not created")
        def abort_request() -> None:
            # shutdown may wake a buffered read as EOF rather than an exception.
            # Keep an independent expiry marker and check after blocking I/O.
            expired.set()
            try:
                wire_socket.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        watchdog = threading.Timer(max(0.001, request_deadline - time.monotonic()), abort_request)
        watchdog.daemon = True
        watchdog.start()
        wire_socket.settimeout(check_deadline())
        connection.request(method, path, body=body, headers=headers or {})
        wire_socket.settimeout(check_deadline())
        response = connection.getresponse()
        check_deadline()
        size = response.getheader("Content-Length")
        if size is not None:
            require(size.isascii() and size.isdecimal(), "invalid HTTP content length")
            require(int(size) <= MAX_HTTP_BYTES, "HTTP response exceeds byte bound")
        # http.client normalizes HEAD/204/304 and chunked semantics here. read1
        # itself does not reject a premature EOF for a declared body length.
        expected_length = response.length
        data = bytearray()
        while True:
            # HTTP/1.0 close semantics can clear connection.sock while the
            # response still owns a file reference to this exact wire socket.
            remaining = check_deadline()
            amount = min(16 * 1024, MAX_HTTP_BYTES + 1 - len(data))
            # A completed close-delimited response may already have released its
            # final socket reference. Do not set a timeout on that closed socket.
            if response.isclosed():
                chunk = b""
            else:
                wire_socket.settimeout(remaining)
                chunk = response.read1(amount)
            check_deadline()
            if not chunk:
                require(expected_length is None or len(data) == expected_length,
                        "HTTP response ended before declared content length")
                break
            data.extend(chunk)
            require(len(data) <= MAX_HTTP_BYTES, "HTTP response exceeds byte bound")
        check_deadline()
        return response.status, {key.lower(): value for key, value in response.getheaders()}, bytes(data)
    except (OSError, http.client.HTTPException, TimeoutError) as error:
        raise SmokeError("bounded loopback HTTP request failed") from error
    finally:
        if watchdog is not None:
            watchdog.cancel()
        if response is not None:
            response.close()
        connection.close()


def parse_json_body(data: bytes) -> dict[str, Any]:
    require(len(data) <= MAX_JSON_BYTES, "JSON response exceeds one MiB")
    try:
        value = json.loads(data)
    except (ValueError, UnicodeDecodeError, RecursionError) as error:
        raise SmokeError("response is not valid JSON") from error
    require(isinstance(value, dict), "expected JSON object")
    require(json_depth(value) <= 32, "JSON response nesting exceeds bound")
    validate_json_bounds(value)
    return value


def json_depth(value: Any, depth: int = 0) -> int:
    if isinstance(value, dict):
        return max([depth] + [json_depth(key, depth + 1) for key in value] +
                   [json_depth(item, depth + 1) for item in value.values()])
    if isinstance(value, list):
        return max([depth] + [json_depth(item, depth + 1) for item in value])
    return depth


def validate_json_bounds(value: Any) -> None:
    if isinstance(value, dict):
        require(len(value) <= MAX_ITEMS, "JSON object exceeds item bound")
        for key, item in value.items():
            require(isinstance(key, str) and len(key.encode("utf-8")) <= MAX_STRING_BYTES,
                    "JSON key exceeds string bound")
            validate_json_bounds(item)
    elif isinstance(value, list):
        require(len(value) <= MAX_ITEMS, "JSON collection exceeds item bound")
        for item in value:
            validate_json_bounds(item)
    elif isinstance(value, str):
        require(len(value.encode("utf-8")) <= MAX_STRING_BYTES, "JSON string exceeds bound")


def generated_model_policy() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "revision": "0",
        "models": [{
            "id": "canary-smoke-model",
            "label": "Synthetic canary policy",
            "provider": "codex_cli",
            "provider_model": "synthetic-canary-model",
            "tier": 0,
            "enabled": True,
            "node_id": None,
            "endpoint": None,
            "credential_file_ref": None,
            "max_concurrency": 1,
            "capabilities": ["coding"],
            "task_max_attempts": 1,
            "escalate_to": None,
        }],
    }


def write_private(path: Path, data: bytes) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        os.close(descriptor)
    os.chmod(path, 0o600)


def isolated_env(directory: Path) -> dict[str, str]:
    return {
        "HOME": str(directory),
        "TMPDIR": str(directory),
        "TMP": str(directory),
        "TEMP": str(directory),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
    }


def launch_server(binary: Path, database: Path, token_file: Path, models_file: Path,
                  output: BoundedCapture, env: dict[str, str]) -> subprocess.Popen[bytes]:
    child = subprocess.Popen(
        [str(binary), "--database", str(database), "--token-file", str(token_file),
         "--models-file", str(models_file), "--listen", "127.0.0.1:0"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        shell=False, close_fds=True, env=env, bufsize=0,
    )
    drain_stderr(child, output)
    return child


def launch_demo(binary: Path, output: BoundedCapture, env: dict[str, str]) -> subprocess.Popen[bytes]:
    child = subprocess.Popen(
        [str(binary), "demo", "--listen", "127.0.0.1:0"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        shell=False, close_fds=True, env=env, bufsize=0,
    )
    drain_stderr(child, output)
    return child


def run_readonly(binary: Path, args: list[str], env: dict[str, str], deadline: float) -> tuple[int, bytes, bytes]:
    child = subprocess.Popen([str(binary), *args], stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, shell=False,
                             close_fds=True, env=env, bufsize=0)
    stdout_capture = BoundedCapture()
    stderr_capture = BoundedCapture()
    require(child.stdout is not None and child.stderr is not None, "CLI output pipes were not created")
    out_thread = drain_pipe(child.stdout, stdout_capture, "canary-cli-stdout")
    err_thread = drain_pipe(child.stderr, stderr_capture, "canary-cli-stderr")
    try:
        remaining = min(MAX_REQUEST_SECONDS, deadline - time.monotonic())
        require(remaining > 0, "whole-smoke deadline elapsed")
        child.wait(timeout=remaining)
        out_thread.join(timeout=0.1)
        err_thread.join(timeout=0.1)
        require(not stdout_capture.oversized and not stderr_capture.oversized,
                "read-only CLI output exceeded storage bound")
        return child.returncode, stdout_capture.snapshot(), stderr_capture.snapshot()
    except subprocess.TimeoutExpired as error:
        raise SmokeError("read-only CLI command exceeded deadline") from error
    finally:
        if child.poll() is None:
            cleanup_owned_child(child)


def record_check(results: list[dict[str, Any]], name: str, check: Any) -> Any:
    started = time.monotonic()
    try:
        result = check()
        results.append({"name": name, "state": "pass", "duration_ms": round((time.monotonic() - started) * 1000)})
        return result
    except Exception as error:
        results.append({"name": name, "state": "fail", "duration_ms": round((time.monotonic() - started) * 1000),
                        "diagnostic": str(error)[:512]})
        raise


def validate_fleet(value: dict[str, Any], *, simulated: bool) -> None:
    require(value.get("simulated") is simulated, "fleet simulated mode did not match expected mode")
    require(value.get("native_execution_qualified") is False,
            "fleet missing native_execution_qualified=false")


def validate_models(value: dict[str, Any]) -> None:
    require(value.get("native_execution_qualified") is False,
            "models missing native_execution_qualified=false")
    models = value.get("models")
    require(isinstance(models, list) and len(models) == 1, "synthetic model policy was not visible")
    require(models[0].get("id") == "canary-smoke-model", "unexpected model policy")


def server_checks(host: str, port: int, token: str, deadline: float,
                  results: list[dict[str, Any]]) -> None:
    def health() -> None:
        status, _, body = bounded_http(host, port, "GET", "/healthz", deadline=deadline)
        value = parse_json_body(body)
        require_status(status, 200, "health")
        require(value.get("status") == "ok", "health check did not return ok")
        require(value.get("native_execution_qualified") is False, "health reported native execution qualified")
    record_check(results, "real_health_native_unqualified", health)

    for path, name in (("/v1/fleet", "real_fleet"), ("/v1/models", "real_models")):
        def unauthorized(path: str = path) -> None:
            status, _, _ = bounded_http(host, port, "GET", path, deadline=deadline)
            require_status(status, 401, f"unauthorized {path}")
        record_check(results, f"unauthorized_{name}_rejected", unauthorized)

    def unauthorized_malformed_model_request() -> None:
        status, _, _ = bounded_http(host,port,"POST","/v1/models/select",
                                   headers={"Content-Type":"application/json"},body=b"{",deadline=deadline)
        require_status(status,401,"unauthorized malformed model request")
    record_check(results,"model_auth_precedes_body_validation",unauthorized_malformed_model_request)

    def fleet_authorized() -> None:
        status, _, body = bounded_http(host, port, "GET", "/v1/fleet",
                                       headers={"Authorization": f"Bearer {token}"}, deadline=deadline)
        value = parse_json_body(body)
        require_status(status, 200, "authorized real fleet")
        validate_fleet(value, simulated=False)
    record_check(results, "authorized_real_fleet", fleet_authorized)

    def models_authorized() -> None:
        status, _, body = bounded_http(host, port, "GET", "/v1/models",
                                       headers={"Authorization": f"Bearer {token}"}, deadline=deadline)
        value = parse_json_body(body)
        require_status(status, 200, "authorized model listing")
        validate_models(value)
    record_check(results, "authorized_models_policy", models_authorized)

    def model_selection() -> None:
        request = {"request": {"required_capabilities": ["coding"]}, "capacity": {}}
        status, _, body = bounded_http(host, port, "POST", "/v1/models/select",
                                       headers={"Authorization": f"Bearer {token}",
                                                "Content-Type": "application/json"},
                                       body=json.dumps(request, separators=(",", ":")).encode(),
                                       deadline=deadline)
        value = parse_json_body(body)
        require_status(status, 200, "model selection proposal")
        require(value.get("status") == "selected", "model selection proposal failed")
        require(value.get("model_id") == "canary-smoke-model", "model selection proposal changed policy")
        require(value.get("usage_cost", {}).get("status") == "unknown", "model proposal implied known cost")
    record_check(results, "model_selection_proposal_only", model_selection)


def demo_checks(host: str, port: int, deadline: float, results: list[dict[str, Any]]) -> None:
    def demo_fleet() -> None:
        status, _, body = bounded_http(host, port, "GET", "/v1/fleet", deadline=deadline)
        value = parse_json_body(body)
        require_status(status, 200, "demo fleet")
        validate_fleet(value, simulated=True)
    record_check(results, "demo_fleet_simulated", demo_fleet)

    for method, path, name in (("GET", "/v1/models", "models"), ("PUT", "/v1/models", "model_policy_write"),
                               ("GET", "/v1/legacy", "legacy_policy")):
        def absent(method: str = method, path: str = path) -> None:
            status, _, _ = bounded_http(host, port, method, path,
                                        headers={"Content-Type": "application/json"},
                                        body=b"{}" if method == "PUT" else None, deadline=deadline)
            require_status(status, 404, f"demo {path}")
        record_check(results, f"demo_no_{name}", absent)


def binary_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def execute_smoke(server_binary_arg: str, cli_binary_arg: str, scratch_arg: str) -> dict[str, Any]:
    started_at = datetime.now(timezone.utc).isoformat()
    started = time.monotonic()
    deadline = started + MAX_TOTAL_SECONDS
    results: list[dict[str, Any]] = []
    server_binary = validate_absolute_owned_file(server_binary_arg, executable=True)
    cli_binary = validate_absolute_owned_file(cli_binary_arg, executable=True)
    scratch = validate_scratch_root(scratch_arg)
    require(server_binary != cli_binary, "server and CLI binaries must be distinct paths")
    real_root: Path | None = None
    demo_root: Path | None = None
    real_child: subprocess.Popen[bytes] | None = None
    demo_child: subprocess.Popen[bytes] | None = None
    cleanup: dict[str, str] = {}
    try:
        real_root = Path(tempfile.mkdtemp(prefix="adm-canary-real-", dir=scratch))
        os.chmod(real_root, 0o700)
        demo_root = Path(tempfile.mkdtemp(prefix="adm-canary-demo-", dir=scratch))
        os.chmod(demo_root, 0o700)
        token = secrets.token_urlsafe(32)
        token_file = real_root / "auth.token"
        model_file = real_root / "models.json"
        database = real_root / "state.sqlite"
        write_private(token_file, (token + "\n").encode("ascii"))
        policy = generated_model_policy()
        write_private(model_file, json.dumps(policy, separators=(",", ":")).encode("utf-8"))
        real_capture = BoundedCapture()
        demo_capture = BoundedCapture()
        env = isolated_env(real_root)
        demo_env = isolated_env(demo_root)
        real_child = launch_server(server_binary, database, token_file, model_file, real_capture, env)
        real_host, real_port = record_check(
            results, "real_server_startup",
            lambda: wait_for_child_listener(real_child, real_capture,
                                            min(deadline, time.monotonic() + MAX_STARTUP_SECONDS), token),
        )
        server_checks(real_host, real_port, token, deadline, results)
        real_listener = (real_host, real_port)

        demo_child = launch_demo(cli_binary, demo_capture, demo_env)
        demo_host, demo_port = record_check(
            results, "demo_startup",
            lambda: wait_for_child_listener(demo_child, demo_capture,
                                            min(deadline, time.monotonic() + MAX_STARTUP_SECONDS), token),
        )
        require((demo_host, demo_port) != real_listener, "real and demo listeners unexpectedly match")
        demo_checks(demo_host, demo_port, deadline, results)

        doctor_code, doctor_out, doctor_err = run_readonly(cli_binary, ["doctor"], demo_env, deadline)
        doctor = parse_json_body(doctor_out)
        require(doctor_code == 0 and doctor.get("native_execution_qualified") is False,
                "doctor did not report native execution unqualified")
        results.append({"name": "doctor_read_only_capability", "state": "pass"})
        help_code, help_out, help_err = run_readonly(cli_binary, ["demo", "--help"], demo_env, deadline)
        help_text = (help_out + help_err).decode("utf-8", errors="replace")
        require(help_code == 0 and "--models-file" not in help_text and "--legacy-status-file" not in help_text,
                "demo CLI advertises real model or legacy configuration flags")
        results.append({"name": "demo_cli_has_no_real_policy_flags", "state": "pass"})
        return {
            "started_at_utc": started_at,
            "duration_ms": round((time.monotonic() - started) * 1000),
            "status": "pass",
            "synthetic": False,
            "clock": "UTC start timestamp plus monotonic elapsed/deadline measurements",
            "randomness": "ephemeral token from OS CSPRNG; token value not recorded",
            "checks": results,
            "binaries": {"admiraltyd_sha256": binary_digest(server_binary),
                         "admiralty_sha256": binary_digest(cli_binary)},
            "cleanup": cleanup,
            "acceptance": False,
            "fleet_rollout_closure": False,
        }
    except Exception as error:
        return {
            "started_at_utc": started_at,
            "duration_ms": round((time.monotonic() - started) * 1000),
            "status": "fail",
            "synthetic": False,
            "clock": "UTC start timestamp plus monotonic elapsed/deadline measurements",
            "randomness": "ephemeral token from OS CSPRNG; token value not recorded",
            "checks": results,
            "diagnostic": str(error)[:512],
            "binaries": {"admiraltyd_sha256": binary_digest(server_binary),
                         "admiralty_sha256": binary_digest(cli_binary)},
            "cleanup": cleanup,
            "acceptance": False,
            "fleet_rollout_closure": False,
        }
    finally:
        if demo_child is not None:
            cleanup["demo"] = cleanup_owned_child(demo_child)
        if real_child is not None:
            cleanup["admiraltyd"] = cleanup_owned_child(real_child)
        for directory in (demo_root, real_root):
            if directory is None:
                continue
            try:
                directory.chmod(0o700)
                # The scope contains only this run's policy, generated token, DB and logs.
                shutil.rmtree(directory)
            except OSError:
                cleanup["scratch_cleanup"] = "incomplete"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--admiraltyd-binary")
    parser.add_argument("--admiralty-binary")
    parser.add_argument("--scratch-root")
    parser.add_argument("--self-test", action="store_true", help="run synthetic stdlib tests only")
    args = parser.parse_args(argv)
    if args.self_test:
        if args.admiraltyd_binary or args.admiralty_binary or args.scratch_root:
            parser.error("--self-test cannot be combined with real binary or scratch arguments")
    elif not (args.admiraltyd_binary and args.admiralty_binary and args.scratch_root):
        parser.error("real smoke requires explicit --admiraltyd-binary, --admiralty-binary and --scratch-root")
    return args


class HarnessTests(unittest.TestCase):
    def test_argument_validation_requires_explicit_paths(self) -> None:
        with self.assertRaises(SystemExit):
            parse_args([])
        with self.assertRaises(SystemExit):
            parse_args(["--self-test", "--scratch-root", "/tmp"])

    def test_binary_path_rejects_relative_and_symlink(self) -> None:
        with self.assertRaises(SmokeError):
            validate_absolute_owned_file("relative/path")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = root / "target"
            target.write_text("x")
            link = root / "link"
            link.symlink_to(target)
            with self.assertRaises(SmokeError):
                validate_absolute_owned_file(str(link))

    def test_scratch_requires_private_owned_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            root.chmod(0o700)
            self.assertEqual(validate_scratch_root(str(root)), root)
            root.chmod(0o755)
            with self.assertRaises(SmokeError):
                validate_scratch_root(str(root))

    def test_listener_rejects_bad_future_and_protected_sources(self) -> None:
        self.assertEqual(validate_listener_line(b"Admiralty listening on http://127.0.0.1:12345 (read-only; native execution disabled)"),
                         ("127.0.0.1", 12345))
        for line in (b"future listener version http://127.0.0.1:12345",
                     b"Admiralty listening on http://example.com:12345",
                     b"Admiralty listening on http://127.0.0.1:18891",
                     b"Admiralty listening on https://127.0.0.1:12345",
                     b"Admiralty listening on http://127.0.0.1:0"):
            with self.subTest(line=line), self.assertRaises(SmokeError):
                validate_listener_line(line)

    def test_listener_waits_for_every_split_and_crlf_record(self) -> None:
        for ending in (b"\n", b"\r\n"):
            line = (LISTEN_PREFIX + b"127.0.0.1:12345"
                    b" (read-only; native execution disabled)" + ending)
            for split in range(len(line)):
                with self.subTest(ending=ending, split=split):
                    capture = BoundedCapture()
                    capture.append(line[:split])
                    self.assertIsNone(find_listener(capture))
                    capture.append(line[split:])
                    self.assertEqual(find_listener(capture), ("127.0.0.1", 12345))

    def test_listener_unterminated_eof_is_not_a_record(self) -> None:
        class Exited:
            def poll(self) -> int:
                return 0

        for tail in (LISTEN_PREFIX, LISTEN_PREFIX + b"127.0.0.1:12345",
                     LISTEN_PREFIX + b"127.0.0.1:12345 (read-only; native execution disabled)"):
            with self.subTest(tail=tail):
                capture = BoundedCapture()
                capture.append(b"unrelated complete diagnostic\n" + tail)
                capture.eof = True
                self.assertIsNone(find_listener(capture))
                with self.assertRaisesRegex(SmokeError, "child exited before listener announcement"):
                    wait_for_listener(Exited(), capture, time.monotonic() + 0.1)

    def test_listener_complete_malformed_record_still_fails(self) -> None:
        suffix = b" (read-only; native execution disabled)\n"
        for line in (LISTEN_PREFIX + b"127.0.0.1:12345 (future mode)\n",
                     LISTEN_PREFIX + b"127.0.0.1:18890" + suffix,
                     LISTEN_PREFIX + b"127.0.0.1:18891" + suffix,
                     LISTEN_PREFIX + b"192.0.2.1:12345" + suffix,
                     LISTEN_PREFIX + b"example.com:12345" + suffix):
            with self.subTest(line=line):
                capture = BoundedCapture()
                capture.append(line)
                with self.assertRaises(SmokeError):
                    find_listener(capture)

    def test_listener_ignores_only_complete_unrelated_records(self) -> None:
        capture = BoundedCapture()
        capture.append(b"unrelated\nnot a boundary\v" + LISTEN_PREFIX
                       + b"127.0.0.1:12345 (read-only; native execution disabled)\n")
        self.assertIsNone(find_listener(capture))
        capture.append(LISTEN_PREFIX
                       + b"[::1]:12345 (read-only; native execution disabled)\n")
        self.assertEqual(find_listener(capture), ("::1", 12345))

    def test_listener_actual_pipe_can_pause_between_prefix_and_suffix(self) -> None:
        prefix = LISTEN_PREFIX
        suffix = b"127.0.0.1:12345 (read-only; native execution disabled)\n"
        code = (f"import os,sys,time; os.write(2,{prefix!r}); "
                f"sys.stdin.buffer.read(1); os.write(2,{suffix!r}); time.sleep(10)")
        child = subprocess.Popen([sys.executable, "-c", code], stdin=subprocess.PIPE,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                 shell=False, bufsize=0)
        capture = BoundedCapture()
        reader = drain_stderr(child, capture)
        try:
            deadline = time.monotonic() + 2
            while len(capture.snapshot()) < len(prefix) and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertEqual(capture.snapshot(), prefix)
            self.assertIsNone(find_listener(capture))
            self.assertIsNotNone(child.stdin)
            child.stdin.write(b"x")
            child.stdin.flush()
            self.assertEqual(wait_for_listener(child, capture, deadline), ("127.0.0.1", 12345))
        finally:
            cleanup_owned_child(child)
            reader.join(timeout=FINAL_WAIT_SECONDS)
            if child.stdin is not None:
                child.stdin.close()
            if child.stderr is not None:
                child.stderr.close()

    def test_capture_is_bounded(self) -> None:
        capture = BoundedCapture(8)
        capture.append(b"123456789")
        self.assertTrue(capture.oversized)
        self.assertEqual(capture.snapshot(), b"12345678")

    def test_json_bounds_reject_oversized_strings_and_arrays(self) -> None:
        with self.assertRaises(SmokeError):
            validate_json_bounds({"x": "z" * (MAX_STRING_BYTES + 1)})
        with self.assertRaises(SmokeError):
            validate_json_bounds({"x": list(range(MAX_ITEMS + 1))})

    def test_auth_and_real_demo_expectations_are_strict(self) -> None:
        self.assertFalse(generated_model_policy()["models"][0]["endpoint"])
        with self.assertRaises(SmokeError):
            require_status(200, 401, "unauthorized request")
        require_status(401, 401, "unauthorized request")
        with self.assertRaises(SmokeError):
            validate_fleet({"simulated": True, "native_execution_qualified": False}, simulated=False)
        with self.assertRaises(SmokeError):
            validate_fleet({"simulated": False}, simulated=False)
        with self.assertRaises(SmokeError):
            validate_models({"native_execution_qualified": False, "models": []})

    def test_protected_ports_are_rejected_before_connection(self) -> None:
        with self.assertRaises(SmokeError):
            bounded_http("127.0.0.1", 18890, "GET", "/healthz", deadline=time.monotonic() + 1)
        with self.assertRaises(SmokeError):
            bounded_http("127.0.0.1", 18891, "GET", "/healthz", deadline=time.monotonic() + 1)

    def test_http_response_byte_limit(self) -> None:
        import http.server

        class LargeHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                self.send_response(200)
                self.send_header("Content-Length", str(MAX_HTTP_BYTES + 1))
                self.end_headers()
            def log_message(self, *_: Any) -> None:
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), LargeHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with self.assertRaises(SmokeError):
                bounded_http("127.0.0.1", server.server_port, "GET", "/", deadline=time.monotonic() + 2)
        finally:
            server.shutdown()
            server.server_close()

    def test_http_request_has_total_deadline_under_slow_drip(self) -> None:
        import http.server

        class SlowHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                self.send_response(200)
                self.send_header("Content-Length", "100")
                self.end_headers()
                self.wfile.write(b"x")
                self.wfile.flush()
                time.sleep(0.3)
                try:
                    self.wfile.write(b"x" * 99)
                    self.wfile.flush()
                except OSError:
                    pass
            def log_message(self, *_: Any) -> None:
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), SlowHandler)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        global MAX_REQUEST_SECONDS
        saved_timeout = MAX_REQUEST_SECONDS
        MAX_REQUEST_SECONDS = 0.05
        before = time.monotonic()
        try:
            with self.assertRaises(SmokeError):
                bounded_http("127.0.0.1", server.server_port, "GET", "/", deadline=time.monotonic() + 1)
            self.assertLess(time.monotonic() - before, 0.5)
        finally:
            MAX_REQUEST_SECONDS = saved_timeout
            server.shutdown()
            server.server_close()

    def test_header_drip_cannot_extend_whole_request_deadline(self) -> None:
        import socketserver
        class Drip(socketserver.BaseRequestHandler):
            def handle(self) -> None:
                self.request.recv(4096)
                for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}":
                    try:
                        self.request.sendall(bytes([byte]))
                    except OSError:
                        break
                    time.sleep(0.02)
        server = socketserver.ThreadingTCPServer(("127.0.0.1", 0),Drip)
        server.daemon_threads=True
        thread=threading.Thread(target=server.serve_forever,daemon=True)
        thread.start()
        before=time.monotonic()
        try:
            with self.assertRaises(SmokeError):
                bounded_http("127.0.0.1",server.server_address[1],"GET","/",deadline=before+0.08)
            self.assertLess(time.monotonic()-before,0.5)
        finally:
            server.shutdown()
            server.server_close()

    def test_eof_after_deadline_cannot_be_reported_as_success(self) -> None:
        from unittest.mock import patch

        for expiry in ("clock", "watchdog"):
            with self.subTest(expiry=expiry):
                clock = [100.0]
                callbacks: list[Any] = []

                class FakeTimer:
                    def __init__(self, interval: float, callback: Any) -> None:
                        callbacks.append(callback)
                    def start(self) -> None:
                        pass
                    def cancel(self) -> None:
                        pass

                class FakeSocket:
                    def settimeout(self, timeout: float) -> None:
                        pass
                    def shutdown(self, how: int) -> None:
                        pass

                class FakeResponse:
                    status = 200
                    length = None
                    closed = False
                    def getheader(self, name: str) -> None:
                        return None
                    def getheaders(self) -> list[Any]:
                        return []
                    def isclosed(self) -> bool:
                        return self.closed
                    def read1(self, amount: int) -> bytes:
                        # A deterministic stand-in for shutdown waking read1 as
                        # EOF, without relying on OS timeout/Timer scheduling.
                        if expiry == "clock":
                            clock[0] = 101.0
                        else:
                            callbacks[0]()
                        return b""
                    def close(self) -> None:
                        self.closed = True

                response = FakeResponse()

                class FakeConnection:
                    sock = FakeSocket()
                    closed = False
                    def connect(self) -> None:
                        pass
                    def request(self, *_: Any, **__: Any) -> None:
                        pass
                    def getresponse(self) -> FakeResponse:
                        return response
                    def close(self) -> None:
                        self.closed = True

                connection = FakeConnection()
                with patch.object(time, "monotonic", side_effect=lambda: clock[0]), \
                        patch.object(threading, "Timer", FakeTimer), \
                        patch.object(http.client, "HTTPConnection", return_value=connection):
                    with self.assertRaisesRegex(SmokeError, "per-request HTTP deadline"):
                        bounded_http("127.0.0.1", 12345, "GET", "/", deadline=100.05)
                self.assertTrue(response.closed)
                self.assertTrue(connection.closed)

    def test_clean_close_delimited_response_and_premature_content_length_eof(self) -> None:
        import http.server

        class BodyHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                self.send_response(200)
                self.send_header("Content-Length", "2" if self.path == "/complete" else "3")
                self.end_headers()
                self.wfile.write(b"{}")
                self.wfile.flush()
            def log_message(self, *_: Any) -> None:
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), BodyHandler)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            status, _, body = bounded_http("127.0.0.1", server.server_port, "GET", "/complete",
                                          deadline=time.monotonic() + 1)
            self.assertEqual((status, body), (200, b"{}"))
            with self.assertRaisesRegex(SmokeError, "before declared content length"):
                bounded_http("127.0.0.1", server.server_port, "GET", "/truncated",
                             deadline=time.monotonic() + 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_startup_timeout_is_bounded(self) -> None:
        capture = BoundedCapture()
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.PIPE, shell=False)
        drain_stderr(child, capture)
        before = time.monotonic()
        try:
            with self.assertRaises(SmokeError):
                wait_for_listener(child, capture, time.monotonic() + 0.05)
        finally:
            cleanup_owned_child(child)
        self.assertLess(time.monotonic() - before, 2.5)

    def test_oversized_child_output_fails_startup_with_bounded_capture(self) -> None:
        capture = BoundedCapture(16)
        child = subprocess.Popen(
            [sys.executable, "-c", "import os,time; os.write(2,b'x'*4096); time.sleep(10)"],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE, shell=False, bufsize=0,
        )
        drain_stderr(child, capture)
        try:
            deadline = time.monotonic() + 1
            while not capture.oversized and time.monotonic() < deadline:
                time.sleep(0.01)
            with self.assertRaisesRegex(SmokeError, "exceeded bounded output"):
                wait_for_listener(child, capture, time.monotonic() + 0.1)
        finally:
            cleanup_owned_child(child)

    def test_child_exit_before_listener_is_reported(self) -> None:
        capture = BoundedCapture()
        child = subprocess.Popen([sys.executable, "-c", "raise SystemExit(7)"],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.PIPE, shell=False)
        drain_stderr(child, capture)
        with self.assertRaisesRegex(SmokeError, "exit 7"):
            wait_for_listener(child, capture, time.monotonic() + 1)
        self.assertEqual(cleanup_owned_child(child), "already_exited")

    def test_term_ignoring_child_gets_handle_bound_kill_and_is_reaped(self) -> None:
        # Deliberately slow startup proves cleanup waits for the installed
        # handler, rather than racing a fixed sleep against interpreter startup.
        code = "import signal,time; time.sleep(0.2); signal.signal(signal.SIGTERM, signal.SIG_IGN); print('ready',flush=True); time.sleep(30)"
        child = subprocess.Popen([sys.executable, "-c", code], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, shell=False)
        ready = threading.Event()
        output: list[bytes] = []

        def read_ready() -> None:
            assert child.stdout is not None
            output.append(child.stdout.read(6))
            ready.set()

        reader = threading.Thread(target=read_ready, daemon=True)
        reader.start()
        try:
            self.assertTrue(ready.wait(MAX_STARTUP_SECONDS), "child did not install its SIGTERM handler")
            self.assertEqual(output, [b"ready\n"])
            before = time.monotonic()
            result = cleanup_owned_child(child)
            self.assertEqual(result, "killed_after_term_timeout")
            self.assertIsNotNone(child.returncode)
            self.assertLess(time.monotonic() - before, TERM_WAIT_SECONDS + FINAL_WAIT_SECONDS + 1)
        finally:
            cleanup_owned_child(child)
            reader.join(timeout=FINAL_WAIT_SECONDS)
            if child.stdout is not None:
                child.stdout.close()

    def test_redaction_never_emits_generated_token(self) -> None:
        self.assertNotIn("synthetic-secret", redact(b"token=synthetic-secret", "synthetic-secret"))


def self_test() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(HarnessTests)
    result = unittest.TextTestRunner(stream=sys.stderr, verbosity=2).run(suite)
    print(json.dumps({"status": "pass" if result.wasSuccessful() else "fail",
                      "synthetic": True, "tests_run": result.testsRun,
                      "failures": len(result.failures), "errors": len(result.errors),
                      "acceptance": False, "fleet_rollout_closure": False}, separators=(",", ":")))
    return 0 if result.wasSuccessful() else 1


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.self_test:
        return self_test()
    try:
        receipt = execute_smoke(args.admiraltyd_binary, args.admiralty_binary, args.scratch_root)
    except Exception as error:
        receipt = {"started_at_utc": datetime.now(timezone.utc).isoformat(), "status": "fail",
                   "synthetic": False, "checks": [], "diagnostic": str(error)[:512],
                   "acceptance": False, "fleet_rollout_closure": False}
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    return 0 if receipt.get("status") == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
