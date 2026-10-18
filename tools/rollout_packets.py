#!/usr/bin/env python3
"""Prepare bounded Admiralty implementation missions; never dispatch a process.

Only a caller-selected, owned directory outside the repository is writable.
The dashboard consumes rollout.json. Candidate receipts are observations, not
verification or integration authority. Python 3.10+, standard library only.
"""
from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import fnmatch
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile
import uuid


PLAN_DIR = Path("docs/plans/admiralty-v2")
PACKET_ID = re.compile(r"P(?:[0-2][0-9]|3[01])\Z")
SHA = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
IDENTITY = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,79}\Z")
STATUSES = {"planned", "ready", "claimed", "in_progress", "blocked", "review", "verified", "integrated"}
ACTIVE = {"claimed", "in_progress", "blocked", "review", "verified"}
MAX_JSON = 2 * 1024 * 1024
OWNER_FILE = ".admiralty-rollout-owner.json"

# Explicit, reviewable write permissions, never inferred from prose at runtime.
# ** means descendants of this precise directory. Integration-owned shared files
# remain excluded for non-integration lanes even when a packet mentions them.
ALLOWLISTS = {
    "P00": ("tests/fixtures/legacy/**", "docs/architecture/legacy-metric-dictionary.md", "docs/operators/legacy-inventory.md"),
    "P01": ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".gitignore", ".github/workflows/ci.yml", "crates/admiralty-{core,store,runtime,server,cli,testkit}/Cargo.toml", "crates/admiralty-{core,store,runtime,testkit}/src/lib.rs", "crates/admiralty-{server,cli}/src/main.rs", "crates/admiralty-runtime/src/bin/admiralty-{session,node}.rs", "crates/admiralty-testkit/src/**", "scripts/{dev,check}/**"),
    "P02": ("crates/admiralty-core/src/{ids,contract,state,events,policy,reducer}.rs", "protocol/**", "schemas/**", "packages/contracts/**", "configs/{defaults,profiles,examples}/**", "docs/protocol/**"),
    "P03": ("crates/admiralty-store/src/{writer,inbox,outbox,projections,backup}.rs", "crates/admiralty-store/migrations/[0-9][0-9][0-9][0-9]_*.sql", "tests/integration/{store_replay,store_crash}.rs"),
    "P04": ("crates/admiralty-store/src/artifacts.rs", "tests/integration/artifact_custody.rs", "docs/architecture/artifact-lifecycle.md"),
    "P05": ("crates/admiralty-core/src/coordination/{message,mailbox,topics,decisions}.rs", "crates/admiralty-server/src/services/coordination.rs", "tests/integration/coordination_delivery.rs"),
    "P06": ("crates/admiralty-core/src/coordination/{context,render,tool_results,tried,governor}.rs", "tests/fixtures/context/**", "tests/integration/context_fidelity.rs"),
    "P07": ("crates/admiralty-runtime/src/session/**", "crates/admiralty-runtime/src/bin/admiralty-session.rs", "tests/integration/{session_continuity,input_uncertainty}.rs"),
    "P08": ("crates/admiralty-runtime/src/providers/{mod,fake,codex,capabilities,detectors}.rs", "configs/profiles/providers/**", "tests/fixtures/providers/codex/**", "tests/integration/provider_codex.rs"),
    "P09": ("crates/admiralty-runtime/src/workspace/{registry,git,runner,resources}.rs", "packaging/containers/**", "tests/integration/runner_isolation.rs"),
    "P10": ("crates/admiralty-core/src/economics/**", "crates/admiralty-server/src/services/metrics.rs", "tests/integration/economic_conservation.rs", "docs/architecture/metrics.md"),
    "P11": ("crates/admiralty-server/src/api/**", "crates/admiralty-server/src/main.rs", "crates/admiralty-cli/src/{main,doctor,init,export}.rs", "tests/integration/api_stream.rs"),
    "P12": ("apps/dashboard/{index.html,vite.config.ts,tsconfig.json,tsconfig.app.json,tsconfig.node.json,eslint.config.js}", "apps/dashboard/src/{main.tsx,vite-env.d.ts}", "apps/dashboard/src/{app,api,components,styles}/**", "apps/dashboard/src/features/overview/**"),
    "P13": ("crates/admiralty-server/src/services/legacy.rs", "crates/admiralty-core/src/plans/{import,lint,revision}.rs", "configs/profiles/legacy/**", "crates/admiralty-cli/src/{project,work}.rs", "apps/dashboard/src/features/{projects,workplans}/**", "tests/integration/plan_import.rs"),
    "P14": ("crates/admiralty-core/src/scheduling/**", "crates/admiralty-server/src/services/coordinator.rs", "tests/integration/scheduler_admission.rs"),
    "P15": ("crates/admiralty-runtime/src/verification/{candidate,oracle,receipts,checks}.rs", "tests/integration/exact_candidate.rs"),
    "P16": ("crates/admiralty-runtime/src/verification/jankurai.rs", "configs/profiles/quality/**", "tests/fixtures/jankurai/**", "tests/integration/jankurai_staged.rs", "docs/contributors/quality.md"),
    "P17": ("crates/admiralty-core/src/coordination/{claims,interfaces}.rs", "crates/admiralty-runtime/src/agent_tools/**", "apps/dashboard/src/features/{coordination,agents}/**", "tests/integration/peer_handoff.rs"),
    "P18": ("crates/admiralty-runtime/src/integration/{conflict,train}.rs", "tests/integration/integration_train.rs"),
    "P19": ("crates/admiralty-runtime/src/integration/{publisher,reconcile}.rs", "tests/integration/publication_gate.rs", "docs/operators/publication.md"),
    "P20": ("crates/admiralty-runtime/src/workspace/cleanup.rs", "tests/integration/cleanup_custody.rs", "docs/operators/cleanup.md"),
    "P21": ("crates/admiralty-runtime/src/providers/claude.rs", "tests/fixtures/providers/claude/**", "tests/integration/provider_claude.rs"),
    "P22": ("crates/admiralty-runtime/src/providers/{agy,grok,process}.rs", "tests/fixtures/providers/{agy,grok,process}/**", "tests/integration/provider_matrix.rs"),
    "P23": ("crates/admiralty-runtime/src/fleet/{enrollment,transport,transfer}.rs", "crates/admiralty-runtime/src/bin/admiralty-node.rs", "crates/admiralty-cli/src/node.rs", "tests/integration/fleet_transport.rs"),
    "P24": ("crates/admiralty-runtime/src/fleet/{placement,partition}.rs", "tests/integration/fleet_partition.rs", "docs/operators/fleet.md"),
    "P25": ("crates/admiralty-runtime/src/operations/**", "packaging/{systemd,launchd}/**", "tests/integration/{upgrade_continuity,disaster_restore}.rs", "docs/operators/{upgrades,recovery}.md"),
    "P26": ("apps/dashboard/src/features/economics/**", "apps/dashboard/e2e/economics.spec.ts"),
    "P27": ("crates/admiralty-server/src/services/improvement/{excursions,proposals}.rs", "apps/dashboard/src/features/incidents/**", "tests/integration/excursion_lineage.rs"),
    "P28": ("crates/admiralty-server/src/services/improvement/{experiments,datasets}.rs", "crates/admiralty-core/src/evaluation/**", "apps/dashboard/src/features/experiments/**", "tests/integration/experiment_validity.rs", "docs/architecture/improvement.md"),
    "P29": ("apps/dashboard/src/features/{integration,quality,settings,assets}/**", "apps/dashboard/e2e/{integration,quality,assets,settings}.spec.ts"),
    "P30": ("scripts/{package,release}/**", "crates/admiralty-cli/src/demo.rs", "docs/{operators,contributors,brand}/**", "apps/dashboard/public/brand/**", "README.md", "CONTRIBUTING.md", "SECURITY.md", "CODE_OF_CONDUCT.md", "LICENSE"),
    "P31": ("tests/qualification/**", "benchmarks/**", "docs/operators/deployment.md", "docs/plans/admiralty-v2/receipts/**", "docs/plans/admiralty-v2/EXECUTION.md"),
}

