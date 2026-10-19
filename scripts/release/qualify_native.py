#!/usr/bin/env python3
"""Qualify a private native archive with real CLI and dashboard payloads offline."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        os.chmod(path, 0o600)
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


def regular(path):
    # No symlink aliases, including ancestors, become payload discovery roots.
    for ancestor in [path, *path.parents]:
        if ancestor.is_symlink():
            raise ValueError(f"symlink input: {ancestor}")
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f"nonregular input: {path}")
    return path


def qualify(binary, assets, output, source_commit, target):
    repo = Path(__file__).absolute().parents[2]
    binary = regular(binary.absolute())
    assets = assets.absolute()
    regular(assets / "index.html")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", source_commit):
        raise ValueError("source commit must be a complete lowercase Git ID")
    resolved = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "--verify", source_commit + "^{commit}"],
        check=True, capture_output=True, text=True, timeout=10,
    ).stdout.strip()
    if resolved != source_commit:
        raise ValueError("source commit must identify an existing local commit")
    # Keep this evidence directory. Never clear or reuse a previous result.
    output = output.absolute()
    for ancestor in [output.parent, *output.parent.parents]:
        if ancestor.is_symlink():
            raise ValueError(f"symlink output ancestor: {ancestor}")
        metadata = ancestor.stat()
        if (not stat.S_ISDIR(metadata.st_mode)
                or metadata.st_uid not in (0, os.geteuid())
                or (metadata.st_mode & 0o022 and not metadata.st_mode & stat.S_ISVTX)):
            raise ValueError(f"unprotected output ancestor: {ancestor}")
    output.mkdir(mode=0o700)
    inputs = output / "inputs"
    inputs.mkdir(mode=0o700)
    scratch = output / "scratch"
    scratch.mkdir(mode=0o700)
    empty_path = output / "empty-path"
    empty_path.mkdir(mode=0o700)
    # No account credentials, provider settings or tool executables are inherited.
    environment = {"PATH": str(empty_path), "TMPDIR": str(scratch), "LANG": "C"}
    commands = []

    def invoke(program, args, label, succeeds=True):
        stdout_path = output / f"{label}.stdout"
        stderr_path = output / f"{label}.stderr"
        with stdout_path.open("xb") as stdout, stderr_path.open("xb") as stderr:
            result = subprocess.run(
                [str(program), *map(str, args)], cwd=output, env=environment,
                stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, timeout=60,
                check=False,
            )
        commands.append({"label": label, "exit_code": result.returncode,
                         "stdout_sha256": sha256(stdout_path), "stderr_sha256": sha256(stderr_path)})
        if (result.returncode == 0) != succeeds:
            raise ValueError(f"{label}: unexpected exit {result.returncode}; inspect retained captures")
        if stdout_path.stat().st_size > 4 * 1024 * 1024 or stderr_path.stat().st_size > 4 * 1024 * 1024:
            raise ValueError(f"{label}: capture exceeds 4 MiB")
        return json.loads(stdout_path.read_text()) if succeeds else None

    allowlist = []

    def add(source, relative, mode=0o644):
        regular(source)
        destination = inputs / relative
        destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with source.open("rb") as src, destination.open("xb") as dst:
            shutil.copyfileobj(src, dst)
        os.chmod(destination, mode)
        allowlist.append({"source": relative, "expected_sha256": sha256(destination), "mode": mode})

    add(binary, "target/release/admiralty", 0o755)
    for asset in sorted(assets.rglob("*")):
        if asset.is_symlink():
            raise ValueError(f"symlink asset: {asset}")
        if asset.is_file():
            add(asset, "apps/dashboard/dist/" + asset.relative_to(assets).as_posix())
    add(repo / "schemas/domain-event-v2.schema.json", "schemas/domain-event-v2.schema.json")
    for guard in sorted((repo / "packages/contracts/generated").glob("validators*.mjs")):
        add(guard, "packages/contracts/generated/" + guard.name)
    for name in ["native-install.md", "recovery.md", "upgrades.md", "release-status.md"]:
        add(repo / "docs/operators" / name, "docs/operators/" + name)
    for name in ["LICENSE", "CONTRIBUTING.md", "SECURITY.md"]:
        add(repo / name, "docs/" + name)
    allowlist_file = output / "allowlist.json"
    write_json(allowlist_file, allowlist)
    stage = output / "stage"
    staged = invoke(binary, ["release", "stage", "--input-root", inputs, "--output", stage,
                             "--allowlist", allowlist_file, "--version", "2.0.0-private-qualification",
                             "--target", target, "--source-commit", source_commit], "stage")
    expected = output / "expected-manifest.json"
    write_json(expected, staged["receipt"]["manifest"])
    manifest_hash = sha256(stage / "MANIFEST.json")
    archive_hashes = []
    for number in range(2):
        archive = output / f"native-{number}.tar.gz"
        archived = invoke(binary, ["release", "archive", "--staged-dir", stage, "--manifest", expected,
                                   "--expected-manifest-sha256", manifest_hash, "--output", archive],
                          f"archive-{number}")
        actual_hash = sha256(archive)
        if actual_hash != archived["receipt"]["archive_sha256"]:
            raise ValueError("archive receipt differs from actual bytes")
        archive_hashes.append(actual_hash)
    if archive_hashes[0] != archive_hashes[1]:
        raise ValueError("repeated archive build differs")
    destination = output / "installed"
    install_args = ["release", "install", "--archive", output / "native-0.tar.gz",
                    "--expected-archive-sha256", archive_hashes[0], "--expected-manifest-sha256",
                    manifest_hash, "--destination", destination]
    installed = invoke(binary, install_args, "install")
    executable = destination / "bin/admiralty"
    if sha256(executable) != sha256(binary):
        raise ValueError("installed executable differs from input binary")
    # Exercise the installed executable with no tools on PATH and no account.
    doctor = invoke(executable, ["doctor"], "installed-doctor")
    demo = invoke(executable, ["demo", "--snapshot-only", "--assets",
                              destination / "share/admiralty/dashboard"], "installed-demo")
    if not doctor["capabilities"]["isolated_offline_demo"] or doctor["native_execution_qualified"]:
        raise ValueError("unexpected doctor capability boundary")
    if not demo["projection"]["tasks"]:
        raise ValueError("demo did not seed useful task state")
    if not all(task["contract"]["simulated"] for task in demo["projection"]["tasks"].values()):
        raise ValueError("demo contains a nonsimulated task")
    invoke(binary, install_args, "existing-destination-refused", succeeds=False)
    if sha256(executable) != sha256(binary):
        raise ValueError("refused reinstall changed installed binary")
    bad_args = list(install_args)
    bad_args[bad_args.index("--expected-archive-sha256") + 1] = "0" * 64
    bad_args[-1] = output / "bad-install"
    invoke(binary, bad_args, "wrong-archive-pin-refused", succeeds=False)
    if (output / "bad-install").exists():
        raise ValueError("bad archive pin published a destination")
    if list(scratch.iterdir()):
        raise ValueError("demo left temporary runtime state")
    report = {
        "schema_version": 1, "status": "passed", "scope": "private_current_host_archive_install_demo",
        "platform": platform.platform(), "source_commit_label": source_commit,
        "target_label": target, "qualification_script_sha256": sha256(Path(__file__)),
        "allowlist_sha256": sha256(allowlist_file),
        "binary_sha256": sha256(binary), "manifest_raw_sha256": manifest_hash,
        "archive_sha256": archive_hashes[0], "reproducible_archive": True,
        "installed_file_count": installed["receipt"]["installed_file_count"],
        "demo_task_count": len(demo["projection"]["tasks"]), "commands": commands,
        "account_required": False, "tools_on_path": False, "node_server_required": False,
        "source_commit_verified": False, "signed": False, "published": False,
        "service_activation_qualified": False, "full_packet_acceptance": False,
    }
    write_json(output / "qualification.json", report)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    print(json.dumps(qualify(args.binary, args.assets, args.output, args.source_commit, args.target), indent=2))
