//! Prepare bounded Admiralty implementation missions; never dispatch a process.
//!
//! Port of the retired `tools/rollout_packets.py`. Only a caller-selected,
//! owned directory outside the repository is writable. The dashboard consumes
//! `rollout.json`. Candidate receipts are observations, not verification or
//! integration authority.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::error::{ToolError, ToolResult, require};
use crate::fsx::{self, ReplaceFn};
use crate::proc;
use crate::pyjson::{self, DumpOptions, LoadError, LoadOptions, Map, NULL, Value};
use crate::pytime;

/// Plan directory relative to the repository.
pub const PLAN_DIR: &str = "docs/plans/admiralty-v2";
/// Ownership marker written into an initialized output directory.
pub const OWNER_FILE: &str = ".admiralty-rollout-owner.json";
/// Manifest producer.
pub const PRODUCER: &str = "admiralty.rollout_packets/v1";
/// JSON input bound (two MiB).
pub const MAX_JSON: u64 = 2 * 1024 * 1024;

const STATUSES: [&str; 8] = [
    "planned",
    "ready",
    "claimed",
    "in_progress",
    "blocked",
    "review",
    "verified",
    "integrated",
];
const ACTIVE: [&str; 5] = ["claimed", "in_progress", "blocked", "review", "verified"];

static PACKET_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^P(?:[0-2][0-9]|3[01])\z").expect("static packet pattern"));
static SHA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})\z").expect("static object-id pattern")
});
static DIGEST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}\z").expect("static digest pattern"));
static IDENTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}\z").expect("static identity pattern")
});
static BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^codex/[A-Za-z0-9][A-Za-z0-9_./-]{0,150}\z").expect("static branch pattern")
});
static BRACES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^{}]+)\}").expect("static brace pattern"));
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^### (\S+) — ([^\n]+)\n").expect("static heading pattern"));
static STEP_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\. ").expect("static step pattern"));
static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^P[0-9][0-9] through P[0-9][0-9]\z").expect("static range pattern")
});
static RANGE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"P([0-9][0-9])").expect("static range-number pattern"));
static DEPENDENCY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"P[0-9][0-9]").expect("static dependency pattern"));

/// Explicit, reviewable write permissions, never inferred from prose at
/// runtime. `**` means descendants of this precise directory.
/// Integration-owned shared files remain excluded for non-integration lanes
/// even when a packet mentions them.
pub const ALLOWLISTS: [(&str, &[&str]); 32] = [
    (
        "P00",
        &[
            "tests/fixtures/legacy/**",
            "docs/architecture/legacy-metric-dictionary.md",
            "docs/operators/legacy-inventory.md",
        ],
    ),
    (
        "P01",
        &[
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".gitignore",
            ".github/workflows/ci.yml",
            "crates/admiralty-{core,store,runtime,server,cli,testkit}/Cargo.toml",
            "crates/admiralty-{core,store,runtime,testkit}/src/lib.rs",
            "crates/admiralty-{server,cli}/src/main.rs",
            "crates/admiralty-runtime/src/bin/admiralty-{session,node}.rs",
            "crates/admiralty-testkit/src/**",
            "scripts/{dev,check}/**",
        ],
    ),
    (
        "P02",
        &[
            "crates/admiralty-core/src/{ids,contract,state,events,policy,reducer}.rs",
            "protocol/**",
            "schemas/**",
            "packages/contracts/**",
            "configs/{defaults,profiles,examples}/**",
            "docs/protocol/**",
        ],
    ),
    (
        "P03",
        &[
            "crates/admiralty-store/src/{writer,inbox,outbox,projections,backup}.rs",
            "crates/admiralty-store/migrations/[0-9][0-9][0-9][0-9]_*.sql",
            "tests/integration/{store_replay,store_crash}.rs",
        ],
    ),
    (
        "P04",
        &[
            "crates/admiralty-store/src/artifacts.rs",
            "tests/integration/artifact_custody.rs",
            "docs/architecture/artifact-lifecycle.md",
        ],
    ),
    (
        "P05",
        &[
            "crates/admiralty-core/src/coordination/{message,mailbox,topics,decisions}.rs",
            "crates/admiralty-server/src/services/coordination.rs",
            "tests/integration/coordination_delivery.rs",
        ],
    ),
    (
        "P06",
        &[
            "crates/admiralty-core/src/coordination/{context,render,tool_results,tried,governor}.rs",
            "tests/fixtures/context/**",
            "tests/integration/context_fidelity.rs",
        ],
    ),
    (
        "P07",
        &[
            "crates/admiralty-runtime/src/session/**",
            "crates/admiralty-runtime/src/bin/admiralty-session.rs",
            "tests/integration/{session_continuity,input_uncertainty}.rs",
        ],
    ),
    (
        "P08",
        &[
            "crates/admiralty-runtime/src/providers/{mod,fake,codex,capabilities,detectors}.rs",
            "configs/profiles/providers/**",
            "tests/fixtures/providers/codex/**",
            "tests/integration/provider_codex.rs",
        ],
    ),
    (
        "P09",
        &[
            "crates/admiralty-runtime/src/workspace/{registry,git,runner,resources}.rs",
            "packaging/containers/**",
            "tests/integration/runner_isolation.rs",
        ],
    ),
    (
        "P10",
        &[
            "crates/admiralty-core/src/economics/**",
            "crates/admiralty-server/src/services/metrics.rs",
            "tests/integration/economic_conservation.rs",
            "docs/architecture/metrics.md",
        ],
    ),
    (
        "P11",
        &[
            "crates/admiralty-server/src/api/**",
            "crates/admiralty-server/src/main.rs",
            "crates/admiralty-cli/src/{main,doctor,init,export}.rs",
            "tests/integration/api_stream.rs",
        ],
    ),
    (
        "P12",
        &[
            "apps/dashboard/{index.html,vite.config.ts,tsconfig.json,tsconfig.app.json,tsconfig.node.json,eslint.config.js}",
            "apps/dashboard/src/{main.tsx,vite-env.d.ts}",
            "apps/dashboard/src/{app,api,components,styles}/**",
            "apps/dashboard/src/features/overview/**",
        ],
    ),
    (
        "P13",
        &[
            "crates/admiralty-server/src/services/legacy.rs",
            "crates/admiralty-core/src/plans/{import,lint,revision}.rs",
            "configs/profiles/legacy/**",
            "crates/admiralty-cli/src/{project,work}.rs",
            "apps/dashboard/src/features/{projects,workplans}/**",
            "tests/integration/plan_import.rs",
        ],
    ),
    (
        "P14",
        &[
            "crates/admiralty-core/src/scheduling/**",
            "crates/admiralty-server/src/services/coordinator.rs",
            "tests/integration/scheduler_admission.rs",
        ],
    ),
    (
        "P15",
        &[
            "crates/admiralty-runtime/src/verification/{candidate,oracle,receipts,checks}.rs",
            "tests/integration/exact_candidate.rs",
        ],
    ),
    (
        "P16",
        &[
            "crates/admiralty-runtime/src/verification/jankurai.rs",
            "configs/profiles/quality/**",
            "tests/fixtures/jankurai/**",
            "tests/integration/jankurai_staged.rs",
            "docs/contributors/quality.md",
        ],
    ),
    (
        "P17",
        &[
            "crates/admiralty-core/src/coordination/{claims,interfaces}.rs",
            "crates/admiralty-runtime/src/agent_tools/**",
            "apps/dashboard/src/features/{coordination,agents}/**",
            "tests/integration/peer_handoff.rs",
        ],
    ),
    (
        "P18",
        &[
            "crates/admiralty-runtime/src/integration/{conflict,train}.rs",
            "tests/integration/integration_train.rs",
        ],
    ),
    (
        "P19",
        &[
            "crates/admiralty-runtime/src/integration/{publisher,reconcile}.rs",
            "tests/integration/publication_gate.rs",
            "docs/operators/publication.md",
        ],
    ),
    (
        "P20",
        &[
            "crates/admiralty-runtime/src/workspace/cleanup.rs",
            "tests/integration/cleanup_custody.rs",
            "docs/operators/cleanup.md",
        ],
    ),
    (
        "P21",
        &[
            "crates/admiralty-runtime/src/providers/claude.rs",
            "tests/fixtures/providers/claude/**",
            "tests/integration/provider_claude.rs",
        ],
    ),
    (
        "P22",
        &[
            "crates/admiralty-runtime/src/providers/{agy,grok,process}.rs",
            "tests/fixtures/providers/{agy,grok,process}/**",
            "tests/integration/provider_matrix.rs",
        ],
    ),
    (
        "P23",
        &[
            "crates/admiralty-runtime/src/fleet/{enrollment,transport,transfer}.rs",
            "crates/admiralty-runtime/src/bin/admiralty-node.rs",
            "crates/admiralty-cli/src/node.rs",
            "tests/integration/fleet_transport.rs",
        ],
    ),
    (
        "P24",
        &[
            "crates/admiralty-runtime/src/fleet/{placement,partition}.rs",
            "tests/integration/fleet_partition.rs",
            "docs/operators/fleet.md",
        ],
    ),
    (
        "P25",
        &[
            "crates/admiralty-runtime/src/operations/**",
            "packaging/{systemd,launchd}/**",
            "tests/integration/{upgrade_continuity,disaster_restore}.rs",
            "docs/operators/{upgrades,recovery}.md",
        ],
    ),
    (
        "P26",
        &[
            "apps/dashboard/src/features/economics/**",
            "apps/dashboard/e2e/economics.spec.ts",
        ],
    ),
    (
        "P27",
        &[
            "crates/admiralty-server/src/services/improvement/{excursions,proposals}.rs",
            "apps/dashboard/src/features/incidents/**",
            "tests/integration/excursion_lineage.rs",
        ],
    ),
    (
        "P28",
        &[
            "crates/admiralty-server/src/services/improvement/{experiments,datasets}.rs",
            "crates/admiralty-core/src/evaluation/**",
            "apps/dashboard/src/features/experiments/**",
            "tests/integration/experiment_validity.rs",
            "docs/architecture/improvement.md",
        ],
    ),
    (
        "P29",
        &[
            "apps/dashboard/src/features/{integration,quality,settings,assets}/**",
            "apps/dashboard/e2e/{integration,quality,assets,settings}.spec.ts",
        ],
    ),
    (
        "P30",
        &[
            "scripts/{package,release}/**",
            "crates/admiralty-cli/src/demo.rs",
            "docs/{operators,contributors,brand}/**",
            "apps/dashboard/public/brand/**",
            "README.md",
            "CONTRIBUTING.md",
            "SECURITY.md",
            "CODE_OF_CONDUCT.md",
            "LICENSE",
        ],
    ),
    (
        "P31",
        &[
            "tests/qualification/**",
            "benchmarks/**",
            "docs/operators/deployment.md",
            "docs/plans/admiralty-v2/receipts/**",
            "docs/plans/admiralty-v2/EXECUTION.md",
        ],
    ),
];

