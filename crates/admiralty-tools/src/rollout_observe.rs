//! Temporary read-only lane observer. Never dispatches, types input or accepts work.
//!
//! Port of the retired `tools/rollout_observe.py`. It joins the integration
//! owner's rollout template with captain-written lane reports, optionally
//! corroborates Screen session identity with the fixed argv `screen -ls`, and
//! optionally reads owned worktree *metadata* (never content) under a private
//! 0700 prefix. Output is published atomically and bounded to one MiB.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use regex::Regex;
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use sha2::{Digest, Sha256};

use crate::error::{ToolError, ToolResult, require};
use crate::fsx::{self, ReplaceFn};
use crate::proc;
use crate::pyjson::{self, DumpOptions, LoadOptions, Map, NULL, Value};
use crate::pytime;

/// Input and output byte bound (one MiB).
pub const MAX_BYTES: usize = 1024 * 1024;
/// Maximum lanes in a report, template or combined output.
pub const MAX_LANES: usize = 200;
/// Maximum template packets.
pub const MAX_PACKETS: usize = 2000;
/// Producer recorded under `observer.producer`.
pub const PRODUCER: &str = "admiralty.legacy_rollout_observer/v1";

static ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\z").expect("static identifier pattern")
});
static HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})\z").expect("static object-id pattern")
});
static SESSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:([0-9]+)\.)?([A-Za-z0-9_.-]+)\z").expect("static session pattern")
});
static SCREEN_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(\d+)\.([A-Za-z0-9_.-]+)\s").expect("static screen pattern")
});

/// True when `value` is a safe stable identifier (`ID.fullmatch`).
pub fn is_safe_id(value: &str) -> bool {
    ID.is_match(value)
}

/// Worktree witness bounds (Python module constants).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// `MAX_SCOPE_PATHS`.
    pub max_scope_paths: usize,
    /// `MAX_SCOPE_ENTRIES`.
    pub max_scope_entries: usize,
    /// `MAX_SCOPE_DEPTH`.
    pub max_scope_depth: usize,
    /// `MAX_WORKTREE_PROBES`.
    pub max_worktree_probes: usize,
    /// `WORKTREE_BUDGET_SECONDS`.
    pub worktree_budget: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_scope_paths: 64,
            max_scope_entries: 2000,
            max_scope_depth: 16,
            max_worktree_probes: 32,
            worktree_budget: Duration::from_secs(5),
        }
    }
}

/// Result of the fixed `screen -ls` probe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreenSnapshot {
    /// Whether the probe produced a known answer.
    pub available: bool,
    /// Session name to PID, restricted to the prefix.
    pub sessions: BTreeMap<String, i128>,
    /// Human-readable probe status.
    pub reason: String,
}

/// Descriptor-bound HEAD probe signature (`_bounded_git_head`).
pub type HeadProbe<'a> = &'a dyn Fn(BorrowedFd<'_>, Duration) -> ToolResult<String>;

/// `timestamp(value)`: timezone-aware ISO value converted to UTC.
pub fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    let text = value.as_str()?;
    pytime::fromisoformat(&text.replace('Z', "+00:00"))?.to_utc()
}

fn iso(value: &DateTime<Utc>) -> String {
    pytime::isoformat_seconds(value)
}

fn has_control(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| *byte < 32)
}

/// `safe_path(raw)`: no traversal, no control characters, no user symlinks.
pub fn safe_path(raw: &Path) -> ToolResult<PathBuf> {
    let raw = fsx::expanduser(raw);
    let (_, parts) = fsx::parts(&raw);
    require(
        !parts.iter().any(|part| part == "..") && !has_control(raw.as_os_str().as_bytes()),
        "unsafe path",
    )?;
    let result = fsx::absolute(&raw)?;
    let aliases = [("/tmp", "/private/tmp"), ("/var", "/private/var")];
    for part in fsx::self_and_parents(&result) {
        if fsx::is_symlink(&part) {
            let allowed = aliases.iter().any(|(alias, target)| {
                part == Path::new(alias)
                    && fsx::resolve(&part) == Path::new(target)
                    && std::fs::symlink_metadata(&part).is_ok_and(|meta| meta.uid() == 0)
            });
            require(allowed, "symlink paths are forbidden")?;
        }
    }
    Ok(fsx::resolve(&result))
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[allow(clippy::unnecessary_cast)]
fn stat_mtime(stat: &rustix::fs::Stat) -> f64 {
    pytime::stat_time(stat.st_mtime as i64, stat.st_mtime_nsec as i64)
}

/// `read_owned(path)`: bounded, owned, regular, no-follow JSON input.
pub fn read_owned(path: &Path) -> ToolResult<(Value, Value)> {
    let path = safe_path(path)?;
    let descriptor = rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(descriptor);
    let info = file.metadata()?;
    require(
        info.file_type().is_file() && info.uid() == fsx::getuid(),
        "input must be a regular owned JSON file",
    )?;
    require(info.len() <= MAX_BYTES as u64, "input exceeds one MiB")?;
    let mut data = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut data)?;
    require(data.len() <= MAX_BYTES, "input exceeds one MiB")?;
    let value = pyjson::loads_bytes(
        &data,
        LoadOptions {
            reject_duplicates: true,
            reject_constants: true,
        },
    )
    .map_err(|_| ToolError::invalid("input is not bounded valid JSON"))?;
    let mtime = pytime::stat_time(info.mtime(), info.mtime_nsec());
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = crate::pyobj! {
        "filename" => name,
        "sha256" => sha256_hex(&data),
        "mtime_utc" => pytime::iso_from_timestamp(mtime)?,
    };
    Ok((value, meta))
}

