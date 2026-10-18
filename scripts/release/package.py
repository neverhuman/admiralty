#!/usr/bin/env python3
"""Build a deterministic archive from an explicit, bounded list of regular files."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import stat
import tarfile
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from typing import BinaryIO

MAX_INPUT_JSON = 1024 * 1024
MAX_FILES = 1024
MAX_STRING = 4096
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_TOTAL_BYTES = 1024 * 1024 * 1024
CHUNK_BYTES = 1024 * 1024
MANIFEST_PATH = "MANIFEST.json"
DENIED_COMPONENTS = {
    ".git", ".ssh", ".codex", "runtime", "runtimes", "state", "states",
    "private", "credential", "credentials", "secret", "secrets", "key",
    "keys", "token", "tokens", "id_rsa", "id_ed25519",
}
SENSITIVE_NAME = re.compile(r"(?:^|[._-])(token|tokens|secret|secrets|credential|credentials|private|key|keys)(?:$|[._-])")


class PackageError(ValueError):
    """Raised for invalid or unsafe package inputs."""


def _bounded_text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise PackageError(f"{label} must be a nonempty string of at most {MAX_STRING} bytes")
    try:
        encoded_size = len(value.encode("utf-8"))
    except UnicodeEncodeError as exc:
        raise PackageError(f"{label} contains invalid Unicode") from exc
    if encoded_size > MAX_STRING:
        raise PackageError(f"{label} must be a nonempty string of at most {MAX_STRING} bytes")
    if any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise PackageError(f"{label} contains a control character")
    return value


def _archive_path(value: object) -> str:
    raw = _bounded_text(value, "archive_path")
    if "\\" in raw or raw.startswith("/") or ":" in raw:
        raise PackageError("archive_path must be a relative POSIX path")
    parts = raw.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise PackageError("archive_path must not contain empty, dot, or parent components")
    lowered = [part.lower() for part in parts]
    for part in lowered:
        if part in DENIED_COMPONENTS or part == ".env" or part.startswith(".env.") or SENSITIVE_NAME.search(part):
            raise PackageError("archive_path names a private or credential-bearing path")
        if part.endswith((".pem", ".key", ".token", ".p12", ".pfx")):
            raise PackageError("archive_path names a private or credential-bearing file")
    normalized = PurePosixPath(*parts).as_posix()
    if normalized == MANIFEST_PATH:
        raise PackageError("archive_path is reserved for the package manifest")
    return normalized


def _source_path(value: object) -> str:
    raw = _bounded_text(value, "source")
    if "\\" in raw or raw.startswith("/") or ":" in raw:
        raise PackageError("source must be a relative POSIX path beneath --input-root")
    parts = raw.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise PackageError("source must not contain empty, dot, or parent components")
    for part in (p.lower() for p in parts):
        if part in DENIED_COMPONENTS or part == ".env" or part.startswith(".env.") or SENSITIVE_NAME.search(part):
            raise PackageError("source names a private or credential-bearing path")
        if part.endswith((".pem", ".key", ".token", ".p12", ".pfx")):
            raise PackageError("source names a private or credential-bearing file")
    return PurePosixPath(*parts).as_posix()


def _no_symlink_components(path: Path) -> None:
    absolute = Path(os.path.abspath(path))
    current = Path(absolute.anchor)
    for part in absolute.parts[1:]:
        current = current / part
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            raise PackageError(f"path component does not exist: {current}") from None
        if stat.S_ISLNK(mode):
            raise PackageError(f"symlink path component is forbidden: {current}")


def _validate_root(root: Path) -> Path:
    absolute = Path(os.path.abspath(root))
    _no_symlink_components(absolute)
    if not stat.S_ISDIR(absolute.lstat().st_mode):
        raise PackageError("--input-root must be an existing non-symlink directory")
    return absolute


def _parse_inputs(raw: bytes) -> list[dict[str, object]]:
    if len(raw) > MAX_INPUT_JSON:
        raise PackageError("input JSON exceeds 1 MiB")
    try:
        document = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PackageError(f"invalid input JSON: {exc}") from exc
    if not isinstance(document, dict) or set(document) != {"files"} or not isinstance(document["files"], list):
        raise PackageError("input JSON must contain only a files array")
    records = document["files"]
    if not records or len(records) > MAX_FILES:
        raise PackageError(f"files must contain 1..{MAX_FILES} entries")
    parsed: list[dict[str, object]] = []
    seen: set[str] = set()
    for index, item in enumerate(records):
        if not isinstance(item, dict) or set(item) != {"source", "archive_path", "mode"}:
            raise PackageError(f"files[{index}] must contain exactly source, archive_path, and mode")
        source = _source_path(item["source"])
        archive = _archive_path(item["archive_path"])
        mode = item["mode"]
        if isinstance(mode, bool) or not isinstance(mode, int) or mode not in (0o644, 0o755):
            raise PackageError(f"files[{index}].mode must be 420 (0644) or 493 (0755)")
        if archive in seen:
            raise PackageError(f"duplicate normalized archive path: {archive}")
        seen.add(archive)
        parsed.append({"source": source, "archive_path": archive, "mode": mode})
    return sorted(parsed, key=lambda record: str(record["archive_path"]))


def _validate_metadata(version: object, target: object, source_commit: object) -> tuple[str, str, str]:
    version_text = _bounded_text(version, "version")
    target_text = _bounded_text(target, "target")
    commit_text = _bounded_text(source_commit, "source_commit")
    if not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]{0,127}", version_text):
        raise PackageError("version has invalid characters or format")
    if not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z._+-]{0,127}", target_text):
        raise PackageError("target has invalid characters or format")
    if not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", commit_text):
        raise PackageError("source_commit must be a full lowercase 40- or 64-character object ID")
    return version_text, target_text, commit_text


def _identity(st: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns, st.st_nlink


def _hash_fd(fd: int) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    os.lseek(fd, 0, os.SEEK_SET)
    while True:
        chunk = os.read(fd, CHUNK_BYTES)
        if not chunk:
            break
        total += len(chunk)
        if total > MAX_FILE_BYTES:
            raise PackageError("input file exceeds the per-file byte limit")
        digest.update(chunk)
    return digest.hexdigest(), total


def _capture_source(root: Path, relative: str, spool: BinaryIO) -> tuple[str, int]:
    candidate = root.joinpath(*relative.split("/"))
    _no_symlink_components(candidate)
    before_path = candidate.lstat()
    if not stat.S_ISREG(before_path.st_mode):
        raise PackageError(f"input is not a regular file: {relative}")
    if before_path.st_nlink != 1:
        raise PackageError(f"hard-linked input is ambiguous: {relative}")
    if before_path.st_size > MAX_FILE_BYTES:
        raise PackageError(f"input file exceeds the per-file byte limit: {relative}")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(candidate, flags)
    try:
        before_fd = os.fstat(fd)
        if not stat.S_ISREG(before_fd.st_mode) or _identity(before_fd) != _identity(before_path):
            raise PackageError(f"input changed while opening: {relative}")
        digest = hashlib.sha256()
        total = 0
        os.lseek(fd, 0, os.SEEK_SET)
        while True:
            chunk = os.read(fd, CHUNK_BYTES)
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_FILE_BYTES:
                raise PackageError(f"input file exceeds the per-file byte limit: {relative}")
            spool.write(chunk)
            digest.update(chunk)
        after_copy = os.fstat(fd)
        named_after = candidate.lstat()
        if _identity(before_fd) != _identity(after_copy) or _identity(before_fd) != _identity(named_after):
            raise PackageError(f"input changed during capture: {relative}")
        if total != before_fd.st_size:
            raise PackageError(f"input size changed during capture: {relative}")
        first_digest = digest.hexdigest()
        second_digest, second_size = _hash_fd(fd)
        if second_digest != first_digest or second_size != total:
            raise PackageError(f"input content changed during capture: {relative}")
        after_verify = os.fstat(fd)
        named_verified = candidate.lstat()
        if _identity(before_fd) != _identity(after_verify) or _identity(before_fd) != _identity(named_verified):
            raise PackageError(f"input changed during verification: {relative}")
        spool.seek(0)
        return first_digest, total
    finally:
        os.close(fd)


def _tar_info(name: str, size: int, mode: int) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.size = size
    info.mode = mode
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.type = tarfile.REGTYPE
    return info


def build_package(
    input_root: Path,
    inputs_json: Path,
    output: Path,
    version: str,
    target: str,
    source_commit: str,
) -> dict[str, object]:
    """Build an archive atomically. Refuses all pre-existing destination paths."""
    version_text, target_text, commit_text = _validate_metadata(version, target, source_commit)
    root = _validate_root(input_root)
    _no_symlink_components(inputs_json)
    input_stat = inputs_json.lstat()
    if not stat.S_ISREG(input_stat.st_mode):
        raise PackageError("--inputs must be a non-symlink regular file")
    with inputs_json.open("rb") as input_stream:
        input_bytes = input_stream.read(MAX_INPUT_JSON + 1)
    records = _parse_inputs(input_bytes)
    total_declared = 0
    for record in records:
        source = root.joinpath(*str(record["source"]).split("/"))
        _no_symlink_components(source)
        st = source.lstat()
        if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1:
            raise PackageError(f"input must be an unlinked regular file: {record['source']}")
        if st.st_size > MAX_FILE_BYTES:
            raise PackageError(f"input file exceeds the per-file byte limit: {record['source']}")
        total_declared += st.st_size
        if total_declared > MAX_TOTAL_BYTES:
            raise PackageError("declared inputs exceed the total byte limit")

    destination = Path(os.path.abspath(output))
    parent = destination.parent
    _no_symlink_components(parent)
    if not stat.S_ISDIR(parent.lstat().st_mode):
        raise PackageError("output parent must be an existing non-symlink directory")
    if os.path.lexists(destination):
        raise PackageError("output already exists; refusing to overwrite")

    entries: list[dict[str, object]] = []
    with tempfile.TemporaryDirectory(prefix="adm-pkg-stage-", dir=parent) as stage_name:
        stage = Path(stage_name)
        staged_files: list[tuple[dict[str, object], Path]] = []
        actual_total = 0
        for index, record in enumerate(records):
            spool_path = stage / f"{index:04d}.data"
            with spool_path.open("xb") as spool:
                digest, size = _capture_source(root, str(record["source"]), spool)
            actual_total += size
            if actual_total > MAX_TOTAL_BYTES:
                raise PackageError("captured inputs exceed the total byte limit")
            entries.append({
                "archive_path": record["archive_path"],
                "sha256": digest,
                "size": size,
                "mode": record["mode"],
            })
            staged_files.append((record, spool_path))

        manifest = {
            "schema_version": 1,
            "version": version_text,
            "target": target_text,
            "source_commit": commit_text,
            "entries": entries,
        }
        manifest_bytes = (json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode("utf-8")
        file_descriptor, temporary_name = tempfile.mkstemp(prefix=".adm-pkg-", suffix=".tmp", dir=parent)
        temporary_path = Path(temporary_name)
        try:
            with os.fdopen(file_descriptor, "wb") as output_stream:
                with gzip.GzipFile(fileobj=output_stream, mode="wb", filename="", mtime=0, compresslevel=9) as gzip_stream:
                    with tarfile.open(fileobj=gzip_stream, mode="w|", format=tarfile.PAX_FORMAT) as archive:
                        archive.addfile(_tar_info(MANIFEST_PATH, len(manifest_bytes), 0o644), io.BytesIO(manifest_bytes))
                        for record, spool_path in staged_files:
                            size = spool_path.stat().st_size
                            with spool_path.open("rb") as staged:
                                archive.addfile(_tar_info(str(record["archive_path"]), size, int(record["mode"])), staged)
                output_stream.flush()
                os.fchmod(output_stream.fileno(), 0o644)
                os.fsync(output_stream.fileno())
            # link() is an atomic no-replace operation on the same filesystem.
            os.link(temporary_path, destination, follow_symlinks=False)
            temporary_path.unlink()
            directory_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
        except FileExistsError as exc:
            raise PackageError("output appeared during creation; refusing to overwrite") from exc
        finally:
            try:
                temporary_path.unlink()
            except FileNotFoundError:
                pass
    return manifest


def _cli(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-root", type=Path)
    parser.add_argument("--inputs", type=Path, help="JSON file with an explicit files array")
    parser.add_argument("--version")
    parser.add_argument("--target")
    parser.add_argument("--source-commit")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return _self_test()
    if any(value is None for value in (args.input_root, args.inputs, args.version, args.target, args.source_commit, args.output)):
        parser.error("packaging requires --input-root, --inputs, --version, --target, --source-commit, and --output")
    try:
        result = build_package(args.input_root, args.inputs, args.output, args.version, args.target, args.source_commit)
    except (OSError, PackageError) as exc:
        parser.error(str(exc))
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


class PackageTests(unittest.TestCase):
    def setUp(self) -> None:
        self._scratch = Path(os.environ.get("ADM_PACKAGE_TEST_SCRATCH", "."))
        self._scratch.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="p30a-test-", dir=self._scratch)
        self.base = Path(self.temp.name)
        self.root = self.base / "input"
        self.root.mkdir()
        (self.root / "bin").mkdir()
        (self.root / "assets").mkdir()
        (self.root / "bin" / "demo").write_bytes(b"binary-one")
        (self.root / "assets" / "index.html").write_text("hello", encoding="utf-8")
        self.inputs = self.base / "inputs.json"
        self.output = self.base / "package.tar.gz"
        self.records = [
            {"source": "bin/demo", "archive_path": "bin/demo", "mode": 493},
            {"source": "assets/index.html", "archive_path": "share/admiralty/index.html", "mode": 420},
        ]
        self.write_inputs(self.records)
        self.commit = "a" * 40

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write_inputs(self, records: list[dict[str, object]]) -> None:
        self.inputs.write_text(json.dumps({"files": records}), encoding="utf-8")

    def package(self, output: Path | None = None, version: str = "1.2.3", target: str = "linux-x86_64", commit: str | None = None) -> dict[str, object]:
        return build_package(self.root, self.inputs, output or self.output, version, target, commit or self.commit)

    @staticmethod
    def members(path: Path) -> tuple[list[tarfile.TarInfo], dict[str, bytes]]:
        with tarfile.open(path, "r:gz") as archive:
            infos = archive.getmembers()
            contents = {info.name: archive.extractfile(info).read() for info in infos if info.isfile()}
        return infos, contents

    def test_reproducible_across_input_order_and_source_mtime(self) -> None:
        self.package()
        first = self.output.read_bytes()
        os.utime(self.root / "bin" / "demo", (123456789, 123456789))
        self.write_inputs(list(reversed(self.records)))
        second_output = self.base / "second.tar.gz"
        self.package(second_output)
        self.assertEqual(first, second_output.read_bytes())

    def test_manifest_hashes_and_archive_metadata(self) -> None:
        self.package()
        raw_archive = self.output.read_bytes()
        self.assertEqual(b"\x00\x00\x00\x00", raw_archive[4:8])
        self.assertEqual(0, raw_archive[3] & 0x08)
        infos, contents = self.members(self.output)
        manifest = json.loads(contents[MANIFEST_PATH])
        self.assertEqual(["MANIFEST.json", "bin/demo", "share/admiralty/index.html"], [i.name for i in infos])
        self.assertEqual("1.2.3", manifest["version"])
        self.assertEqual("linux-x86_64", manifest["target"])
        self.assertEqual(self.commit, manifest["source_commit"])
        for entry in manifest["entries"]:
            data = contents[entry["archive_path"]]
            self.assertEqual(hashlib.sha256(data).hexdigest(), entry["sha256"])
            self.assertEqual(len(data), entry["size"])
        for info in infos:
            self.assertEqual(0, info.mtime)
            self.assertEqual(0, info.uid)
            self.assertEqual(0, info.gid)
            self.assertEqual("", info.uname)
            self.assertEqual("", info.gname)
        self.assertEqual(0o644, infos[0].mode)
        self.assertEqual(0o755, infos[1].mode)
        self.assertEqual(0o644, infos[2].mode)

    def test_duplicate_traversal_private_and_reserved_names_rejected(self) -> None:
        bad_sets = [
            [self.records[0], {**self.records[1], "archive_path": "bin/./demo"}],
            [{**self.records[0], "archive_path": "../escape"}],
            [{**self.records[0], "archive_path": ".env.production"}],
            [{**self.records[0], "archive_path": "MANIFEST.json"}],
        ]
        for records in bad_sets:
            with self.subTest(records=records):
                self.write_inputs(records)
                with self.assertRaises(PackageError):
                    self.package(self.base / f"bad-{bad_sets.index(records)}.tgz")
        self.write_inputs([{**self.records[0], "source": ".ssh/id_ed25519"}])
        with self.assertRaises(PackageError):
            self.package(self.base / "private.tgz")

    def test_symlink_files_and_parents_are_rejected(self) -> None:
        link = self.root / "bin" / "linked"
        link.symlink_to(self.root / "bin" / "demo")
        self.write_inputs([{"source": "bin/linked", "archive_path": "bin/linked", "mode": 420}])
        with self.assertRaises(PackageError):
            self.package(self.base / "symlink.tgz")
        parent = self.root / "linked-dir"
        parent.symlink_to(self.root / "assets", target_is_directory=True)
        self.write_inputs([{"source": "linked-dir/index.html", "archive_path": "index.html", "mode": 420}])
        with self.assertRaises(PackageError):
            self.package(self.base / "symlink-parent.tgz")
        output_parent = self.base / "output-link"
        output_parent.symlink_to(self.base, target_is_directory=True)
        with self.assertRaises(PackageError):
            self.package(output_parent / "unsafe.tgz")

    def test_missing_oversized_and_invalid_metadata_rejected(self) -> None:
        self.write_inputs([{"source": "missing", "archive_path": "missing", "mode": 420}])
        with self.assertRaises((OSError, PackageError)):
            self.package(self.base / "missing.tgz")
        big = self.root / "oversized"
        with big.open("wb") as stream:
            stream.truncate(MAX_FILE_BYTES + 1)
        self.write_inputs([{"source": "oversized", "archive_path": "oversized", "mode": 420}])
        with self.assertRaises(PackageError):
            self.package(self.base / "oversized.tgz")
        for version, target, commit in [("../bad", "linux", self.commit), ("1.0", "../bad", self.commit), ("1", "linux", "short")]:
            with self.assertRaises(PackageError):
                self.package(self.base / f"invalid-{version}.tgz", version, target, commit)

    def test_output_no_overwrite_symlink_and_race(self) -> None:
        self.output.write_bytes(b"keep")
        with self.assertRaises(PackageError):
            self.package()
        self.assertEqual(b"keep", self.output.read_bytes())
        self.output.unlink()
        symlink = self.base / "out-link.tgz"
        symlink.symlink_to(self.base / "missing-target")
        with self.assertRaises(PackageError):
            self.package(symlink)
        symlink.unlink()
        import unittest.mock
        real_link = os.link
        def race_link(src: str, dst: str, **kwargs: object) -> None:
            Path(dst).write_bytes(b"racer")
            real_link(src, dst, **kwargs)
        with unittest.mock.patch("os.link", side_effect=race_link):
            with self.assertRaises(PackageError):
                self.package()
        self.assertEqual(b"racer", self.output.read_bytes())

    def test_source_mutation_during_capture_is_detected(self) -> None:
        import unittest.mock
        real_read = os.read
        fired = False
        def changing_read(fd: int, count: int) -> bytes:
            nonlocal fired
            data = real_read(fd, count)
            if data and not fired:
                fired = True
                (self.root / "bin" / "demo").write_bytes(b"binary-two")
            return data
        with unittest.mock.patch("os.read", side_effect=changing_read):
            with self.assertRaises(PackageError):
                self.package()
        self.assertFalse(self.output.exists())

    def test_bounded_json_and_hardlink_rejected(self) -> None:
        self.inputs.write_bytes(b" " * (MAX_INPUT_JSON + 1))
        with self.assertRaises(PackageError):
            self.package()
        self.inputs.unlink()
        self.inputs.symlink_to(self.base / "missing-input-json")
        with self.assertRaises(PackageError):
            self.package()
        self.inputs.unlink()
        os.link(self.root / "bin" / "demo", self.root / "bin" / "hardlink")
        self.write_inputs([{"source": "bin/demo", "archive_path": "bin/demo", "mode": 420}])
        with self.assertRaises(PackageError):
            self.package()


def _self_test() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(PackageTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() and result.testsRun > 0 else 1


if __name__ == "__main__":
    raise SystemExit(_cli())