/// Fail closed when a plan's scope changes: reviewers must reconcile the
/// explicit paths above rather than silently inheriting expanded permissions.
pub const SCOPE_DIGESTS: [&str; 32] = [
    "aae3a40c341d0bd3fc51e17fbacc7f21355afa87449d822f1e79e081c56dc0d9",
    "bc1cf4ca6bcfd6ba1c07782e04246055f5aef6731e54ecb2ce7f60376964e3d3",
    "3b6ba27498c106ae53aa43857855c158348df471abc44a96a002ed3ed972477d",
    "778d87f6bff9569b9863b3e905ecdd404f322364772cf468d4a568362b5a6680",
    "704c92229a10d964c390cf7d1083ba496d55f72c9eb97ab80b106eb3d94a1405",
    "72a01972e4796ba33089ddebc4e2e42c1587a92f22fffa2ef58cc1357f0dc8b8",
    "f6d0c46e76a154434aa2f72539a475144dd04978e632486b6783e4faa98fff2f",
    "1562c9189cc8c9dd7987f70e4cc2f74ca6ac0753d259cf4399cbd613a4d66aa5",
    "71daa1b38972f79637026cb681284e8ac3660d10acacf95b3d80b7f85471f0e3",
    "91abf0dc73339c5406ffedabd8af089d9aeab294c04cdb8f869d3fc0ec488f1b",
    "2774895bfc79de55a35d7fb66a3b3ea80c842fe7b8d35f559a25d6485575a6ec",
    "e2a2b619612da3bee6789fbf437bc2e2e801ea4a9c526a3bf7cc412c3156f595",
    "cbefb4d5f4ad5c0fe42a94c763611b2d769eff82bf6d4374b5e16a010ac93c8f",
    "9de44cc6947a9a97be3dbf8132742e10d7053009d961a37d5fbe196fe8453f62",
    "e305aa92196c0ba19c2bb8ec1d0b6de34c62e94619daadc6b63b834293dd8320",
    "ee685a0eb679c818e675a8ccb9e310261a5005e09d09750f729907388bb9be4b",
    "f53c99b097003f6aeb1f29f00fc164665092043db10707214da3a38981c39ac1",
    "14ce50be2eb3e1e7710bb5519822ab8096b5b8b2d56ab230589225ec2f06556d",
    "a2905ef20cfc6c9f7cc0ed4595da25dab1004834efce0b02aa4465de122ce596",
    "444634495645aa9eb1ce30c1d4462939b24dd98a59e4a6a0346b9312a0ee366c",
    "132f6595307b03dcbca5e4c7087d83b9c24b76fddbf79fec37727c764d6e26cb",
    "7566f8a1ae301a8de0a5869df3409b91468e2205e059455fca27478257863926",
    "1f5a7a31a77fe4f5976a286a96bb7fa1ac6f7c938e0f29c42daf105c76277755",
    "6860313971428bf6f3b3e1c4b09b3a4e47afad843f2348bc045dc9e2d953d475",
    "e5c477149f088798e79c0c897560191d0ddb493279c5aad457b70615cdb37474",
    "aeb95a79aead10669c077b1ce490604b173296c1a0b384cb34379519d50c9604",
    "fc7c2716d9888513ab70f8c23d37036627b8419cbcfb38e278aac9488764b7c7",
    "883e8e5115af74eb3d74564727ab771afd9ea169c7546f96bc4db1fdfdfa1b4c",
    "fae2123060d60a6a61019cac01eebed273e35332c402650ca5ff88075a1d3813",
    "3c1a0e9dbf3e6be243e8522b94c285f5b5adaee377f566263e3b60b223d53a78",
    "ef0b540d8cf819477abf51047a4309a8781afa94376bc169bc31e12eeaa240d3",
    "693897c0e7d7fd269562628586e4b4b47cb7cd2d7a18728fb2bbc69ee4bf66f2",
];

/// Packet identifiers in allowlist order.
pub fn packet_ids() -> impl Iterator<Item = &'static str> {
    ALLOWLISTS.iter().map(|(identity, _)| *identity)
}

fn packet_index(packet_id: &str) -> Option<usize> {
    ALLOWLISTS
        .iter()
        .position(|(identity, _)| *identity == packet_id)
}