/// Only independent probe: fixed argv `screen -ls`. Never runs JSON-supplied text.
pub fn screen_snapshot(prefix: &str) -> ToolResult<ScreenSnapshot> {
    require(ID.is_match(prefix), "unsafe session prefix")?;
    let unavailable = |reason: &str| ScreenSnapshot {
        available: false,
        sessions: BTreeMap::new(),
        reason: reason.to_owned(),
    };
    let result = proc::run_bounded(
        Command::new("screen").arg("-ls"),
        None,
        Duration::from_secs(5),
        false,
        Some(65537),
    );
    let Ok(captured) = result else {
        return Ok(unavailable("screen probe unavailable"));
    };
    if captured.stdout.len() > 65536 {
        return Ok(unavailable("screen output exceeds bound"));
    }
    let output = String::from_utf8_lossy(&captured.stdout);
    let mut sessions = BTreeMap::new();
    for capture in SCREEN_LINE.captures_iter(&output) {
        let name = &capture[2];
        if name.starts_with(prefix)
            && let Ok(pid) = capture[1].parse::<i128>()
        {
            sessions.insert(name.to_owned(), pid);
        }
    }
    let known = captured.status.success()
        || output.contains("No Sockets found")
        || output.contains("No sockets found");
    Ok(ScreenSnapshot {
        available: known,
        sessions,
        reason: if known {
            "screen -ls".to_owned()
        } else {
            "screen probe unavailable".to_owned()
        },
    })
}

/// `text(value, limit)`: control characters removed, truncated to `limit`.
pub fn text(value: &Value, limit: usize) -> String {
    match value {
        Value::Str(raw) => raw
            .chars()
            .filter(|character| (*character as u32) >= 32)
            .take(limit)
            .collect(),
        _ => String::new(),
    }
}

fn lane_id(value: &Value) -> ToolResult<String> {
    let map = value
        .as_object()
        .ok_or_else(|| ToolError::invalid("lane must be an object"))?;
    let identity = map
        .get("id")
        .or_else(|| map.get("lane"))
        .or_else(|| map.get("screen"));
    match identity.and_then(Value::as_str) {
        Some(identity) if ID.is_match(identity) => Ok(identity.to_owned()),
        _ => Err(ToolError::invalid("lane requires a safe stable id")),
    }
}

fn lane_rows(value: Option<&Value>) -> ToolResult<(Vec<Value>, Value)> {
    let Some(value) = value else {
        return Ok((Vec::new(), Value::Null));
    };
    let rows = match value {
        Value::Object(map) => map.get("lanes").unwrap_or(&NULL),
        other => other,
    };
    let rows = match rows {
        Value::Array(items) if items.len() <= MAX_LANES => items.clone(),
        _ => {
            return Err(ToolError::invalid(
                "lane report must contain at most 200 lanes",
            ));
        }
    };
    let updated = match value {
        Value::Object(map) => map
            .get("updated_utc")
            .or_else(|| map.get("observed_utc"))
            .cloned()
            .unwrap_or(Value::Null),
        _ => Value::Null,
    };
    for row in &rows {
        lane_id(row)?;
    }
    Ok((rows, updated))
}

fn owned_directory(parent: BorrowedFd<'_>, parts: &[OsString]) -> ToolResult<OwnedFd> {
    let mut descriptor = rustix::io::fcntl_dupfd_cloexec(parent, 0)?;
    for part in parts {
        let child = rustix::fs::openat(
            &descriptor,
            part.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        descriptor = child;
        require(
            rustix::fs::fstat(&descriptor)?.st_uid == fsx::getuid(),
            "directory is not owned",
        )?;
    }
    Ok(descriptor)
}

/// Fixed argv and sanitized environment of the HEAD probe.
pub fn git_head_invocation(
    target: &str,
    environ: impl IntoIterator<Item = (OsString, OsString)>,
) -> (Vec<String>, BTreeMap<OsString, OsString>) {
    let mut environment: BTreeMap<OsString, OsString> = environ
        .into_iter()
        .filter(|(key, _)| !key.as_bytes().starts_with(b"GIT_"))
        .collect();
    for (key, value) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_WORK_TREE", target),
        ("GIT_PAGER", "cat"),
    ] {
        environment.insert(OsString::from(key), OsString::from(value));
    }
    let argv = [
        "git",
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-C",
        target,
        "rev-parse",
        "--verify",
        "HEAD",
    ]
    .map(str::to_owned)
    .to_vec();
    (argv, environment)
}

/// Spawner used by the HEAD probe (replaceable in tests).
pub type GitSpawn<'a> = &'a dyn Fn(&[String], &BTreeMap<OsString, OsString>) -> io::Result<Child>;