# Fail closed when a plan's scope changes: reviewers must reconcile the explicit
# paths above, rather than silently inheriting expanded permissions.
SCOPE_DIGESTS = dict(zip(ALLOWLISTS, """
aae3a40c341d0bd3fc51e17fbacc7f21355afa87449d822f1e79e081c56dc0d9
bc1cf4ca6bcfd6ba1c07782e04246055f5aef6731e54ecb2ce7f60376964e3d3
3b6ba27498c106ae53aa43857855c158348df471abc44a96a002ed3ed972477d
778d87f6bff9569b9863b3e905ecdd404f322364772cf468d4a568362b5a6680
704c92229a10d964c390cf7d1083ba496d55f72c9eb97ab80b106eb3d94a1405
72a01972e4796ba33089ddebc4e2e42c1587a92f22fffa2ef58cc1357f0dc8b8
f6d0c46e76a154434aa2f72539a475144dd04978e632486b6783e4faa98fff2f
1562c9189cc8c9dd7987f70e4cc2f74ca6ac0753d259cf4399cbd613a4d66aa5
71daa1b38972f79637026cb681284e8ac3660d10acacf95b3d80b7f85471f0e3
91abf0dc73339c5406ffedabd8af089d9aeab294c04cdb8f869d3fc0ec488f1b
2774895bfc79de55a35d7fb66a3b3ea80c842fe7b8d35f559a25d6485575a6ec
e2a2b619612da3bee6789fbf437bc2e2e801ea4a9c526a3bf7cc412c3156f595
cbefb4d5f4ad5c0fe42a94c763611b2d769eff82bf6d4374b5e16a010ac93c8f
9de44cc6947a9a97be3dbf8132742e10d7053009d961a37d5fbe196fe8453f62
e305aa92196c0ba19c2bb8ec1d0b6de34c62e94619daadc6b63b834293dd8320
ee685a0eb679c818e675a8ccb9e310261a5005e09d09750f729907388bb9be4b
f53c99b097003f6aeb1f29f00fc164665092043db10707214da3a38981c39ac1
14ce50be2eb3e1e7710bb5519822ab8096b5b8b2d56ab230589225ec2f06556d
a2905ef20cfc6c9f7cc0ed4595da25dab1004834efce0b02aa4465de122ce596
444634495645aa9eb1ce30c1d4462939b24dd98a59e4a6a0346b9312a0ee366c
132f6595307b03dcbca5e4c7087d83b9c24b76fddbf79fec37727c764d6e26cb
7566f8a1ae301a8de0a5869df3409b91468e2205e059455fca27478257863926
1f5a7a31a77fe4f5976a286a96bb7fa1ac6f7c938e0f29c42daf105c76277755
6860313971428bf6f3b3e1c4b09b3a4e47afad843f2348bc045dc9e2d953d475
e5c477149f088798e79c0c897560191d0ddb493279c5aad457b70615cdb37474
aeb95a79aead10669c077b1ce490604b173296c1a0b384cb34379519d50c9604
fc7c2716d9888513ab70f8c23d37036627b8419cbcfb38e278aac9488764b7c7
883e8e5115af74eb3d74564727ab771afd9ea169c7546f96bc4db1fdfdfa1b4c
fae2123060d60a6a61019cac01eebed273e35332c402650ca5ff88075a1d3813
3c1a0e9dbf3e6be243e8522b94c285f5b5adaee377f566263e3b60b223d53a78
ef0b540d8cf819477abf51047a4309a8781afa94376bc169bc31e12eeaa240d3
693897c0e7d7fd269562628586e4b4b47cb7cd2d7a18728fb2bbc69ee4bf66f2
""".split()))