/// Lowercase SHA-256 hex digest.
pub fn digest(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `now()`: UTC `isoformat(timespec="seconds")`.
pub fn now() -> String {
    pytime::now_iso()
}

/// `expand_braces(pattern)`.
pub fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(capture) = BRACES.captures(pattern) else {
        return vec![pattern.to_owned()];
    };
    let whole = capture.get(0).map_or(0..0, |m| m.range());
    let inner = capture.get(1).map_or("", |m| m.as_str());
    inner
        .split(',')
        .flat_map(|part| {
            expand_braces(&format!(
                "{}{}{}",
                &pattern[..whole.start],
                part,
                &pattern[whole.end..]
            ))
        })
        .collect()
}

/// `safe_path(value, pattern=...)` for a relative repository path.
pub fn safe_path(value: Option<&str>, pattern: bool) -> ToolResult<String> {
    let value = value
        .filter(|value| {
            let count = value.chars().count();
            0 < count && count <= 500
        })
        .ok_or_else(|| ToolError::invalid("missing or oversized path"))?;
    require(
        !value.contains('\\') && !value.chars().any(|c| (c as u32) < 32),
        "unsafe path",
    )?;
    require(
        !value.starts_with('/')
            && value
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")),
        "path must be a relative non-traversing path",
    )?;
    require(
        value.split('/').next() != Some(".git"),
        "Git metadata is forbidden",
    )?;
    if !pattern {
        require(
            !value.chars().any(|c| "*?[]{}".contains(c)),
            "concrete path required",
        )?;
    }
    Ok(value.to_owned())
}

/// `allowlist(packet_id)`: sorted, expanded, validated patterns.
pub fn allowlist(packet_id: &str) -> ToolResult<Vec<String>> {
    let index = packet_index(packet_id).ok_or_else(|| ToolError::invalid("unknown packet ID"))?;
    let mut patterns = BTreeSet::new();
    for raw in ALLOWLISTS[index].1 {
        for pattern in expand_braces(raw) {
            patterns.insert(safe_path(Some(&pattern), true)?);
        }
    }
    Ok(patterns.into_iter().collect())
}

pub use crate::fsx::fnmatchcase;