fn spawn_git(argv: &[String], environment: &BTreeMap<OsString, OsString>) -> io::Result<Child> {
    Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

/// Only Git probe: `rev-parse HEAD`. No status, filters, hooks or file output.
pub fn bounded_git_head(descriptor: BorrowedFd<'_>, timeout: Duration) -> ToolResult<String> {
    bounded_git_head_with(descriptor, timeout, &spawn_git)
}

/// [`bounded_git_head`] with an injectable spawner.
pub fn bounded_git_head_with(
    descriptor: BorrowedFd<'_>,
    timeout: Duration,
    spawn: GitSpawn<'_>,
) -> ToolResult<String> {
    // Linux procfs preserves directory identity across exec. A duplicate
    // without close-on-exec is the descriptor passed to the child (Python
    // `pass_fds`); it is closed in this process right after spawning.
    let inherited = rustix::io::dup(descriptor)?;
    let target = format!("/proc/self/fd/{}", inherited.as_raw_fd());
    require(
        Path::new("/proc/self/fd").is_dir() && Path::new(&target).exists(),
        "descriptor-bound Git unavailable on this platform",
    )?;
    let (argv, environment) = git_head_invocation(&target, std::env::vars_os());
    let child = spawn(&argv, &environment)?;
    drop(inherited);
    let deadline = Instant::now() + timeout;
    let (output, status) = proc::read_bounded_then_wait(
        child,
        256,
        deadline,
        "Git HEAD probe timed out",
        "Git HEAD output exceeds bound",
    )?;
    require(status.success(), "Git HEAD unavailable")?;
    require(output.is_ascii(), "Git HEAD is not ASCII")?;
    let value = String::from_utf8_lossy(&output);
    let value = pyjson::py_strip(&value);
    require(
        HEAD.is_match(value),
        "Git HEAD is not a complete object identity",
    )?;
    Ok(value.to_owned())
}

/// Opt-in metadata witness confined to an existing owned private prefix.
pub struct WorktreeWitness<'a> {
    configured: (bool, Vec<OsString>),
    root: (bool, Vec<OsString>),
    descriptor: OwnedFd,
    deadline: Instant,
    probes: usize,
    limits: Limits,
    head_probe: HeadProbe<'a>,
}

struct Scan {
    files: BTreeSet<Vec<OsString>>,
    visited: HashSet<Vec<OsString>>,
    latest: Option<f64>,
    skipped: usize,
    bounded: bool,
    deadline: Instant,
    limits: Limits,
}

impl Scan {
    fn scan(
        &mut self,
        parent: BorrowedFd<'_>,
        name: &OsStr,
        relative: Vec<OsString>,
        depth: usize,
        recursive: bool,
    ) {
        if self.visited.contains(&relative) {
            return;
        }
        if self.visited.len() >= self.limits.max_scope_entries || Instant::now() >= self.deadline {
            self.bounded = true;
            return;
        }
        self.visited.insert(relative.clone());
        if self
            .scan_entry(parent, name, relative, depth, recursive)
            .is_err()
        {
            self.skipped += 1;
        }
    }

    fn scan_entry(
        &mut self,
        parent: BorrowedFd<'_>,
        name: &OsStr,
        relative: Vec<OsString>,
        depth: usize,
        recursive: bool,
    ) -> ToolResult<()> {
        let info = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let kind = FileType::from_raw_mode(info.st_mode);
        if info.st_uid != fsx::getuid() || kind == FileType::Symlink {
            self.skipped += 1;
        } else if kind == FileType::RegularFile {
            self.files.insert(relative);
            let mtime = stat_mtime(&info);
            self.latest = Some(self.latest.map_or(mtime, |latest| latest.max(mtime)));
        } else if kind == FileType::Directory && recursive {
            if depth >= self.limits.max_scope_depth {
                self.bounded = true;
                return Ok(());
            }
            let child = owned_directory(parent, &[name.to_os_string()])?;
            let entries = Dir::read_from(&child)?;
            for entry in entries {
                if self.bounded {
                    break;
                }
                let entry = entry?;
                let entry_name = OsStr::from_bytes(entry.file_name().to_bytes()).to_os_string();
                if entry_name == "." || entry_name == ".." {
                    continue;
                }
                let mut next = relative.clone();
                next.push(entry_name.clone());
                self.scan(child.as_fd(), &entry_name, next, depth + 1, true);
            }
        } else {
            self.skipped += 1;
        }
        Ok(())
    }
}

fn strip_prefix(path: &[OsString], prefix: &[OsString]) -> Option<Vec<OsString>> {
    path.strip_prefix(prefix).map(<[OsString]>::to_vec)
}