class Invalid(ValueError):
    """Invalid input; no external effect is authorized."""


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")


def require(condition, message):
    if not condition:
        raise Invalid(message)


def expand_braces(pattern: str) -> list[str]:
    match = re.search(r"\{([^{}]+)\}", pattern)
    if not match:
        return [pattern]
    return [expanded for part in match[1].split(",")
            for expanded in expand_braces(pattern[:match.start()] + part + pattern[match.end():])]


def safe_path(value: str, *, pattern=False) -> str:
    require(isinstance(value, str) and 0 < len(value) <= 500, "missing or oversized path")
    require("\\" not in value and not any(ord(c) < 32 for c in value), "unsafe path")
    path = PurePosixPath(value)
    require(not path.is_absolute() and all(part not in ("", ".", "..") for part in value.split("/")), "path must be a relative non-traversing path")
    require(path.parts[0] != ".git", "Git metadata is forbidden")
    if not pattern:
        require(not any(c in value for c in "*?[]{}"), "concrete path required")
    return value


def allowlist(packet_id: str) -> list[str]:
    require(packet_id in ALLOWLISTS, "unknown packet ID")
    return sorted({safe_path(p, pattern=True) for raw in ALLOWLISTS[packet_id] for p in expand_braces(raw)})


def allows(path: str, patterns: list[str]) -> bool:
    safe_path(path)
    for pattern in patterns:
        if pattern.endswith("/**") and path.startswith(pattern[:-2]):
            return True
        # fnmatch's '*' crosses slashes: forbid that for exact file patterns.
        if len(path.split("/")) == len(pattern.split("/")) and fnmatch.fnmatchcase(path, pattern):
            return True
    return False