/// `allows(path, patterns)`.
pub fn allows(path: &Value, patterns: &[String]) -> ToolResult<bool> {
    let path = safe_path(path.as_str(), false)?;
    for pattern in patterns {
        if let Some(stem) = pattern.strip_suffix("**")
            && pattern.ends_with("/**")
            && path.starts_with(stem)
        {
            return Ok(true);
        }
        // fnmatch's '*' crosses slashes: forbid that for exact file patterns.
        if path.split('/').count() == pattern.split('/').count() && fnmatchcase(&path, pattern) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `required_field(body, field)`; re-expresses the Python look-ahead regex
/// `\*\*FIELD:\*\*\s*(.+?)(?=\s*\*\*|\n\n|\Z)` (re.S) procedurally.
pub fn required_field(body: &str, field: &str) -> ToolResult<String> {
    let missing = || ToolError::invalid(format!("missing {field}"));
    let marker = format!("**{field}:**");
    let start = body.find(&marker).ok_or_else(missing)? + marker.len();
    if start == body.len() {
        return Err(missing());
    }
    let rest = &body[start..];
    let whitespace: usize = rest
        .chars()
        .take_while(|c| pyjson::is_py_whitespace(*c))
        .map(char::len_utf8)
        .sum();
    // `.+?` needs one character; give one whitespace back when only
    // whitespace remains (regex backtracking).
    let group_start = if whitespace == rest.len() {
        start + rest.chars().last().map_or(0, |c| rest.len() - c.len_utf8())
    } else {
        start + whitespace
    };
    let lookahead = |at: usize| -> bool {
        if at == body.len() {
            return true;
        }
        let tail = &body[at..];
        if tail.starts_with("\n\n") {
            return true;
        }
        tail.trim_start_matches(pyjson::is_py_whitespace)
            .starts_with("**")
    };
    let group = &body[group_start..];
    let mut end = body.len();
    for (offset, character) in group.char_indices() {
        let candidate = group_start + offset + character.len_utf8();
        if lookahead(candidate) {
            end = candidate;
            break;
        }
    }
    let value = pyjson::py_strip(&body[group_start..end]);
    require(!value.is_empty(), format!("missing {field}"))?;
    Ok(value.to_owned())
}

/// `dependencies(raw)`.
pub fn dependencies(raw: &str) -> ToolResult<Vec<String>> {
    let raw = raw.trim_end_matches('.');
    if raw == "none" {
        return Ok(Vec::new());
    }
    let result: Vec<String> = if RANGE.is_match(raw) {
        let numbers: Vec<u32> = RANGE_NUMBER
            .captures_iter(raw)
            .filter_map(|capture| capture[1].parse().ok())
            .collect();
        let (low, high) = (numbers[0], numbers[1]);
        require(low <= high, "backwards dependency range")?;
        (low..=high).map(|number| format!("P{number:02}")).collect()
    } else {
        // The semicolon describes a conditional later-stage dependency. Keep
        // it as a full dependency here: conservative full-packet readiness.
        DEPENDENCY
            .find_iter(raw)
            .map(|m| m.as_str().to_owned())
            .collect()
    };
    let unique: HashSet<&String> = result.iter().collect();
    require(
        !result.is_empty() && unique.len() == result.len(),
        "missing or duplicate dependencies",
    )?;
    require(
        result.iter().all(|identity| PACKET_ID.is_match(identity)),
        "unsafe dependency ID",
    )?;
    Ok(result)
}

fn hashable_strings(items: &[Value]) -> ToolResult<HashSet<String>> {
    items.iter().map(pyjson::py_hash_key).collect()
}

/// `validate_graph(packets)`.
pub fn validate_graph(packets: &[Value]) -> ToolResult<()> {
    let mut order = Vec::new();
    let mut index: HashMap<String, &Value> = HashMap::new();
    for packet in packets {
        let identity = packet.get("id")?.and_then(Value::as_str);
        let identity = identity
            .filter(|identity| PACKET_ID.is_match(identity))
            .ok_or_else(|| ToolError::invalid("unsafe packet ID"))?;
        require(!index.contains_key(identity), "duplicate packet ID")?;
        index.insert(identity.to_owned(), packet);
        order.push(identity.to_owned());
    }
    let expected: HashSet<&str> = packet_ids().collect();
    require(
        index.len() == expected.len() && index.keys().all(|key| expected.contains(key.as_str())),
        "expected exactly P00 through P31",
    )?;
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    fn visit(
        identity: &str,
        index: &HashMap<String, &Value>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) -> ToolResult<()> {
        require(
            !visiting.contains(identity),
            format!("dependency cycle at {identity}"),
        )?;
        if visited.contains(identity) {
            return Ok(());
        }
        visiting.insert(identity.to_owned());
        let packet = index
            .get(identity)
            .ok_or_else(|| ToolError::Key(identity.to_owned()))?;
        let dependencies = match packet.get("depends_on")? {
            Some(Value::Array(items)) if hashable_strings(items)?.len() == items.len() => items,
            _ => {
                return Err(ToolError::invalid(format!(
                    "invalid dependencies for {identity}"
                )));
            }
        };
        for dependency in dependencies {
            let name = dependency.as_str().filter(|name| index.contains_key(*name));
            let Some(name) = name else {
                return Err(ToolError::invalid(format!(
                    "unknown dependency {}",
                    pyjson::py_str(dependency)
                )));
            };
            visit(name, index, visiting, visited)?;
        }
        visiting.remove(identity);
        visited.insert(identity.to_owned());
        Ok(())
    }
    for identity in &order {
        visit(identity, &index, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn steps(body: &str) -> Vec<String> {
    let lines: Vec<&str> = body.split('\n').collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let starts = STEP_START
            .find(line)
            .is_some_and(|prefix| prefix.end() < line.len());
        if !starts {
            index += 1;
            continue;
        }
        let mut step = line.to_owned();
        let mut next = index + 1;
        while next < lines.len() {
            let candidate = lines[next];
            if candidate.is_empty() || STEP_START.is_match(candidate) || candidate.starts_with("**")
            {
                break;
            }
            step.push('\n');
            step.push_str(candidate);
            next += 1;
        }
        result.push(step);
        index = next;
    }
    result
}

/// `parse_plan(text)`.
pub fn parse_plan(text: &str) -> ToolResult<Vec<Value>> {
    let matches: Vec<_> = HEADING.captures_iter(text).collect();
    require(matches.len() == 32, "expected 32 packet headings")?;
    let mut packets = Vec::new();
    for (number, capture) in matches.iter().enumerate() {
        let identity = capture.get(1).map_or("", |m| m.as_str());
        let title = capture.get(2).map_or("", |m| m.as_str());
        require(PACKET_ID.is_match(identity), "unsafe packet ID")?;
        let body_start = capture.get(0).map_or(0, |m| m.end());
        let end = matches
            .get(number + 1)
            .and_then(|next| next.get(0))
            .map_or(text.len(), |m| m.start());
        let section = &text[body_start..end];
        let body = pyjson::py_strip(section.split("\n## ").next().unwrap_or(""));
        let scope = required_field(body, "Write scope")?;
        let index = packet_index(identity).ok_or_else(|| ToolError::Key(identity.to_owned()))?;
        require(
            digest(scope.as_bytes()) == SCOPE_DIGESTS[index],
            format!("{identity} scope changed; review explicit allowlist"),
        )?;
        let steps = steps(body);
        require(
            !steps.is_empty(),
            format!("{identity} has no implementation steps"),
        )?;
        let depends_on = dependencies(&required_field(body, "Depends on")?)?;
        let owner = required_field(body, "Owner")?
            .trim_end_matches('.')
            .to_owned();
        let release = required_field(body, "Release")?
            .trim_end_matches('.')
            .to_owned();
        let read_first = required_field(body, "Read first")?;
        let write_allowlist = allowlist(identity)?;
        let validation = required_field(body, "Validate")?;
        let acceptance = required_field(body, "Pass")?;
        let stop = if body.contains("**Stop:**") {
            Value::Str(required_field(body, "Stop")?)
        } else {
            Value::Null
        };
        let mut packet = Map::new();
        packet.insert("id", identity);
        packet.insert("title", title);
        packet.insert(
            "depends_on",
            Value::Array(depends_on.into_iter().map(Value::Str).collect()),
        );
        packet.insert("owner", owner);
        packet.insert("release", release);
        packet.insert("read_first", read_first);
        packet.insert("source_write_scope", scope);
        packet.insert(
            "write_allowlist",
            Value::Array(write_allowlist.into_iter().map(Value::Str).collect()),
        );
        packet.insert(
            "steps",
            Value::Array(steps.into_iter().map(Value::Str).collect()),
        );
        packet.insert("validation", validation);
        packet.insert("acceptance", acceptance);
        packet.insert("stop", stop);
        packet.insert("source_sha256", digest(body.as_bytes()));
        packets.push(Value::Object(packet));
    }
    validate_graph(&packets)?;
    Ok(packets)
}

fn field_str(packet: &Value, key: &str) -> ToolResult<String> {
    Ok(pyjson::py_str(packet.index_key(key)?))
}

fn string_list(value: &Value) -> ToolResult<Vec<String>> {
    Ok(pyjson::py_iter(value)?.iter().map(pyjson::py_str).collect())
}

/// `capsule(packet, repo, base)`: the bounded worker mission text.
pub fn capsule(packet: &Value, repo: &Path, base: &str) -> ToolResult<String> {
    let paths: Vec<String> = string_list(packet.index_key("write_allowlist")?)?
        .into_iter()
        .map(|path| format!("- `{path}`"))
        .collect();
    let dependencies = string_list(packet.index_key("depends_on")?)?.join(", ");
    let dependencies = if dependencies.is_empty() {
        "none".to_owned()
    } else {
        dependencies
    };
    let steps = string_list(packet.index_key("steps")?)?.join("\n");
    let stop = packet.index_key("stop")?;
    let stop = if stop.truthy() {
        pyjson::py_str(stop)
    } else {
        "Apply the shared stop conditions above.".to_owned()
    };
    let identity = field_str(packet, "id")?;
    let title = field_str(packet, "title")?;
    let owner = field_str(packet, "owner")?;
    let release = field_str(packet, "release")?;
    let read_first = field_str(packet, "read_first")?;
    let scope = field_str(packet, "source_write_scope")?;
    let validation = field_str(packet, "validation")?;
    let acceptance = field_str(packet, "acceptance")?;
    let repo = repo.display();
    let paths = paths.join("\n");
    Ok(format!(
        "# {identity} — {title}

Objective: implement this packet's complete acceptance contract. Owner: {owner}. Release: {release}.
Repository: `{repo}`. Pinned base: `{base}`. Dependencies: {dependencies}.
This is a prepared mission, not a claim or authorization to take over an existing session.

Before starting: obtain an explicit claim; read current local instructions and integrated dependency receipts; verify actual base and a clean, privately owned worktree. Stop on missing dependency proof, stale base, conflicting ownership, essential unknowns, scope expansion, or unsafe effects. Do not load all proposals or other projects' prompts.

Read first, in order: {read_first}
Shared reference: `docs/plans/admiralty-v2/MASTER_PLAN.md` §§1.4, 3.1, 12–14 and the sections named above. Read only relevant sections.

Exact write allowlist (`/**` means descendants; other patterns match one path component):
{paths}

Original scope restrictions: {scope}
Allowlist is an upper bound. Shared manifests, lockfiles, module wiring, migration numbering, generated contracts and CI require the integration owner. Submit exact requested edits when not expressly owned. Generated artifacts require their generator; no handwritten output. Preserve original exports, private transcripts, credentials and other agents' changes. All paths outside this list are read-only. Receipt handoff is written to the runtime output directory; integration owner files the accepted receipt in the repository.

Implementation:
{steps}

Validation: {validation}
Acceptance: {acceptance}
Packet stop condition: {stop}

Return one bounded candidate plus JSON receipt: schema_version=1, packet_id, owner, base_sha, head_sha, interface_version, changed_paths, criteria[{{id,status,evidence}}], commands[{{argv,cwd,exit_code}}], environment, artifacts[{{path,sha256}}], risks, blockers, next_action. Use full Git IDs and computed SHA-256 digests; retain failed commands and repairs. Report unavailable checks explicitly. Never equate a final message or exit code with verified/integrated status.

Report only changed evidence, blocker owner, exact API/schema requests and next action. Routine healthy status needs no model turn. Peer interface questions go to the scoped owner; captains receive decisions and blockers. No live provider, terminal input, remote publication, cleanup or fleet takeover is authorized by this capsule.
"
    ))
}

/// `read_json(path)`: regular, non-symlink, bounded, duplicate-free object.
pub fn read_json(path: &Path) -> ToolResult<Value> {
    require(
        path.is_file() && !fsx::is_symlink(path),
        format!("missing or symlink JSON: {}", path.display()),
    )?;
    require(fs::metadata(path)?.len() <= MAX_JSON, "oversized JSON")?;
    let data = fs::read(path)?;
    let text = std::str::from_utf8(&data)
        .map_err(|error| ToolError::invalid(format!("invalid JSON: {error}")))?;
    let value = pyjson::loads(
        text,
        LoadOptions {
            reject_duplicates: true,
            reject_constants: true,
        },
    )
    .map_err(|error| match error {
        LoadError::DuplicateKey(key) => {
            ToolError::invalid(format!("invalid JSON: duplicate JSON field: {key}"))
        }
        other => ToolError::invalid(format!("invalid JSON: {other}")),
    })?;
    require(value.as_object().is_some(), "JSON object required")?;
    Ok(value)
}

fn truthy_fields(value: &Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|map| {
        keys.iter()
            .all(|key| map.get(key).is_some_and(Value::truthy))
    })
}

fn str_list_value(items: &[String]) -> Value {
    Value::Array(items.iter().cloned().map(Value::Str).collect())
}

fn sha_field(value: &Value, message: &str) -> ToolResult<()> {
    match value {
        Value::Str(text) => require(SHA.is_match(text), message),
        other => Err(ToolError::type_error(format!(
            "expected string or bytes-like object, got '{}'",
            other.type_name()
        ))),
    }
}

/// `validate_manifest(manifest)`.
pub fn validate_manifest(manifest: &Value) -> ToolResult<()> {
    require(
        manifest.get("schema_version")? == Some(&Value::Int(1)),
        "unsupported manifest schema",
    )?;
    require(
        truthy_fields(
            manifest.get_value("project")?,
            &["id", "name", "repository_url"],
        ),
        "missing project identity",
    )?;
    require(
        manifest
            .get_value("base_ref")?
            .as_str()
            .is_some_and(|base| SHA.is_match(base)),
        "full base_ref required",
    )?;
    let packets = manifest
        .get_value("packets")?
        .as_array()
        .ok_or_else(|| ToolError::invalid("packets list required"))?;
    require(
        manifest.get_value("lanes")?.as_array().is_some()
            && manifest.get_value("pr")?.as_object().is_some(),
        "lanes/pr required",
    )?;
    validate_graph(packets)?;
    let mut index: HashMap<String, &Value> = HashMap::new();
    for packet in packets {
        index.insert(field_str(packet, "id")?, packet);
    }
    for packet in packets {
        let identity = field_str(packet, "id")?;
        let status = packet.get_value("status")?;
        let status_text = status.as_str().filter(|status| STATUSES.contains(status));
        if !matches!(
            status,
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_)
        ) {
            return Err(ToolError::type_error(format!(
                "unhashable type: '{}'",
                status.type_name()
            )));
        }
        let status = status_text.ok_or_else(|| ToolError::invalid("unknown packet status"))?;
        require(
            packet.get_value("write_allowlist")? == &str_list_value(&allowlist(&identity)?),
            "write allowlist drift",
        )?;
        let mut ready = true;
        for dependency in pyjson::py_iter(packet.index_key("depends_on")?)? {
            let name = pyjson::py_str(&dependency);
            let other = index
                .get(&name)
                .ok_or_else(|| ToolError::Key(name.clone()))?;
            if other.index_key("status")? != &Value::Str("integrated".to_owned()) {
                ready = false;
                break;
            }
        }
        if !matches!(status, "planned" | "blocked") {
            require(ready, format!("{identity} dependencies are not integrated"))?;
        }
        if ACTIVE.contains(&status) {
            let claim = packet.get_value("claim")?;
            require(
                truthy_fields(claim, &["owner", "lane"]),
                "active packet requires explicit claim",
            )?;
            require(
                packet.get_value("lane")? == claim.index_key("lane")?,
                "claim lane mismatch",
            )?;
        }
        if matches!(status, "verified" | "integrated") {
            let proof = packet.get_value("verification")?;
            require(
                truthy_fields(proof, &["receipt", "revision", "verifier", "observed_utc"]),
                "verified requires independent provenance",
            )?;
            sha_field(
                proof.index_key("revision")?,
                "verification revision must be full Git ID",
            )?;
        }
        if status == "integrated" {
            let proof = packet.get_value("integration")?;
            require(
                truthy_fields(
                    proof,
                    &["receipt", "revision", "target_ref", "observed_utc"],
                ),
                "integrated requires integration provenance",
            )?;
            sha_field(
                proof.index_key("revision")?,
                "integration revision must be full Git ID",
            )?;
        }
        require(
            packet.get_value("evidence")?.as_array().is_some(),
            "evidence list required",
        )?;
        safe_path(packet.get_value("capsule_path")?.as_str(), false)?;
        require(
            packet
                .get_value("capsule_sha256")?
                .as_str()
                .is_some_and(|value| DIGEST.is_match(value)),
            "capsule digest required",
        )?;
    }
    Ok(())
}

/// `verify_capsules(output, manifest)`.
pub fn verify_capsules(output: &Path, manifest: &Value) -> ToolResult<()> {
    for packet in pyjson::py_iter(manifest.index_key("packets")?)? {
        let relative = safe_path(packet.index_key("capsule_path")?.as_str(), false)?;
        let path = output.join(relative);
        require(
            fsx::resolve(&path).starts_with(output) && !fsx::is_symlink(&path),
            "unsafe capsule path",
        )?;
        let expected = packet.index_key("capsule_sha256")?;
        require(
            path.is_file() && &Value::Str(digest(&fs::read(&path)?)) == expected,
            "capsule digest mismatch",
        )?;
    }
    Ok(())
}

/// `atomic_write(path, data)` with an injectable replacement.
pub fn atomic_write(path: &Path, data: &[u8], replace: ReplaceFn<'_>) -> ToolResult<()> {
    fsx::atomic_write(path, ".write-", data, replace)?;
    Ok(())
}

/// `json_bytes(value)`: sorted, indented, UTF-8, finite floats only.
pub fn json_bytes(value: &Value) -> ToolResult<Vec<u8>> {
    let mut text = pyjson::dumps(
        value,
        &DumpOptions::PYTHON
            .sorted()
            .indent(2)
            .unicode()
            .strict_floats(),
    )?;
    text.push('\n');
    Ok(text.into_bytes())
}

fn marker_value(repo: &Path) -> Value {
    crate::pyobj! {
        "schema_version" => 1,
        "repo" => repo.display().to_string(),
        "uid" => fsx::getuid(),
    }
}

/// `owned_output(raw, repo, create=...)`.
pub fn owned_output(raw: &Path, repo: &Path, create: bool) -> ToolResult<PathBuf> {
    let output = fsx::absolute(&fsx::expanduser(raw))?;
    // macOS exposes its OS-owned temporary roots through these standard
    // aliases. User-created aliases, including the output itself, are denied.
    let aliases = [("/tmp", "/private/tmp"), ("/var", "/private/var")];
    for part in fsx::self_and_parents(&output) {
        if fsx::is_symlink(&part) {
            let allowed = aliases.iter().any(|(alias, target)| {
                part == Path::new(alias)
                    && fsx::resolve(&part) == Path::new(target)
                    && fs::symlink_metadata(&part)
                        .is_ok_and(|meta| std::os::unix::fs::MetadataExt::uid(&meta) == 0)
            });
            require(allowed, "output symlinks are forbidden")?;
        }
    }
    let output = fsx::resolve(&output);
    require(
        output != repo && !output.starts_with(repo),
        "output must be outside repository",
    )?;
    require(
        output != Path::new("/") && output != Path::new(&fsx::home_dir()),
        "choose a dedicated output directory",
    )?;
    let marker = output.join(OWNER_FILE);
    if !output.exists() {
        require(create, "output has not been initialized")?;
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::DirBuilder::new().mode(0o700).create(&output)?;
    }
    require(
        output.is_dir() && fsx::owner(&output)? == fsx::getuid(),
        "output is not an owned directory",
    )?;
    if !marker.exists() {
        require(
            create && fs::read_dir(&output)?.next().is_none(),
            "output must be empty or initialized by this tool",
        )?;
        atomic_write(&marker, &json_bytes(&marker_value(repo))?, &fsx::os_replace)?;
    }
    require(
        read_json(&marker)? == marker_value(repo),
        "output ownership marker mismatch",
    )?;
    for child in ["rollout.json", "generations", "receipts", ".lock"] {
        require(
            !fsx::is_symlink(&output.join(child)),
            "output contains unsafe symlink",
        )?;
    }
    Ok(output)
}

/// Advisory exclusive lock held for the guard's lifetime (`output_lock`).
pub struct OutputLock {
    _file: File,
}

/// `output_lock(output)`: blocking exclusive `flock` on `<output>/.lock`.
pub fn output_lock(output: &Path) -> ToolResult<OutputLock> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(output.join(".lock"))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)?;
    Ok(OutputLock { _file: file })
}