impl<'a> WorktreeWitness<'a> {
    /// Open the configured root (absolute, owned, mode 0700, no symlinks).
    pub fn open(raw: &Path, limits: Limits, head_probe: HeadProbe<'a>) -> ToolResult<Self> {
        let configured = fsx::parts(raw);
        require(configured.0, "worktree root must be absolute")?;
        let root = safe_path(raw)?;
        let descriptor = rustix::fs::open(
            &root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let info = rustix::fs::fstat(&descriptor)?;
        if info.st_uid != fsx::getuid() || info.st_mode & 0o077 != 0 {
            return Err(ToolError::invalid(
                "worktree root must be owned and private (0700)",
            ));
        }
        Ok(Self {
            configured,
            root: fsx::parts(&root),
            descriptor,
            deadline: Instant::now() + limits.worktree_budget,
            probes: 0,
            limits,
            head_probe,
        })
    }

    /// Observe one reported lane's worktree metadata.
    pub fn observe(&mut self, row: &Value, now: &DateTime<Utc>) -> Value {
        let mut result = Map::new();
        result.insert("source", "owned_worktree_metadata/v1");
        result.insert("state", "unavailable");
        result.insert("observed_utc", iso(now));
        result.insert(
            "assurance",
            "Metadata only; no agent liveness, task progress or acceptance established.",
        );
        if self.probes >= self.limits.max_worktree_probes || Instant::now() >= self.deadline {
            result.insert("state", "deferred");
            return Value::Object(result);
        }
        let raw = match row.get("worktree") {
            Ok(Some(Value::Str(raw))) if !raw.is_empty() => raw.clone(),
            _ => {
                result.insert("state", "not_reported");
                return Value::Object(result);
            }
        };
        if self.observe_owned(row, &raw, &mut result).is_err() {
            result.insert("state", "unsafe");
        }
        Value::Object(result)
    }

    fn observe_owned(&mut self, row: &Value, raw: &str, result: &mut Map) -> ToolResult<()> {
        let candidate = fsx::parts(Path::new(raw));
        require(
            raw.chars().count() <= 4096
                && candidate.0
                && !candidate.1.iter().any(|part| part == "..")
                && !has_control(raw.as_bytes()),
            "unsafe worktree",
        )?;
        let relative = strip_prefix(&candidate.1, &self.configured.1)
            .or_else(|| strip_prefix(&candidate.1, &self.root.1))
            .ok_or_else(|| ToolError::invalid("worktree outside configured prefix"))?;
        let descriptor = owned_directory(self.descriptor.as_fd(), &relative)?;
        self.probes += 1;
        let empty = Value::Array(Vec::new());
        let single;
        let paths = match row.get("candidate_paths")? {
            Some(paths) => paths,
            None => match row.get("candidate_path")? {
                Some(path) => {
                    single = Value::Array(vec![path.clone()]);
                    &single
                }
                None => &empty,
            },
        };
        let paths = match paths {
            Value::Array(items)
                if items.len() <= self.limits.max_scope_paths
                    && items.iter().all(|item| item.as_str().is_some()) =>
            {
                items
            }
            _ => {
                return Err(ToolError::invalid(
                    "bounded explicit candidate paths required",
                ));
            }
        };
        let mut scopes: Vec<(Vec<OsString>, bool)> = Vec::new();
        for raw_path in paths.iter().filter_map(Value::as_str) {
            let recursive = raw_path.ends_with("/**");
            let value = if recursive {
                &raw_path[..raw_path.len() - 3]
            } else {
                raw_path
            };
            require(
                !value.is_empty()
                    && value.chars().count() <= 4096
                    && !value.chars().any(|c| "*?[]\\".contains(c))
                    && !has_control(value.as_bytes()),
                "unsafe candidate path",
            )?;
            let (absolute, mut parts) = fsx::parts(Path::new(value));
            if absolute {
                parts = strip_prefix(&parts, &candidate.1)
                    .ok_or_else(|| ToolError::type_error("path is not in the subpath"))?;
            }
            require(
                !parts.is_empty()
                    && parts.len() <= self.limits.max_scope_depth
                    && parts.iter().all(|part| part != "." && part != ".."),
                "candidate path escapes worktree",
            )?;
            scopes.push((parts, recursive));
        }
        scopes.sort_by(|a, b| (!a.1, a.0.len(), &a.0).cmp(&(!b.1, b.0.len(), &b.0)));
        let mut scan = Scan {
            files: BTreeSet::new(),
            visited: HashSet::new(),
            latest: None,
            skipped: 0,
            bounded: false,
            deadline: self.deadline,
            limits: self.limits,
        };
        for (parts, recursive) in &scopes {
            if scan.bounded {
                break;
            }
            let Some((name, parents)) = parts.split_last() else {
                continue;
            };
            match owned_directory(descriptor.as_fd(), parents) {
                Ok(parent) => scan.scan(parent.as_fd(), name, parts.clone(), 0, *recursive),
                Err(_) => scan.skipped += 1,
            }
        }
        let latest = match scan.latest {
            Some(latest) => Value::Str(pytime::iso_from_timestamp(latest)?),
            None => Value::Null,
        };
        result.insert("state", if scan.bounded { "bounded" } else { "observed" });
        result.insert("owned_scope_files", scan.files.len());
        result.insert("scanned_entries", scan.visited.len());
        result.insert("scope_paths", scopes.len());
        result.insert("skipped_entries", scan.skipped);
        result.insert("bounded", scan.bounded);
        result.insert("latest_owned_path_mtime_utc", latest);
        // Presence of a local .git marker prevents repository discovery from
        // silently using an unrelated parent checkout. Never follow a link.
        let head = (|| -> ToolResult<(String, Value)> {
            let marker = rustix::fs::statat(&descriptor, ".git", AtFlags::SYMLINK_NOFOLLOW)?;
            let kind = FileType::from_raw_mode(marker.st_mode);
            require(
                marker.st_uid == fsx::getuid()
                    && (kind == FileType::RegularFile || kind == FileType::Directory),
                "unsafe git marker",
            )?;
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            require(
                !remaining.is_zero(),
                "worktree observation budget exhausted",
            )?;
            let head =
                (self.head_probe)(descriptor.as_fd(), remaining.min(Duration::from_secs(1)))?;
            let matches = match row.get("head_sha")? {
                Some(Value::Str(reported)) if HEAD.is_match(reported) => {
                    Value::Bool(&head == reported)
                }
                _ => Value::Null,
            };
            Ok((head, matches))
        })();
        match head {
            Ok((head, matches)) => {
                result.insert("head_sha", head);
                result.insert("reported_head_matches", matches);
            }
            Err(_) => {
                result.insert("head_state", "unavailable");
            }
        }
        Ok(())
    }
}

/// Merge options (Python keyword arguments of `merge`).
pub struct MergeOptions<'a> {
    /// Observation time.
    pub now: DateTime<Utc>,
    /// Session prefix (`None` disables Screen corroboration).
    pub prefix: Option<&'a str>,
    /// Screen probe result.
    pub screens: Option<&'a ScreenSnapshot>,
    /// Staleness bound in seconds.
    pub stale_after: i64,
    /// Opt-in owned worktree prefix.
    pub worktree_root: Option<&'a Path>,
    /// Witness bounds.
    pub limits: Limits,
    /// HEAD probe implementation.
    pub head_probe: HeadProbe<'a>,
}