def required_field(body: str, field: str) -> str:
    match = re.search(r"\*\*" + re.escape(field) + r":\*\*\s*(.+?)(?=\s*\*\*|\n\n|\Z)", body, re.S)
    require(match is not None and match[1].strip(), f"missing {field}")
    return match[1].strip()


def dependencies(raw: str) -> list[str]:
    raw = raw.rstrip(".")
    if raw == "none":
        return []
    if re.fullmatch(r"P\d\d through P\d\d", raw):
        lo, hi = re.findall(r"P(\d\d)", raw)
        require(int(lo) <= int(hi), "backwards dependency range")
        result = [f"P{i:02}" for i in range(int(lo), int(hi) + 1)]
    else:
        # The semicolon describes a conditional later-stage dependency. Keep it
        # as a full dependency here: conservative full-packet readiness.
        result = re.findall(r"P\d\d", raw)
    require(result and len(result) == len(set(result)), "missing or duplicate dependencies")
    require(all(PACKET_ID.fullmatch(p) for p in result), "unsafe dependency ID")
    return result


def validate_graph(packets: list[dict]) -> None:
    index = {}
    for packet in packets:
        pid = packet.get("id")
        require(isinstance(pid, str) and PACKET_ID.fullmatch(pid), "unsafe packet ID")
        require(pid not in index, "duplicate packet ID")
        index[pid] = packet
    require(set(index) == set(ALLOWLISTS), "expected exactly P00 through P31")
    visiting, visited = set(), set()

    def visit(pid):
        require(pid not in visiting, f"dependency cycle at {pid}")
        if pid in visited:
            return
        visiting.add(pid)
        deps = index[pid].get("depends_on")
        require(isinstance(deps, list) and len(deps) == len(set(deps)), f"invalid dependencies for {pid}")
        for dep in deps:
            require(dep in index, f"unknown dependency {dep}")
            visit(dep)
        visiting.remove(pid)
        visited.add(pid)

    for pid in index:
        visit(pid)


def parse_plan(text: str) -> list[dict]:
    matches = list(re.finditer(r"^### (\S+) — ([^\n]+)\n", text, re.M))
    require(len(matches) == 32, "expected 32 packet headings")
    packets = []
    for number, match in enumerate(matches):
        pid, title = match[1], match[2]
        require(PACKET_ID.fullmatch(pid), "unsafe packet ID")
        end = matches[number + 1].start() if number + 1 < len(matches) else len(text)
        body = text[match.end():end].split("\n## ", 1)[0].strip()
        scope = required_field(body, "Write scope")
        require(digest(scope.encode()) == SCOPE_DIGESTS[pid], f"{pid} scope changed; review explicit allowlist")
        steps = re.findall(r"^\d+\. .+(?:\n(?!\n|\d+\. |\*\*).+)*", body, re.M)
        require(steps, f"{pid} has no implementation steps")
        packets.append({
            "id": pid, "title": title,
            "depends_on": dependencies(required_field(body, "Depends on")),
            "owner": required_field(body, "Owner").rstrip("."),
            "release": required_field(body, "Release").rstrip("."),
            "read_first": required_field(body, "Read first"),
            "source_write_scope": scope, "write_allowlist": allowlist(pid),
            "steps": steps, "validation": required_field(body, "Validate"),
            "acceptance": required_field(body, "Pass"),
            "stop": required_field(body, "Stop") if "**Stop:**" in body else None,
            "source_sha256": digest(body.encode()),
        })
    validate_graph(packets)
    return packets