/// `resolve_base(repo, ref)`: full commit ID via `git rev-parse`.
pub fn resolve_base(repo: &Path, reference: &str) -> ToolResult<String> {
    require(
        !reference.is_empty() && !reference.starts_with('-') && !reference.contains('\n'),
        "unsafe base reference",
    )?;
    let captured = proc::run_bounded(
        Command::new("git").arg("-C").arg(repo).args([
            "rev-parse",
            "--verify",
            &format!("{reference}^{{commit}}"),
        ]),
        None,
        Duration::from_secs(10),
        true,
        None,
    )?;
    let stdout = String::from_utf8_lossy(&captured.stdout);
    let value = pyjson::py_strip(&stdout);
    require(
        captured.status.success() && SHA.is_match(value),
        "base reference does not resolve to a commit",
    )?;
    Ok(value.to_owned())
}

/// Export options.
pub struct ExportOptions<'a> {
    /// `--project-id`.
    pub project_id: &'a str,
    /// `--repository-url`.
    pub repository_url: &'a str,
    /// Replacement used for manifest/capsule publication.
    pub replace: ReplaceFn<'a>,
}

impl Default for ExportOptions<'_> {
    fn default() -> Self {
        Self {
            project_id: "admiralty-v2",
            repository_url: "https://github.com/neverhuman/admiralty",
            replace: &fsx::os_replace,
        }
    }
}