impl MergeOptions<'_> {
    /// Defaults: no prefix, no screens, 120 s staleness, no witness.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now,
            prefix: None,
            screens: None,
            stale_after: 120,
            worktree_root: None,
            limits: Limits::default(),
            head_probe: &bounded_git_head,
        }
    }
}

/// `merge(template, reports, source_meta, lane_meta, ...)`.
pub fn merge(
    template: &Value,
    reports: Option<&Value>,
    source_meta: &Value,
    lane_meta: &Value,
    options: &MergeOptions<'_>,
) -> ToolResult<Value> {
    let mut witness = match options.worktree_root {
        Some(root) => Some(WorktreeWitness::open(
            root,
            options.limits,
            options.head_probe,
        )?),
        None => None,
    };
    merge_inner(
        template,
        reports,
        source_meta,
        lane_meta,
        options,
        witness.as_mut(),
    )
}

fn row_key(item: &Value) -> (String, String) {
    let time = item
        .as_object()
        .and_then(|map| map.get("updated_utc").or_else(|| map.get("observed_utc")))
        .and_then(timestamp)
        .map_or_else(|| pytime::MIN_ISO.to_owned(), |value| iso(&value));
    (
        time,
        pyjson::dumps(item, &DumpOptions::PYTHON.sorted()).unwrap_or_default(),
    )
}

fn get_or<'v>(map: &'v Map, key: &str, default: &'v Value) -> &'v Value {
    map.get(key).unwrap_or(default)
}