def capsule(packet: dict, repo: Path, base: str) -> str:
    paths = "\n".join(f"- `{path}`" for path in packet["write_allowlist"])
    deps = ", ".join(packet["depends_on"]) or "none"
    return f"""# {packet['id']} — {packet['title']}

Objective: implement this packet's complete acceptance contract. Owner: {packet['owner']}. Release: {packet['release']}.
Repository: `{repo}`. Pinned base: `{base}`. Dependencies: {deps}.
This is a prepared mission, not a claim or authorization to take over an existing session.

Before starting: obtain an explicit claim; read current local instructions and integrated dependency receipts; verify actual base and a clean, privately owned worktree. Stop on missing dependency proof, stale base, conflicting ownership, essential unknowns, scope expansion, or unsafe effects. Do not load all proposals or other projects' prompts.

Read first, in order: {packet['read_first']}
Shared reference: `docs/plans/admiralty-v2/MASTER_PLAN.md` §§1.4, 3.1, 12–14 and the sections named above. Read only relevant sections.

Exact write allowlist (`/**` means descendants; other patterns match one path component):
{paths}

Original scope restrictions: {packet['source_write_scope']}
Allowlist is an upper bound. Shared manifests, lockfiles, module wiring, migration numbering, generated contracts and CI require the integration owner. Submit exact requested edits when not expressly owned. Generated artifacts require their generator; no handwritten output. Preserve original exports, private transcripts, credentials and other agents' changes. All paths outside this list are read-only. Receipt handoff is written to the runtime output directory; integration owner files the accepted receipt in the repository.

Implementation:
{chr(10).join(packet['steps'])}

Validation: {packet['validation']}
Acceptance: {packet['acceptance']}
Packet stop condition: {packet['stop'] or 'Apply the shared stop conditions above.'}

Return one bounded candidate plus JSON receipt: schema_version=1, packet_id, owner, base_sha, head_sha, interface_version, changed_paths, criteria[{{id,status,evidence}}], commands[{{argv,cwd,exit_code}}], environment, artifacts[{{path,sha256}}], risks, blockers, next_action. Use full Git IDs and computed SHA-256 digests; retain failed commands and repairs. Report unavailable checks explicitly. Never equate a final message or exit code with verified/integrated status.

Report only changed evidence, blocker owner, exact API/schema requests and next action. Routine healthy status needs no model turn. Peer interface questions go to the scoped owner; captains receive decisions and blockers. No live provider, terminal input, remote publication, cleanup or fleet takeover is authorized by this capsule.
"""