const PRESERVED: [&str; 6] = [
    "status",
    "lane",
    "claim",
    "verification",
    "integration",
    "evidence",
];

/// `export(repo, output, base, ...)`.
pub fn export(
    repo: &Path,
    output: &Path,
    base: &str,
    options: &ExportOptions<'_>,
) -> ToolResult<Value> {
    require(IDENTITY.is_match(options.project_id), "unsafe project ID")?;
    let plan = repo.join(PLAN_DIR);
    let work = fs::read(plan.join("WORK_PACKETS.md"))?;
    let master = fs::read(plan.join("MASTER_PLAN.md"))?;
    let work_text = std::str::from_utf8(&work)
        .map_err(|error| ToolError::type_error(format!("'utf-8' codec can't decode: {error}")))?;
    let mut packets = parse_plan(work_text)?;
    let generation = uuid::Uuid::new_v4().simple().to_string();
    let mut manifest = Map::new();
    manifest.insert("schema_version", 1);
    manifest.insert("producer", PRODUCER);
    manifest.insert("updated_utc", now());
    manifest.insert(
        "project",
        crate::pyobj! {
            "id" => options.project_id,
            "name" => "Admiralty v2 implementation",
            "repository_url" => options.repository_url,
        },
    );
    manifest.insert("base_ref", base);
    manifest.insert(
        "source",
        crate::pyobj! {
            "work_packets_sha256" => digest(&work),
            "master_plan_sha256" => digest(&master),
        },
    );
    manifest.insert("packets", Value::Array(Vec::new()));
    manifest.insert("lanes", Value::Array(Vec::new()));
    manifest.insert("pr", crate::pyobj! { "url" => Value::Null, "head" => base });
    manifest.insert("generation", generation.as_str());
    let rollout = output.join("rollout.json");
    let previous = if rollout.exists() {
        Some(read_json(&rollout)?)
    } else {
        None
    }
    .filter(Value::truthy);
    let mut old: HashMap<String, Value> = HashMap::new();
    if let Some(previous) = &previous {
        validate_manifest(previous)?;
        require(
            previous.index_key("source")? == manifest.get("source").unwrap_or(&NULL)
                && previous.index_key("base_ref")? == &Value::Str(base.to_owned())
                && previous.index_key("project")? == manifest.get("project").unwrap_or(&NULL),
            "source/base/project changed; reconcile in a fresh owned output directory",
        )?;
        manifest.insert("lanes", previous.index_key("lanes")?.clone());
        manifest.insert("pr", previous.index_key("pr")?.clone());
        for packet in pyjson::py_iter(previous.index_key("packets")?)? {
            old.insert(field_str(&packet, "id")?, packet);
        }
    }
    let mut rendered_capsules = Vec::new();
    for packet in &mut packets {
        let identity = field_str(packet, "id")?;
        let map = packet
            .as_object_mut()
            .ok_or_else(|| ToolError::invalid("packet must be an object"))?;
        let has_dependencies = map.get("depends_on").is_some_and(Value::truthy);
        map.insert("status", if has_dependencies { "planned" } else { "ready" });
        for key in ["lane", "claim", "verification", "integration"] {
            map.insert(key, Value::Null);
        }
        map.insert("evidence", Value::Array(Vec::new()));
        if let Some(previous) = old.get(&identity) {
            for key in PRESERVED {
                map.insert(key, previous.index_key(key)?.clone());
            }
        }
        let rendered = capsule(packet, repo, base)?;
        let map = packet
            .as_object_mut()
            .ok_or_else(|| ToolError::invalid("packet must be an object"))?;
        map.insert(
            "capsule_path",
            format!("generations/{generation}/capsules/{identity}.md"),
        );
        map.insert("capsule_sha256", digest(rendered.as_bytes()));
        rendered_capsules.push((identity, rendered));
    }
    let statuses: HashMap<String, Value> = packets
        .iter()
        .map(|packet| {
            Ok((
                field_str(packet, "id")?,
                packet.index_key("status")?.clone(),
            ))
        })
        .collect::<ToolResult<_>>()?;
    for packet in &mut packets {
        let status = packet.index_key("status")?.clone();
        if status == Value::Str("planned".to_owned()) || status == Value::Str("ready".to_owned()) {
            let mut ready = true;
            for dependency in pyjson::py_iter(packet.index_key("depends_on")?)? {
                let name = pyjson::py_str(&dependency);
                let state = statuses
                    .get(&name)
                    .ok_or_else(|| ToolError::Key(name.clone()))?;
                if state != &Value::Str("integrated".to_owned()) {
                    ready = false;
                    break;
                }
            }
            if let Some(map) = packet.as_object_mut() {
                map.insert("status", if ready { "ready" } else { "planned" });
            }
        }
    }
    manifest.insert("packets", Value::Array(packets));
    let manifest = Value::Object(manifest);
    validate_manifest(&manifest)?;
    let generations = output.join("generations");
    match fs::create_dir(&generations) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && generations.is_dir() => {
        }
        Err(error) => return Err(error.into()),
    }
    let staged = tempfile::Builder::new()
        .prefix(".generation-")
        .rand_bytes(8)
        .tempdir_in(&generations)?;
    let staged_path = staged.path().to_path_buf();
    fs::create_dir(staged_path.join("capsules"))?;
    for (identity, rendered) in &rendered_capsules {
        atomic_write(
            &staged_path.join("capsules").join(format!("{identity}.md")),
            rendered.as_bytes(),
            options.replace,
        )?;
    }
    atomic_write(
        &staged_path.join("mission.json"),
        &json_bytes(&manifest)?,
        options.replace,
    )?;
    fs::rename(&staged_path, generations.join(&generation))?;
    // The staged directory now lives under its generation name.
    let _ = staged.keep();
    fsx::fsync_dir(&generations)?;
    atomic_write(&rollout, &json_bytes(&manifest)?, options.replace)?;
    Ok(manifest)
}