#[allow(clippy::too_many_lines)]
fn merge_inner(
    template: &Value,
    reports: Option<&Value>,
    source_meta: &Value,
    lane_meta: &Value,
    options: &MergeOptions<'_>,
    mut witness: Option<&mut WorktreeWitness<'_>>,
) -> ToolResult<Value> {
    let template_map = template.as_object();
    require(
        template_map.and_then(|map| map.get("schema_version")) == Some(&Value::Int(1)),
        "template schema_version 1 required",
    )?;
    let template_map = template_map.ok_or_else(|| ToolError::invalid("template required"))?;
    require(
        template_map
            .get("project")
            .and_then(Value::as_object)
            .and_then(|project| project.get("id"))
            .is_some_and(Value::truthy),
        "template project required",
    )?;
    require(
        timestamp(template_map.get("updated_utc").unwrap_or(&NULL)).is_some(),
        "template updated_utc requires timezone",
    )?;
    let packets = match template_map.get("packets") {
        Some(Value::Array(items)) if items.len() <= MAX_PACKETS => items,
        _ => return Err(ToolError::invalid("bounded template packets required")),
    };
    let mut packet_ids = HashSet::new();
    for packet in packets {
        let identity = packet
            .as_object()
            .and_then(|map| map.get("id"))
            .and_then(Value::as_str)
            .filter(|identity| ID.is_match(identity))
            .ok_or_else(|| ToolError::invalid("unsafe packet id"))?;
        require(packet_ids.insert(identity), "duplicate packet id")?;
    }
    let empty = Value::Array(Vec::new());
    let original = match get_or(template_map, "lanes", &empty) {
        Value::Array(items) if items.len() <= MAX_LANES => items,
        _ => return Err(ToolError::invalid("bounded template lanes required")),
    };
    let (rows, report_time) = lane_rows(reports)?;
    let report_time = if report_time.truthy() {
        report_time
    } else {
        lane_meta.or_empty_get("mtime_utc")?.clone()
    };
    let source_time = template_map
        .get("updated_utc")
        .cloned()
        .unwrap_or(Value::Null);
    let mut candidates: BTreeMap<String, (bool, Value)> = BTreeMap::new();
    let mut duplicate_ids = HashSet::new();
    // Template supplies pending lanes; reports override only observational fields.
    for row in original {
        let identity = lane_id(row)?;
        require(
            !candidates.contains_key(&identity),
            "duplicate template lane id",
        )?;
        candidates.insert(identity, (false, row.clone()));
    }
    let mut selected: HashMap<String, Value> = HashMap::new();
    for row in &rows {
        let identity = lane_id(row)?;
        let replace = match selected.get(&identity) {
            Some(old) => {
                if old != row {
                    duplicate_ids.insert(identity.clone());
                }
                // Prefer newest explicit source time, then deterministic bytes.
                row_key(row) > row_key(old)
            }
            None => true,
        };
        if replace {
            selected.insert(identity, row.clone());
        }
    }
    for (identity, row) in selected {
        candidates.insert(identity, (true, row));
    }
    require(candidates.len() <= MAX_LANES, "combined lanes exceed bound")?;
    let mut output = template_map.clone();
    output.insert("lanes", Value::Array(Vec::new()));
    let now = options.now;
    let prefix = options.prefix.filter(|prefix| !prefix.is_empty());
    let mut lanes = Vec::new();
    for (identity, (reported, row_value)) in &candidates {
        let row = row_value
            .as_object()
            .ok_or_else(|| ToolError::invalid("lane must be an object"))?;
        let raw_time = if *reported {
            get_or(
                row,
                "updated_utc",
                get_or(row, "observed_utc", &report_time),
            )
            .clone()
        } else {
            get_or(row, "updated_utc", &source_time).clone()
        };
        let age = timestamp(&raw_time).map(|updated| {
            (now - updated)
                .num_microseconds()
                .map_or(f64::INFINITY, |micros| micros as f64 / 1e6)
        });
        let freshness = match age {
            None => "unknown",
            Some(age) if age < -30.0 => "future",
            Some(age) if age > options.stale_after as f64 => "stale",
            Some(_) => "fresh",
        };
        let mut state = if *reported { freshness } else { "pending" };
        let session_default = if row.get("agent_session").is_some_and(Value::truthy) {
            Value::Str(String::new())
        } else {
            Value::Str(identity.clone())
        };
        let session = get_or(row, "screen", get_or(row, "session", &session_default));
        let session = session.as_str().unwrap_or("").to_owned();
        let captures = SESSION.captures(&session);
        let name = captures
            .as_ref()
            .and_then(|capture| capture.get(2))
            .map_or("", |m| m.as_str())
            .to_owned();
        let mut reported_pid = row.get("screen_pid").cloned().unwrap_or(Value::Null);
        if reported_pid.is_null()
            && let Some(digits) = captures.as_ref().and_then(|capture| capture.get(1))
        {
            reported_pid = digits
                .as_str()
                .parse::<i128>()
                .map(Value::Int)
                .map_err(|_| ToolError::invalid("invalid screen PID"))?;
        }
        if !reported_pid.is_null() {
            require(
                reported_pid
                    .as_exact_int()
                    .is_some_and(|pid| 0 < pid && pid < (1i128 << 31)),
                "invalid screen PID",
            )?;
        }
        let mut screen_state = "not_checked";
        let mut actual_pid: Option<i128> = None;
        if session.is_empty() {
            screen_state = "not_exposed";
        } else if let (Some(prefix), Some(screens)) = (prefix, options.screens) {
            if !name.starts_with(prefix) {
                screen_state = "out_of_scope";
            } else if !screens.available {
                screen_state = "unknown";
            } else {
                actual_pid = screens.sessions.get(&name).copied();
                screen_state = match (actual_pid, reported_pid.as_exact_int()) {
                    (None, _) => "absent",
                    (Some(actual), Some(reported)) if actual != reported => "identity_mismatch",
                    _ => "present",
                };
                if *reported && matches!(screen_state, "absent" | "identity_mismatch") {
                    state = "orphaned";
                }
            }
        }
        if duplicate_ids.contains(identity) {
            state = "pending";
        }
        let mut reported_status = text(get_or(row, "status", get_or(row, "state", &NULL)), 160);
        if reported_status.is_empty() {
            reported_status = "unknown".to_owned();
        }
        let mut status = format!("{state}; reported {reported_status}; screen {screen_state}");
        let model = text(row.get("model").unwrap_or(&NULL), 80);
        let effort = text(row.get("effort").unwrap_or(&NULL), 30);
        if !model.is_empty() {
            let suffix = format!("; {model} {effort}");
            status.push_str(suffix.trim_end_matches(pyjson::is_py_whitespace));
        }
        let reserved = get_or(row, "reserved_cpus", &empty);
        let reserved_items = match reserved {
            Value::Array(items)
                if items.len() <= 128
                    && items.iter().all(|cpu| {
                        cpu.as_exact_int()
                            .is_some_and(|cpu| (0..1_048_576).contains(&cpu))
                    }) =>
            {
                items
            }
            _ => return Err(ToolError::invalid("invalid reserved CPU list")),
        };
        let pinned = row.get("cpus_pinned").unwrap_or(&NULL);
        if !reserved_items.is_empty() {
            let shown: Vec<String> = reserved_items.iter().take(16).map(pyjson::py_str).collect();
            status.push_str("; CPU reserved ");
            status.push_str(&shown.join(","));
            status.push_str(match pinned {
                Value::Bool(true) => " pinned",
                Value::Bool(false) => " NOT pinned",
                _ => " pinning unknown",
            });
        }
        if freshness == "stale" && state != "stale" {
            status.push_str("; source stale");
        }
        if duplicate_ids.contains(identity) {
            status.push_str("; conflicting duplicate reports");
        }
        let activity = match witness.as_deref_mut() {
            Some(witness) if *reported => Some(witness.observe(row_value, &now)),
            _ => None,
        };
        if let Some(Value::Object(activity)) = &activity {
            let mut suffix = String::new();
            if activity.get("scope_paths").is_some_and(Value::truthy)
                && activity.contains_key("owned_scope_files")
            {
                suffix = format!(
                    "; owned scope files {}",
                    pyjson::py_str(activity.get("owned_scope_files").unwrap_or(&NULL))
                );
                if let Some(latest) = activity
                    .get("latest_owned_path_mtime_utc")
                    .filter(|latest| latest.truthy())
                {
                    suffix.push_str("; latest ");
                    suffix.push_str(&pyjson::py_str(latest));
                }
                if activity.get("bounded").is_some_and(Value::truthy) {
                    suffix.push_str(" (bounded sample)");
                }
            } else if matches!(
                activity.get("reported_head_matches"),
                Some(Value::Bool(true))
            ) {
                let head = activity
                    .get("head_sha")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                suffix = format!(
                    "; candidate HEAD {}",
                    head.chars().take(12).collect::<String>()
                );
            }
            if !suffix.is_empty() {
                let keep = 200usize.saturating_sub(suffix.chars().count());
                status = status.chars().take(keep).collect::<String>() + &suffix;
            }
        }
        let mut lane = Map::new();
        lane.insert("id", identity.as_str());
        lane.insert(
            "owner",
            text(
                get_or(row, "owner", get_or(row, "agent_session", &NULL)),
                160,
            ),
        );
        lane.insert(
            "node",
            text(get_or(row, "node", get_or(row, "host", &NULL)), 160),
        );
        lane.insert("branch", text(row.get("branch").unwrap_or(&NULL), 160));
        lane.insert("status", status.chars().take(200).collect::<String>());
        lane.insert("reported_status", reported_status);
        lane.insert(
            "packet_id",
            text(
                get_or(
                    row,
                    "packet_id",
                    get_or(row, "packet", get_or(row, "task", &NULL)),
                ),
                160,
            ),
        );
        lane.insert("observation_state", state);
        lane.insert("freshness", freshness);
        lane.insert("source_updated_utc", raw_time);
        lane.insert("observed_utc", iso(&now));
        lane.insert("screen", text(&Value::Str(session.clone()), 160));
        lane.insert("screen_state", screen_state);
        lane.insert("screen_pid", actual_pid);
        lane.insert("reported_screen_pid", reported_pid);
        lane.insert("self_reported", *reported);
        lane.insert(
            "agent_session",
            text(row.get("agent_session").unwrap_or(&NULL), 160),
        );
        lane.insert("model", model);
        lane.insert("effort", effort);
        lane.insert("tier", text(row.get("tier").unwrap_or(&NULL), 160));
        lane.insert("head_sha", text(row.get("head_sha").unwrap_or(&NULL), 160));
        lane.insert("worktree", text(row.get("worktree").unwrap_or(&NULL), 500));
        lane.insert("reserved_cpus", reserved.clone());
        lane.insert(
            "cpus_pinned",
            match pinned {
                Value::Bool(flag) => Value::Bool(*flag),
                _ => Value::Null,
            },
        );
        if let Some(activity) = activity {
            lane.insert("activity", activity);
        }
        lanes.push(Value::Object(lane));
    }
    output.insert("lanes", Value::Array(lanes));
    let mut observer = Map::new();
    observer.insert("producer", PRODUCER);
    observer.insert("observed_utc", iso(&now));
    observer.insert("source_updated_utc", source_time);
    observer.insert("lanes_source_updated_utc", report_time);
    observer.insert("source", source_meta.clone());
    observer.insert("lanes_source", lane_meta.clone());
    observer.insert(
        "reports_state",
        if reports.is_none() {
            "pending"
        } else {
            "available"
        },
    );
    observer.insert("session_prefix", options.prefix);
    observer.insert(
        "screen_probe",
        match options.screens {
            None => "disabled",
            Some(screens) if screens.available => "available",
            Some(_) => "unavailable",
        },
    );
    observer.insert("stale_after_s", options.stale_after);
    observer.insert(
        "launch_method",
        match reports {
            Some(Value::Object(map)) => text(map.get("launch_method").unwrap_or(&NULL), 500),
            _ => String::new(),
        },
    );
    observer.insert(
        "assurance",
        "Read-only observation. Lane self-reports and process liveness do not establish work progress, verification or integration.",
    );
    output.insert("observer", Value::Object(observer));
    // Deliberately preserve producer, updated_utc and every packet/proof byte-value.
    Ok(Value::Object(output))
}