def read_json(path: Path) -> dict:
    require(path.is_file() and not path.is_symlink(), f"missing or symlink JSON: {path}")
    require(path.stat().st_size <= MAX_JSON, "oversized JSON")
    def no_duplicates(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON field: {key}")
            result[key] = value
        return result
    try:
        value = json.loads(path.read_text(), object_pairs_hook=no_duplicates,
                           parse_constant=lambda value: (_ for _ in ()).throw(Invalid(f"non-finite JSON: {value}")))
    except (ValueError, UnicodeError) as error:
        raise Invalid(f"invalid JSON: {error}") from error
    require(isinstance(value, dict), "JSON object required")
    return value


def validate_manifest(manifest: dict) -> None:
    require(manifest.get("schema_version") == 1, "unsupported manifest schema")
    require(isinstance(manifest.get("project"), dict) and all(manifest["project"].get(k) for k in ("id", "name", "repository_url")), "missing project identity")
    require(isinstance(manifest.get("base_ref"), str) and SHA.fullmatch(manifest["base_ref"]), "full base_ref required")
    require(isinstance(manifest.get("packets"), list), "packets list required")
    require(isinstance(manifest.get("lanes"), list) and isinstance(manifest.get("pr"), dict), "lanes/pr required")
    validate_graph(manifest["packets"])
    index = {p["id"]: p for p in manifest["packets"]}
    for packet in manifest["packets"]:
        status = packet.get("status")
        require(status in STATUSES, "unknown packet status")
        require(packet.get("write_allowlist") == allowlist(packet["id"]), "write allowlist drift")
        ready = all(index[d]["status"] == "integrated" for d in packet["depends_on"])
        if status not in {"planned", "blocked"}:
            require(ready, f"{packet['id']} dependencies are not integrated")
        if status in ACTIVE:
            claim = packet.get("claim")
            require(isinstance(claim, dict) and claim.get("owner") and claim.get("lane"), "active packet requires explicit claim")
            require(packet.get("lane") == claim["lane"], "claim lane mismatch")
        if status in {"verified", "integrated"}:
            proof = packet.get("verification")
            require(isinstance(proof, dict) and all(proof.get(k) for k in ("receipt", "revision", "verifier", "observed_utc")), "verified requires independent provenance")
            require(SHA.fullmatch(proof["revision"]), "verification revision must be full Git ID")
        if status == "integrated":
            proof = packet.get("integration")
            require(isinstance(proof, dict) and all(proof.get(k) for k in ("receipt", "revision", "target_ref", "observed_utc")), "integrated requires integration provenance")
            require(SHA.fullmatch(proof["revision"]), "integration revision must be full Git ID")
        require(isinstance(packet.get("evidence"), list), "evidence list required")
        safe_path(packet.get("capsule_path"))
        require(isinstance(packet.get("capsule_sha256"), str) and DIGEST.fullmatch(packet["capsule_sha256"]), "capsule digest required")


def verify_capsules(output: Path, manifest: dict) -> None:
    for packet in manifest["packets"]:
        path = output / safe_path(packet["capsule_path"])
        require(path.resolve().is_relative_to(output) and not path.is_symlink(), "unsafe capsule path")
        require(path.is_file() and digest(path.read_bytes()) == packet["capsule_sha256"], "capsule digest mismatch")


def atomic_write(path: Path, data: bytes) -> None:
    fd, temporary = tempfile.mkstemp(prefix=".write-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def json_bytes(value: dict) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False, allow_nan=False) + "\n").encode()


def owned_output(raw: Path, repo: Path, *, create=False) -> Path:
    output = raw.expanduser().absolute()
    # macOS exposes its OS-owned temporary roots through these standard aliases.
    # User-created aliases, including the output directory itself, remain denied.
    system_aliases = {Path("/tmp"): Path("/private/tmp"), Path("/var"): Path("/private/var")}
    for part in [output, *output.parents]:
        if part.is_symlink():
            require(part in system_aliases and part.resolve() == system_aliases[part]
                    and part.lstat().st_uid == 0, "output symlinks are forbidden")
    output = output.resolve()
    require(output != repo and repo not in output.parents, "output must be outside repository")
    require(output != Path(output.anchor) and output != Path.home(), "choose a dedicated output directory")
    marker = output / OWNER_FILE
    if not output.exists():
        require(create, "output has not been initialized")
        output.mkdir(parents=True, mode=0o700)
    require(output.is_dir() and output.stat().st_uid == os.getuid(), "output is not an owned directory")
    if not marker.exists():
        require(create and not list(output.iterdir()), "output must be empty or initialized by this tool")
        atomic_write(marker, json_bytes({"schema_version": 1, "repo": str(repo), "uid": os.getuid()}))
    require(read_json(marker) == {"schema_version": 1, "repo": str(repo), "uid": os.getuid()}, "output ownership marker mismatch")
    for child in (output / "rollout.json", output / "generations", output / "receipts", output / ".lock"):
        require(not child.is_symlink(), "output contains unsafe symlink")
    return output


@contextlib.contextmanager
def output_lock(output: Path):
    import fcntl
    with (output / ".lock").open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def resolve_base(repo: Path, ref: str) -> str:
    require(ref and not ref.startswith("-") and "\n" not in ref, "unsafe base reference")
    result = subprocess.run(["git", "-C", str(repo), "rev-parse", "--verify", ref + "^{commit}"], capture_output=True, text=True, timeout=10, check=False)
    require(result.returncode == 0 and SHA.fullmatch(result.stdout.strip()), "base reference does not resolve to a commit")
    return result.stdout.strip()


def export(repo: Path, output: Path, base: str, *, project_id="admiralty-v2", repository_url="https://github.com/neverhuman/admiralty") -> dict:
    require(IDENTITY.fullmatch(project_id), "unsafe project ID")
    work_path, master_path = repo / PLAN_DIR / "WORK_PACKETS.md", repo / PLAN_DIR / "MASTER_PLAN.md"
    work, master = work_path.read_bytes(), master_path.read_bytes()
    packets = parse_plan(work.decode())
    generation = uuid.uuid4().hex
    manifest = {"schema_version": 1, "producer": "admiralty.rollout_packets/v1", "updated_utc": now(),
                "project": {"id": project_id, "name": "Admiralty v2 implementation", "repository_url": repository_url},
                "base_ref": base, "source": {"work_packets_sha256": digest(work), "master_plan_sha256": digest(master)},
                "packets": packets, "lanes": [], "pr": {"url": None, "head": base}, "generation": generation}
    previous = read_json(output / "rollout.json") if (output / "rollout.json").exists() else None
    if previous:
        validate_manifest(previous)
        require(previous["source"] == manifest["source"] and previous["base_ref"] == base and previous["project"] == manifest["project"], "source/base/project changed; reconcile in a fresh owned output directory")
        manifest["lanes"], manifest["pr"] = previous["lanes"], previous["pr"]
    old = {p["id"]: p for p in previous["packets"]} if previous else {}
    for packet in packets:
        packet.update({"status": "ready" if not packet["depends_on"] else "planned", "lane": None, "claim": None, "verification": None, "integration": None, "evidence": []})
        if packet["id"] in old:
            packet.update({key: old[packet["id"]][key] for key in ("status", "lane", "claim", "verification", "integration", "evidence")})
        rendered = capsule(packet, repo, base).encode()
        packet["capsule_path"] = f"generations/{generation}/capsules/{packet['id']}.md"
        packet["capsule_sha256"] = digest(rendered)
    index = {p["id"]: p for p in packets}
    for packet in packets:
        if packet["status"] in {"planned", "ready"}:
            packet["status"] = "ready" if all(index[d]["status"] == "integrated" for d in packet["depends_on"]) else "planned"
    validate_manifest(manifest)
    generations = output / "generations"
    generations.mkdir(exist_ok=True)
    staged = Path(tempfile.mkdtemp(prefix=".generation-", dir=generations))
    try:
        (staged / "capsules").mkdir()
        for packet in packets:
            atomic_write(staged / "capsules" / (packet["id"] + ".md"), capsule(packet, repo, base).encode())
        atomic_write(staged / "mission.json", json_bytes(manifest))
        os.rename(staged, generations / generation)
        descriptor = os.open(generations, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        atomic_write(output / "rollout.json", json_bytes(manifest))
    finally:
        if staged.exists():
            shutil.rmtree(staged)
    return manifest


def claim(manifest: dict, pid: str, owner: str, lane: str, node: str | None, branch: str) -> None:
    require(pid in ALLOWLISTS, "unknown packet ID")
    require(IDENTITY.fullmatch(owner) and IDENTITY.fullmatch(lane), "unsafe owner or lane identity")
    require(node is None or IDENTITY.fullmatch(node), "unsafe node identity")
    require(re.fullmatch(r"codex/[A-Za-z0-9][A-Za-z0-9_./-]{0,150}", branch) and ".." not in branch and "//" not in branch and not branch.endswith(("/", ".")), "invalid codex branch")
    packet = next(p for p in manifest["packets"] if p["id"] == pid)
    require(packet["status"] == "ready", "only ready packets can be claimed")
    require(not any(p["lane"] == lane and p["status"] in ACTIVE for p in manifest["packets"]), "lane already has an active packet")
    # Readiness comes from integrated dependency evidence, not worker reports.
    packet.update({"status": "claimed", "lane": lane, "claim": {"owner": owner, "lane": lane, "node": node, "branch": branch, "observed_utc": now()}})
    manifest["lanes"].append({"id": lane, "owner": owner, "node": node, "branch": branch, "status": "claimed"})
    manifest["updated_utc"] = now()
    validate_manifest(manifest)


def validate_receipt(receipt: dict, manifest: dict) -> dict:
    required = {"schema_version", "packet_id", "owner", "base_sha", "head_sha", "interface_version", "changed_paths", "criteria", "commands", "environment", "artifacts", "risks", "blockers", "next_action"}
    require(required <= receipt.keys(), "candidate receipt missing required fields: " + ", ".join(sorted(required - receipt.keys())))
    require(receipt["schema_version"] == 1, "unsupported receipt schema")
    pid = receipt["packet_id"]
    require(isinstance(pid, str) and PACKET_ID.fullmatch(pid), "unsafe receipt packet ID")
    packet = next(p for p in manifest["packets"] if p["id"] == pid)
    require(packet["status"] in {"claimed", "in_progress", "blocked", "review"}, "receipt requires an active explicit claim")
    require(receipt["owner"] == packet["claim"]["owner"], "receipt owner differs from claim")
    require(receipt["base_sha"] == manifest["base_ref"], "receipt base differs from pinned base")
    require(isinstance(receipt["head_sha"], str) and SHA.fullmatch(receipt["head_sha"]), "full candidate head required")
    require(isinstance(receipt["interface_version"], str) and receipt["interface_version"], "interface version required (use explicit unimplemented if unavailable)")
    paths = receipt["changed_paths"]
    require(isinstance(paths, list) and paths and len(paths) == len(set(paths)), "nonempty unique changed_paths required")
    require(all(allows(path, packet["write_allowlist"]) for path in paths), "candidate changes files outside packet scope")
    require(isinstance(receipt["criteria"], list) and receipt["criteria"], "criteria required")
    for criterion in receipt["criteria"]:
        require(isinstance(criterion, dict) and criterion.get("id") and criterion.get("status") in {"passed", "failed", "unavailable"} and criterion.get("evidence"), "invalid criterion")
    require(isinstance(receipt["commands"], list) and receipt["commands"], "command records required")
    for command in receipt["commands"]:
        require(isinstance(command, dict) and isinstance(command.get("argv"), list) and command["argv"] and all(isinstance(a, str) and a for a in command["argv"]), "command argv required")
        require(isinstance(command.get("cwd"), str) and command["cwd"] and type(command.get("exit_code")) is int, "command cwd/exit_code required")
    require(isinstance(receipt["environment"], dict) and receipt["environment"], "environment identity required")
    require(isinstance(receipt["artifacts"], list), "artifact list required")
    for artifact in receipt["artifacts"]:
        require(isinstance(artifact, dict) and artifact.get("path") and isinstance(artifact.get("sha256"), str) and DIGEST.fullmatch(artifact["sha256"]), "artifact path/computed SHA-256 required")
    require(isinstance(receipt["risks"], list) and isinstance(receipt["blockers"], list) and isinstance(receipt["next_action"], str) and receipt["next_action"], "risks/blockers/next_action required")
    return packet


def reconcile(output: Path, manifest: dict, receipt: dict) -> None:
    packet = validate_receipt(receipt, manifest)
    data = json_bytes(receipt)
    hashed = digest(data)
    directory = output / "receipts"
    directory.mkdir(exist_ok=True)
    path = directory / (hashed + ".json")
    require(not path.is_symlink(), "receipt output symlink forbidden")
    if path.exists():
        require(path.read_bytes() == data, "receipt digest collision")
    else:
        atomic_write(path, data)
    evidence = {"kind": "candidate_receipt", "path": f"receipts/{hashed}.json", "sha256": hashed, "revision": receipt["head_sha"], "observed_utc": now(), "trust": "worker_report_unverified"}
    if not any(item.get("sha256") == hashed for item in packet["evidence"]):
        packet["evidence"].append(evidence)
    packet["status"] = "review"
    for lane in manifest["lanes"]:
        if lane["id"] == packet["lane"]:
            lane["status"] = "review"
    manifest["updated_utc"] = now()
    validate_manifest(manifest)
    atomic_write(output / "rollout.json", json_bytes(manifest))


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("export", "check", "claim", "reconcile"):
        command = commands.add_parser(name)
        command.add_argument("--out", type=Path, required=True, help="dedicated owned directory outside repository")
        if name == "export":
            command.add_argument("--base-ref", required=True)
            command.add_argument("--project-id", default="admiralty-v2")
            command.add_argument("--repository-url", default="https://github.com/neverhuman/admiralty")
        if name == "claim":
            command.add_argument("--packet", required=True)
            command.add_argument("--owner", required=True)
            command.add_argument("--lane", required=True)
            command.add_argument("--node")
            command.add_argument("--branch", required=True)
        if name == "reconcile":
            command.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        repo = args.repo.expanduser().resolve()
        base = resolve_base(repo, args.base_ref) if args.command == "export" else None
        output = owned_output(args.out, repo, create=args.command == "export")
        with output_lock(output):
            if args.command == "export":
                manifest = export(repo, output, base, project_id=args.project_id, repository_url=args.repository_url)
            else:
                manifest = read_json(output / "rollout.json")
                validate_manifest(manifest)
                verify_capsules(output, manifest)
                if args.command == "claim":
                    claim(manifest, args.packet, args.owner, args.lane, args.node, args.branch)
                    atomic_write(output / "rollout.json", json_bytes(manifest))
                elif args.command == "reconcile":
                    reconcile(output, manifest, read_json(args.receipt))
            verify_capsules(output, manifest)
        print(json.dumps({"manifest": str(output / "rollout.json"), "packets": len(manifest["packets"]), "ready": [p["id"] for p in manifest["packets"] if p["status"] == "ready"], "command": args.command}))
        return 0
    except (Invalid, OSError, KeyError, TypeError, subprocess.SubprocessError, UnicodeError) as error:
        print(f"rollout_packets: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