fn packet_position(manifest: &Value, packet_id: &str) -> ToolResult<usize> {
    let packets = manifest
        .index_key("packets")?
        .as_array()
        .ok_or_else(|| ToolError::type_error("packets must be a list"))?;
    for (position, packet) in packets.iter().enumerate() {
        if packet.index_key("id")? == &Value::Str(packet_id.to_owned()) {
            return Ok(position);
        }
    }
    Err(ToolError::invalid("unknown packet ID"))
}

fn packet_mut(manifest: &mut Value, position: usize) -> ToolResult<&mut Map> {
    manifest
        .as_object_mut()
        .and_then(|map| map.get_mut("packets"))
        .and_then(Value::as_array_mut)
        .and_then(|packets| packets.get_mut(position))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ToolError::type_error("packet must be an object"))
}

/// `claim(manifest, pid, owner, lane, node, branch)`.
pub fn claim(
    manifest: &mut Value,
    packet_id: &str,
    owner: &str,
    lane: &str,
    node: Option<&str>,
    branch: &str,
) -> ToolResult<()> {
    require(packet_index(packet_id).is_some(), "unknown packet ID")?;
    require(
        IDENTITY.is_match(owner) && IDENTITY.is_match(lane),
        "unsafe owner or lane identity",
    )?;
    require(
        node.is_none_or(|node| IDENTITY.is_match(node)),
        "unsafe node identity",
    )?;
    require(
        BRANCH.is_match(branch)
            && !branch.contains("..")
            && !branch.contains("//")
            && !branch.ends_with('/')
            && !branch.ends_with('.'),
        "invalid codex branch",
    )?;
    let position = packet_position(manifest, packet_id)?;
    let packets = pyjson::py_iter(manifest.index_key("packets")?)?;
    require(
        packets[position].index_key("status")? == &Value::Str("ready".to_owned()),
        "only ready packets can be claimed",
    )?;
    for packet in &packets {
        if packet.index_key("lane")? == &Value::Str(lane.to_owned()) {
            let active = in_set(Some(packet.index_key("status")?), &ACTIVE)?;
            require(!active, "lane already has an active packet")?;
        }
    }
    // Readiness comes from integrated dependency evidence, not worker reports.
    let packet = packet_mut(manifest, position)?;
    packet.insert("status", "claimed");
    packet.insert("lane", lane);
    packet.insert(
        "claim",
        crate::pyobj! {
            "owner" => owner,
            "lane" => lane,
            "node" => node,
            "branch" => branch,
            "observed_utc" => now(),
        },
    );
    let map = manifest
        .as_object_mut()
        .ok_or_else(|| ToolError::type_error("manifest must be an object"))?;
    if let Some(Value::Array(lanes)) = map.get_mut("lanes") {
        lanes.push(crate::pyobj! {
            "id" => lane,
            "owner" => owner,
            "node" => node,
            "branch" => branch,
            "status" => "claimed",
        });
    }
    map.insert("updated_utc", now());
    validate_manifest(manifest)
}

const RECEIPT_FIELDS: [&str; 14] = [
    "schema_version",
    "packet_id",
    "owner",
    "base_sha",
    "head_sha",
    "interface_version",
    "changed_paths",
    "criteria",
    "commands",
    "environment",
    "artifacts",
    "risks",
    "blockers",
    "next_action",
];

fn nonempty_str(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn in_set(value: Option<&Value>, set: &[&str]) -> ToolResult<bool> {
    match value {
        None | Some(Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_)) => Ok(false),
        Some(Value::Str(text)) => Ok(set.contains(&text.as_str())),
        Some(other) => Err(ToolError::type_error(format!(
            "unhashable type: '{}'",
            other.type_name()
        ))),
    }
}

/// `validate_receipt(receipt, manifest)`; returns the packet position.
#[allow(clippy::too_many_lines)]
pub fn validate_receipt(receipt: &Value, manifest: &Value) -> ToolResult<usize> {
    let map = receipt
        .as_object()
        .ok_or_else(|| ToolError::type_error("receipt must be an object"))?;
    let mut missing: Vec<&str> = RECEIPT_FIELDS
        .iter()
        .copied()
        .filter(|field| !map.contains_key(field))
        .collect();
    missing.sort_unstable();
    require(
        missing.is_empty(),
        format!(
            "candidate receipt missing required fields: {}",
            missing.join(", ")
        ),
    )?;
    let field = |key: &str| map.get(key).unwrap_or(&NULL);
    require(
        field("schema_version") == &Value::Int(1),
        "unsupported receipt schema",
    )?;
    let packet_id = field("packet_id")
        .as_str()
        .filter(|identity| PACKET_ID.is_match(identity))
        .ok_or_else(|| ToolError::invalid("unsafe receipt packet ID"))?;
    let position = packet_position(manifest, packet_id)?;
    let packet = pyjson::py_index(manifest.index_key("packets")?, position)?;
    require(
        in_set(
            Some(packet.index_key("status")?),
            &["claimed", "in_progress", "blocked", "review"],
        )?,
        "receipt requires an active explicit claim",
    )?;
    require(
        field("owner") == packet.index_key("claim")?.index_key("owner")?,
        "receipt owner differs from claim",
    )?;
    require(
        field("base_sha") == manifest.index_key("base_ref")?,
        "receipt base differs from pinned base",
    )?;
    require(
        field("head_sha")
            .as_str()
            .is_some_and(|head| SHA.is_match(head)),
        "full candidate head required",
    )?;
    require(
        nonempty_str(Some(field("interface_version"))),
        "interface version required (use explicit unimplemented if unavailable)",
    )?;
    let paths = match field("changed_paths") {
        Value::Array(items)
            if !items.is_empty() && hashable_strings(items)?.len() == items.len() =>
        {
            items
        }
        _ => {
            return Err(ToolError::invalid("nonempty unique changed_paths required"));
        }
    };
    let patterns = string_list(packet.index_key("write_allowlist")?)?;
    for path in paths {
        require(
            allows(path, &patterns)?,
            "candidate changes files outside packet scope",
        )?;
    }
    let criteria = field("criteria")
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| ToolError::invalid("criteria required"))?;
    for criterion in criteria {
        let valid = match criterion {
            Value::Object(item) => {
                item.get("id").is_some_and(Value::truthy)
                    && in_set(item.get("status"), &["passed", "failed", "unavailable"])?
                    && item.get("evidence").is_some_and(Value::truthy)
            }
            _ => false,
        };
        require(valid, "invalid criterion")?;
    }
    let commands = field("commands")
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| ToolError::invalid("command records required"))?;
    for command in commands {
        let argv_ok = command.as_object().is_some_and(|item| {
            item.get("argv")
                .and_then(Value::as_array)
                .is_some_and(|argv| {
                    !argv.is_empty() && argv.iter().all(|arg| nonempty_str(Some(arg)))
                })
        });
        require(argv_ok, "command argv required")?;
        let item = command
            .as_object()
            .ok_or_else(|| ToolError::invalid("command argv required"))?;
        require(
            nonempty_str(item.get("cwd"))
                && item
                    .get("exit_code")
                    .and_then(Value::as_exact_int)
                    .is_some(),
            "command cwd/exit_code required",
        )?;
    }
    require(
        field("environment")
            .as_object()
            .is_some_and(|environment| !environment.is_empty()),
        "environment identity required",
    )?;
    let artifacts = field("artifacts")
        .as_array()
        .ok_or_else(|| ToolError::invalid("artifact list required"))?;
    for artifact in artifacts {
        let valid = artifact.as_object().is_some_and(|item| {
            item.get("path").is_some_and(Value::truthy)
                && item
                    .get("sha256")
                    .and_then(Value::as_str)
                    .is_some_and(|digest| DIGEST.is_match(digest))
        });
        require(valid, "artifact path/computed SHA-256 required")?;
    }
    require(
        field("risks").as_array().is_some()
            && field("blockers").as_array().is_some()
            && nonempty_str(Some(field("next_action"))),
        "risks/blockers/next_action required",
    )?;
    Ok(position)
}