/// `atomic_output(raw, value, inputs)` with an injectable replacement.
pub fn atomic_output(
    raw: &Path,
    value: &Value,
    inputs: &[PathBuf],
    replace: ReplaceFn<'_>,
) -> ToolResult<()> {
    let output = safe_path(raw)?;
    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    require(
        output.extension().is_some_and(|ext| ext == "json")
            && name != "package.json"
            && name != "tsconfig.json",
        "output must be a dedicated runtime JSON filename",
    )?;
    let parent = output.parent().unwrap_or(Path::new("/"));
    require(
        parent.is_dir() && fsx::owner(parent).is_ok_and(|uid| uid == fsx::getuid()),
        "output parent must be an existing owned directory",
    )?;
    require(!inputs.contains(&output), "output cannot replace an input")?;
    if output.exists() {
        let info = std::fs::metadata(&output)?;
        require(
            info.file_type().is_file() && info.uid() == fsx::getuid(),
            "output must be a regular owned file",
        )?;
        require(
            !inputs
                .iter()
                .any(|path| path.exists() && fsx::samefile(&output, path).unwrap_or(false)),
            "output aliases an input",
        )?;
    }
    let mut data = pyjson::dumps(
        value,
        &DumpOptions::PYTHON.sorted().indent(2).strict_floats(),
    )?;
    data.push('\n');
    require(
        data.len() <= MAX_BYTES,
        "combined output exceeds dashboard one MiB bound",
    )?;
    fsx::atomic_write(&output, ".rollout-observe-", data.as_bytes(), replace)?;
    Ok(())
}

/// Observation options for [`observe`].
pub struct ObserveOptions<'a> {
    /// Session prefix enabling the Screen probe.
    pub prefix: Option<&'a str>,
    /// Pre-computed Screen snapshot (skips the probe).
    pub screens: Option<ScreenSnapshot>,
    /// Opt-in worktree prefix.
    pub worktree_root: Option<&'a Path>,
    /// Screen probe implementation.
    pub screen_probe: &'a dyn Fn(&str) -> ToolResult<ScreenSnapshot>,
    /// HEAD probe implementation.
    pub head_probe: HeadProbe<'a>,
    /// Witness bounds.
    pub limits: Limits,
}

impl Default for ObserveOptions<'_> {
    fn default() -> Self {
        Self {
            prefix: None,
            screens: None,
            worktree_root: None,
            screen_probe: &screen_snapshot,
            head_probe: &bounded_git_head,
            limits: Limits::default(),
        }
    }
}

/// `observe(source, lanes, output, ...)`: one observation and publication.
pub fn observe(
    source: &Path,
    lanes: &Path,
    output: &Path,
    options: &ObserveOptions<'_>,
) -> ToolResult<Value> {
    let (template, source_meta) = read_owned(source)?;
    let (reports, lane_meta) = match read_owned(lanes) {
        Ok((reports, meta)) => (Some(reports), meta),
        Err(error) if error.is_not_found() => {
            let name = fsx::normalized(lanes)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (
                None,
                crate::pyobj! { "filename" => name, "state" => "missing" },
            )
        }
        Err(error) => return Err(error),
    };
    let mut screens = options.screens.clone();
    if let Some(prefix) = options.prefix.filter(|prefix| !prefix.is_empty())
        && screens.is_none()
    {
        screens = Some((options.screen_probe)(prefix)?);
    }
    let merge_options = MergeOptions {
        now: Utc::now(),
        prefix: options.prefix,
        screens: screens.as_ref(),
        stale_after: 120,
        worktree_root: options.worktree_root,
        limits: options.limits,
        head_probe: options.head_probe,
    };
    let result = merge(
        &template,
        reports.as_ref(),
        &source_meta,
        &lane_meta,
        &merge_options,
    )?;
    atomic_output(
        output,
        &result,
        &[safe_path(source)?, safe_path(lanes)?],
        &fsx::os_replace,
    )?;
    Ok(result)
}

/// `admiralty rollout observe` arguments (same flags as the Python argparse).
#[derive(clap::Args, Debug, Clone)]
pub struct ObserveArgs {
    /// Integration owner's rollout template JSON.
    #[arg(long)]
    pub source: PathBuf,
    /// Captain-written lane report JSON (may be missing).
    #[arg(long)]
    pub lanes: PathBuf,
    /// Dedicated runtime JSON output.
    #[arg(long)]
    pub output: PathBuf,
    /// Seconds between observations (1..3600).
    #[arg(long, default_value_t = 15, allow_negative_numbers = true)]
    pub interval: i64,
    /// Observe once and exit.
    #[arg(long)]
    pub once: bool,
    /// Enable fixed `screen -ls` corroboration for this safe prefix.
    #[arg(long = "session-prefix")]
    pub session_prefix: Option<String>,
    /// Opt-in existing owned 0700 prefix for reported worktree HEAD and explicit candidate-path metadata only.
    #[arg(long = "worktree-root")]
    pub worktree_root: Option<PathBuf>,
}

/// Run the observer loop; returns the process exit code.
pub fn run(args: &ObserveArgs) -> i32 {
    if !(1..=3600).contains(&args.interval)
        || args
            .session_prefix
            .as_deref()
            .is_some_and(|prefix| !ID.is_match(prefix))
    {
        eprintln!(
            "rollout_observe: error: interval must be 1..3600 seconds and session prefix must be a safe identifier"
        );
        return 2;
    }
    let options = ObserveOptions {
        prefix: args.session_prefix.as_deref(),
        worktree_root: args.worktree_root.as_deref(),
        ..ObserveOptions::default()
    };
    loop {
        match observe(&args.source, &args.lanes, &args.output, &options) {
            Ok(result) => {
                if args.once {
                    let lanes = result
                        .get_value("lanes")
                        .ok()
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    let observed = result
                        .get_value("observer")
                        .and_then(|observer| observer.get_value("observed_utc"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let updated = result
                        .get_value("updated_utc")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let summary = crate::pyobj! {
                        "lanes" => lanes,
                        "observed_utc" => observed,
                        "source_updated_utc" => updated,
                    };
                    println!("{}", pyjson::dumps_default(&summary));
                    return 0;
                }
            }
            Err(error) => {
                eprintln!("rollout_observe: {error}");
                if args.once {
                    return 2;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(args.interval.unsigned_abs()));
    }
}

#[cfg(test)]
#[path = "rollout_observe_tests.rs"]
mod tests;