/// `reconcile(output, manifest, receipt)` with an injectable replacement.
pub fn reconcile(
    output: &Path,
    manifest: &mut Value,
    receipt: &Value,
    replace: ReplaceFn<'_>,
) -> ToolResult<()> {
    let position = validate_receipt(receipt, manifest)?;
    let data = json_bytes(receipt)?;
    let hashed = digest(&data);
    let directory = output.join("receipts");
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && directory.is_dir() => {}
        Err(error) => return Err(error.into()),
    }
    let path = directory.join(format!("{hashed}.json"));
    require(!fsx::is_symlink(&path), "receipt output symlink forbidden")?;
    if path.exists() {
        require(fs::read(&path)? == data, "receipt digest collision")?;
    } else {
        atomic_write(&path, &data, replace)?;
    }
    let evidence = crate::pyobj! {
        "kind" => "candidate_receipt",
        "path" => format!("receipts/{hashed}.json"),
        "sha256" => hashed.as_str(),
        "revision" => receipt.index_key("head_sha")?.clone(),
        "observed_utc" => now(),
        "trust" => "worker_report_unverified",
    };
    let packet = packet_mut(manifest, position)?;
    let lane = packet.get("lane").cloned();
    let items = packet
        .get_mut("evidence")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| ToolError::type_error("evidence must be a list"))?;
    let mut seen = false;
    for item in items.iter() {
        if item.get_value("sha256")? == &Value::Str(hashed.clone()) {
            seen = true;
            break;
        }
    }
    if !seen {
        items.push(evidence);
    }
    packet.insert("status", "review");
    let map = manifest
        .as_object_mut()
        .ok_or_else(|| ToolError::type_error("manifest must be an object"))?;
    if let Some(Value::Array(lanes)) = map.get_mut("lanes") {
        for entry in lanes.iter_mut() {
            let lane = lane
                .as_ref()
                .ok_or_else(|| ToolError::Key("lane".to_owned()))?;
            if entry.index_key("id")? == lane
                && let Some(entry) = entry.as_object_mut()
            {
                entry.insert("status", "review");
            }
        }
    }
    map.insert("updated_utc", now());
    validate_manifest(manifest)?;
    atomic_write(
        &output.join("rollout.json"),
        &json_bytes(manifest)?,
        replace,
    )
}

/// `admiralty rollout packets` arguments (same flags as the Python argparse).
#[derive(clap::Args, Debug, Clone)]
pub struct PacketsArgs {
    /// Admiralty checkout (default: the current directory).
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// Command.
    #[command(subcommand)]
    pub command: PacketsCommand,
}

/// `rollout packets` subcommands.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum PacketsCommand {
    /// Render capsules and publish `rollout.json`.
    Export {
        /// Dedicated owned directory outside repository.
        #[arg(long)]
        out: PathBuf,
        /// Base reference resolved to a full commit ID.
        #[arg(long = "base-ref")]
        base_ref: String,
        /// Project identifier.
        #[arg(long = "project-id", default_value = "admiralty-v2")]
        project_id: String,
        /// Repository URL metadata.
        #[arg(
            long = "repository-url",
            default_value = "https://github.com/neverhuman/admiralty"
        )]
        repository_url: String,
    },
    /// Validate the manifest and capsule digests.
    Check {
        /// Dedicated owned directory outside repository.
        #[arg(long)]
        out: PathBuf,
    },
    /// Record an explicit claim of a ready packet.
    Claim {
        /// Dedicated owned directory outside repository.
        #[arg(long)]
        out: PathBuf,
        /// Packet ID.
        #[arg(long)]
        packet: String,
        /// Claim owner.
        #[arg(long)]
        owner: String,
        /// Lane identity.
        #[arg(long)]
        lane: String,
        /// Node identity.
        #[arg(long)]
        node: Option<String>,
        /// `codex/...` branch.
        #[arg(long)]
        branch: String,
    },
    /// Import a worker candidate receipt (never trusted).
    Reconcile {
        /// Dedicated owned directory outside repository.
        #[arg(long)]
        out: PathBuf,
        /// Candidate receipt JSON.
        #[arg(long)]
        receipt: PathBuf,
    },
}

impl PacketsCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Export { .. } => "export",
            Self::Check { .. } => "check",
            Self::Claim { .. } => "claim",
            Self::Reconcile { .. } => "reconcile",
        }
    }

    fn out(&self) -> &Path {
        match self {
            Self::Export { out, .. }
            | Self::Check { out }
            | Self::Claim { out, .. }
            | Self::Reconcile { out, .. } => out,
        }
    }
}

fn execute(args: &PacketsArgs) -> ToolResult<String> {
    let raw_repo = match &args.repo {
        Some(repo) => repo.clone(),
        None => std::env::current_dir()?,
    };
    let repo = fsx::resolve(&fsx::absolute(&fsx::expanduser(&raw_repo))?);
    let base = match &args.command {
        PacketsCommand::Export { base_ref, .. } => Some(resolve_base(&repo, base_ref)?),
        _ => None,
    };
    let creating = matches!(args.command, PacketsCommand::Export { .. });
    let output = owned_output(args.command.out(), &repo, creating)?;
    let _lock = output_lock(&output)?;
    let manifest = match &args.command {
        PacketsCommand::Export {
            project_id,
            repository_url,
            ..
        } => export(
            &repo,
            &output,
            base.as_deref().unwrap_or_default(),
            &ExportOptions {
                project_id,
                repository_url,
                replace: &fsx::os_replace,
            },
        )?,
        command => {
            let mut manifest = read_json(&output.join("rollout.json"))?;
            validate_manifest(&manifest)?;
            verify_capsules(&output, &manifest)?;
            match command {
                PacketsCommand::Claim {
                    packet,
                    owner,
                    lane,
                    node,
                    branch,
                    ..
                } => {
                    claim(&mut manifest, packet, owner, lane, node.as_deref(), branch)?;
                    atomic_write(
                        &output.join("rollout.json"),
                        &json_bytes(&manifest)?,
                        &fsx::os_replace,
                    )?;
                }
                PacketsCommand::Reconcile { receipt, .. } => {
                    let receipt = read_json(receipt)?;
                    reconcile(&output, &mut manifest, &receipt, &fsx::os_replace)?;
                }
                _ => {}
            }
            manifest
        }
    };
    verify_capsules(&output, &manifest)?;
    let packets = pyjson::py_iter(manifest.index_key("packets")?)?;
    let ready: Vec<Value> = packets
        .iter()
        .filter(|packet| {
            packet
                .get("status")
                .ok()
                .flatten()
                .is_some_and(|status| status == &Value::Str("ready".to_owned()))
        })
        .filter_map(|packet| packet.get("id").ok().flatten().cloned())
        .collect();
    let summary = crate::pyobj! {
        "manifest" => output.join("rollout.json").display().to_string(),
        "packets" => packets.len(),
        "ready" => Value::Array(ready),
        "command" => args.command.name(),
    };
    Ok(pyjson::dumps_default(&summary))
}

/// Run `rollout packets`; returns the process exit code.
pub fn run(args: &PacketsArgs) -> i32 {
    match execute(args) {
        Ok(summary) => {
            println!("{summary}");
            0
        }
        Err(error) => {
            eprintln!("rollout_packets: {error}");
            2
        }
    }
}

#[cfg(test)]
#[path = "rollout_packets_tests.rs"]
mod tests;
