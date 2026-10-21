//! Deterministic source package builder (port of the retired
//! `scripts/release/package.py`).
//!
//! Builds a `.tar.gz` from an explicit, bounded list of regular files:
//! `MANIFEST.json` first, entries sorted by archive path, PAX/ustar headers
//! with mtime 0, uid/gid 0, empty user/group names and mode 0644/0755, a gzip
//! member with mtime 0, no file name and level 9, published atomically
//! without replacement through a hard link. Sources are opened through
//! pinned no-follow directory handles, identity-checked before and after
//! capture and hashed twice.
//!
//! Differences from the Python harness: archive paths must be ASCII (the
//! Python accepted any NFC path; without a Unicode normalization table the
//! port refuses every non-ASCII path rather than risk a non-NFC name), and
//! the compressed bytes come from `miniz_oxide` instead of zlib, so archives
//! are reproducible with this implementation and their *uncompressed* tar
//! stream (headers, padding, record blocking) and manifest bytes are
//! identical to the Python output.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{AtFlags, Mode, OFlags};
use sha2::{Digest, Sha256};

use crate::py::{self, Dump, PyValue};

/// Largest accepted input JSON.
pub const MAX_INPUT_JSON: usize = 1024 * 1024;
/// Most entries.
pub const MAX_FILES: usize = 1024;
/// Largest string in UTF-8 bytes.
pub const MAX_STRING: usize = 4096;
/// Largest source file.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Largest total of all source files.
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const CHUNK_BYTES: usize = 1024 * 1024;
/// Reserved first member.
pub const MANIFEST_PATH: &str = "MANIFEST.json";
const DENIED_COMPONENTS: &[&str] = &[
    ".git",
    ".ssh",
    ".codex",
    "runtime",
    "runtimes",
    "state",
    "states",
    "private",
    "credential",
    "credentials",
    "secret",
    "secrets",
    "key",
    "keys",
    "token",
    "tokens",
    "id_rsa",
    "id_ed25519",
];
const SENSITIVE_WORDS: &[&str] = &[
    "token",
    "tokens",
    "secret",
    "secrets",
    "credential",
    "credentials",
    "private",
    "key",
    "keys",
];
const PRIVATE_SUFFIXES: &[&str] = &[".pem", ".key", ".token", ".p12", ".pfx"];
const BLOCK: usize = 512;
const RECORD: usize = 20 * BLOCK;

/// A refused or failed package build.
#[derive(Debug)]
pub enum SourcePackageError {
    /// Invalid or unsafe input (the Python `SourcePackageError`).
    Invalid(String),
    /// An operating-system failure (the Python `OSError`).
    Io(io::Error),
}

impl fmt::Display for SourcePackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SourcePackageError {}

impl From<io::Error> for SourcePackageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rustix::io::Errno> for SourcePackageError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(error.into())
    }
}

type PackageResult<T> = Result<T, SourcePackageError>;

fn invalid(message: impl Into<String>) -> SourcePackageError {
    SourcePackageError::Invalid(message.into())
}

fn bounded_text(value: &PyValue, label: &str) -> PackageResult<String> {
    match value {
        PyValue::Str(text) => bounded_str(text, label),
        _ => Err(invalid(format!(
            "{label} must be a nonempty string of at most {MAX_STRING} bytes"
        ))),
    }
}

fn bounded_str(text: &str, label: &str) -> PackageResult<String> {
    if text.is_empty() || text.len() > MAX_STRING {
        return Err(invalid(format!(
            "{label} must be a nonempty string of at most {MAX_STRING} bytes"
        )));
    }
    if text.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
        return Err(invalid(format!("{label} contains a control character")));
    }
    Ok(text.to_owned())
}

fn sensitive_name(part: &str) -> bool {
    part.split(['.', '_', '-'])
        .any(|word| SENSITIVE_WORDS.contains(&word))
}

fn private_component(part: &str) -> bool {
    DENIED_COMPONENTS.contains(&part)
        || part == ".env"
        || part.starts_with(".env.")
        || sensitive_name(part)
}

fn reserved_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default().to_lowercase();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// Validate one manifest archive path (`_archive_path`).
pub fn archive_path(value: &PyValue) -> PackageResult<String> {
    let raw = bounded_text(value, "archive_path")?;
    if raw.contains('\\')
        || raw.starts_with('/')
        || raw.contains(['<', '>', ':', '"', '|', '?', '*'])
    {
        return Err(invalid(
            "archive_path must be a portable relative POSIX path",
        ));
    }
    if !raw.is_ascii() {
        // Stricter than Python: NFC cannot be proven without Unicode tables.
        return Err(invalid("archive_path must use NFC Unicode normalization"));
    }
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.iter().any(|part| matches!(*part, "" | "." | "..")) {
        return Err(invalid(
            "archive_path must not contain empty, dot, or parent components",
        ));
    }
    if parts
        .iter()
        .any(|part| part.len() > 255 || part.ends_with(['.', ' ']))
    {
        return Err(invalid("archive_path component is not portable"));
    }
    if parts.iter().any(|part| reserved_device(part)) {
        return Err(invalid("archive_path names a reserved device"));
    }
    for part in parts.iter().map(|part| part.to_lowercase()) {
        if private_component(&part) {
            return Err(invalid(
                "archive_path names a private or credential-bearing path",
            ));
        }
        if PRIVATE_SUFFIXES.iter().any(|suffix| part.ends_with(suffix)) {
            return Err(invalid(
                "archive_path names a private or credential-bearing file",
            ));
        }
    }
    let normalized = parts.join("/");
    if normalized.to_lowercase() == MANIFEST_PATH.to_lowercase() {
        return Err(invalid("archive_path is reserved for the package manifest"));
    }
    Ok(normalized)
}

/// Validate one relative source path (`_source_path`).
pub fn source_path(value: &PyValue) -> PackageResult<String> {
    let raw = bounded_text(value, "source")?;
    if raw.contains('\\') || raw.starts_with('/') || raw.contains(':') {
        return Err(invalid(
            "source must be a relative POSIX path beneath --input-root",
        ));
    }
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.iter().any(|part| matches!(*part, "" | "." | "..")) {
        return Err(invalid(
            "source must not contain empty, dot, or parent components",
        ));
    }
    for part in parts.iter().map(|part| part.to_lowercase()) {
        if private_component(&part) {
            return Err(invalid("source names a private or credential-bearing path"));
        }
        if PRIVATE_SUFFIXES.iter().any(|suffix| part.ends_with(suffix)) {
            return Err(invalid("source names a private or credential-bearing file"));
        }
    }
    Ok(parts.join("/"))
}

fn full_match(text: &str, first: fn(u8) -> bool, rest: fn(u8) -> bool, max_rest: usize) -> bool {
    let bytes = text.as_bytes();
    match bytes.split_first() {
        Some((head, tail)) => {
            first(*head) && tail.len() <= max_rest && tail.iter().all(|b| rest(*b))
        }
        None => false,
    }
}

/// Validate version, target and source commit (`_validate_metadata`).
pub fn validate_metadata(
    version: &str,
    target: &str,
    source_commit: &str,
) -> PackageResult<(String, String, String)> {
    let version = bounded_str(version, "version")?;
    let target = bounded_str(target, "target")?;
    let commit = bounded_str(source_commit, "source_commit")?;
    let alnum = |b: u8| b.is_ascii_alphanumeric();
    if !full_match(
        &version,
        alnum,
        |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'),
        127,
    ) {
        return Err(invalid("version has invalid characters or format"));
    }
    if !full_match(
        &target,
        alnum,
        |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'),
        127,
    ) {
        return Err(invalid("target has invalid characters or format"));
    }
    let lower_hex = commit
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !(lower_hex && (commit.len() == 40 || commit.len() == 64)) {
        return Err(invalid(
            "source_commit must be a full lowercase 40- or 64-character object ID",
        ));
    }
    Ok((version, target, commit))
}

/// `os.path.abspath(path)`: absolute and lexically normalized.
pub fn abspath(path: &Path) -> io::Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::from("/");
    for component in joined.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    Ok(normalized)
}

fn no_symlink_components(path: &Path) -> PackageResult<()> {
    let absolute = abspath(path)?;
    let mut current = PathBuf::from("/");
    for component in absolute.components() {
        if let Component::Normal(part) = component {
            current.push(part);
            let metadata = match fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(invalid(format!(
                        "path component does not exist: {}",
                        current.display()
                    )));
                }
                Err(error) => return Err(error.into()),
            };
            if metadata.file_type().is_symlink() {
                return Err(invalid(format!(
                    "symlink path component is forbidden: {}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

fn validate_root(root: &Path) -> PackageResult<PathBuf> {
    let absolute = abspath(root)?;
    no_symlink_components(&absolute)?;
    if !fs::symlink_metadata(&absolute)?.file_type().is_dir() {
        return Err(invalid(
            "--input-root must be an existing non-symlink directory",
        ));
    }
    Ok(absolute)
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Walk to `path`'s parent through no-follow directory handles, so a
/// swapped ancestor cannot redirect the final open.
fn pinned_parent(path: &Path) -> PackageResult<(OwnedFd, OsString)> {
    let absolute = abspath(path)?;
    let parts: Vec<&OsStr> = absolute
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    let mut descriptor = rustix::fs::open("/", DIR_FLAGS, Mode::empty())?;
    let Some((name, ancestors)) = parts.split_last() else {
        return Ok((descriptor, OsString::new()));
    };
    for part in ancestors {
        descriptor = rustix::fs::openat(&descriptor, *part, DIR_FLAGS, Mode::empty())?;
    }
    Ok((descriptor, name.to_os_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: i128,
    mtime: (i64, i64),
    ctime: (i64, i64),
    nlink: u64,
}

impl Identity {
    fn of_metadata(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: i128::from(metadata.size()),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
            nlink: metadata.nlink(),
        }
    }

    #[allow(clippy::unnecessary_cast, clippy::useless_conversion)]
    fn of_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            dev: stat.st_dev as u64,
            ino: stat.st_ino as u64,
            size: stat.st_size as i128,
            mtime: (stat.st_mtime as i64, stat.st_mtime_nsec as i64),
            ctime: (stat.st_ctime as i64, stat.st_ctime_nsec as i64),
            nlink: stat.st_nlink as u64,
        }
    }
}

fn stat_named(parent: &OwnedFd, name: &OsStr) -> PackageResult<rustix::fs::Stat> {
    Ok(rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?)
}

fn is_regular(stat: &rustix::fs::Stat) -> bool {
    rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::RegularFile
}

fn read_input_json(path: &Path) -> PackageResult<Vec<u8>> {
    let (parent, name) = pinned_parent(path)?;
    let descriptor = rustix::fs::openat(
        &parent,
        &name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| invalid("input JSON could not be opened without following links"))?;
    let file = File::from(descriptor);
    let before = file.metadata()?;
    if !before.file_type().is_file() {
        return Err(invalid("--inputs must be a regular file"));
    }
    let mut raw = Vec::new();
    (&file)
        .take(MAX_INPUT_JSON as u64 + 1)
        .read_to_end(&mut raw)?;
    let after = file.metadata()?;
    let named = stat_named(&parent, &name)?;
    if Identity::of_metadata(&before) != Identity::of_metadata(&after)
        || Identity::of_metadata(&after) != Identity::of_stat(&named)
    {
        return Err(invalid("input JSON changed during read"));
    }
    Ok(raw)
}

/// One validated input record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRecord {
    /// Relative source path beneath the input root.
    pub source: String,
    /// Archive member path.
    pub archive_path: String,
    /// 0o644 or 0o755.
    pub mode: u32,
}

/// Parse and validate the inputs document (`_parse_inputs`), sorted by
/// archive path.
pub fn parse_inputs(raw: &[u8]) -> PackageResult<Vec<InputRecord>> {
    if raw.len() > MAX_INPUT_JSON {
        return Err(invalid("input JSON exceeds 1 MiB"));
    }
    let document = py::loads_bytes(raw, true)
        .map_err(|error| invalid(format!("invalid input JSON: {error}")))?;
    let records = match &document {
        PyValue::Dict(dict) if dict.len() == 1 => match dict.get("files") {
            Some(PyValue::List(records)) => records,
            _ => return Err(invalid("input JSON must contain only a files array")),
        },
        _ => return Err(invalid("input JSON must contain only a files array")),
    };
    if records.is_empty() || records.len() > MAX_FILES {
        return Err(invalid(format!(
            "files must contain 1..{MAX_FILES} entries"
        )));
    }
    let mut parsed = Vec::with_capacity(records.len());
    let mut seen: Vec<String> = Vec::with_capacity(records.len());
    for (index, item) in records.iter().enumerate() {
        let fields = match item {
            PyValue::Dict(fields)
                if fields.len() == 3
                    && ["source", "archive_path", "mode"]
                        .iter()
                        .all(|key| fields.contains_key(key)) =>
            {
                fields
            }
            _ => {
                return Err(invalid(format!(
                    "files[{index}] must contain exactly source, archive_path, and mode"
                )));
            }
        };
        let source = source_path(fields.get("source").unwrap_or(&PyValue::Null))?;
        let archive = archive_path(fields.get("archive_path").unwrap_or(&PyValue::Null))?;
        let mode = match fields.get("mode") {
            Some(PyValue::Int(420)) => 0o644,
            Some(PyValue::Int(493)) => 0o755,
            _ => {
                return Err(invalid(format!(
                    "files[{index}].mode must be 420 (0644) or 493 (0755)"
                )));
            }
        };
        let folded = archive.to_lowercase();
        if seen.iter().any(|other| {
            folded == *other
                || folded.starts_with(&format!("{other}/"))
                || other.starts_with(&format!("{folded}/"))
        }) {
            return Err(invalid(format!(
                "colliding portable archive path: {archive}"
            )));
        }
        seen.push(folded);
        parsed.push(InputRecord {
            source,
            archive_path: archive,
            mode,
        });
    }
    parsed.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
    Ok(parsed)
}

fn hash_file(file: &mut File) -> PackageResult<(String, u64)> {
    let mut digest = Sha256::new();
    let mut total: u64 = 0;
    file.seek(SeekFrom::Start(0))?;
    let mut chunk = vec![0u8; CHUNK_BYTES];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_FILE_BYTES {
            return Err(invalid("input file exceeds the per-file byte limit"));
        }
        digest.update(&chunk[..count]);
    }
    Ok((format!("{:x}", digest.finalize()), total))
}

/// A hook receiving the name about to be opened.
pub(crate) type NameHook<'a> = Box<dyn FnMut(&OsStr) + 'a>;
/// A hook receiving the publish destination.
pub(crate) type PathHook<'a> = Box<dyn FnMut(&Path) + 'a>;

/// Test hooks reproducing the Python mock-based race scenarios.
#[derive(Default)]
pub(crate) struct Hooks<'a> {
    /// Called with the file name right before a source is opened.
    pub before_source_open: Option<NameHook<'a>>,
    /// Called after each non-empty first-pass read of a source.
    pub after_capture_chunk: Option<Box<dyn FnMut() + 'a>>,
    /// Called with the destination right before the publishing link.
    pub before_publish_link: Option<PathHook<'a>>,
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn capture_source(
    root: &Path,
    relative: &str,
    spool: &mut File,
    hooks: &mut Hooks<'_>,
) -> PackageResult<(String, u64)> {
    let candidate = join_relative(root, relative);
    let (parent, name) = pinned_parent(&candidate)?;
    let before_path = stat_named(&parent, &name)?;
    if !is_regular(&before_path) {
        return Err(invalid(format!("input is not a regular file: {relative}")));
    }
    let identity_before = Identity::of_stat(&before_path);
    if identity_before.nlink != 1 {
        return Err(invalid(format!(
            "hard-linked input is ambiguous: {relative}"
        )));
    }
    if identity_before.size > i128::from(MAX_FILE_BYTES) {
        return Err(invalid(format!(
            "input file exceeds the per-file byte limit: {relative}"
        )));
    }
    if let Some(hook) = hooks.before_source_open.as_mut() {
        hook(name.as_os_str());
    }
    let descriptor = rustix::fs::openat(
        &parent,
        &name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut file = File::from(descriptor);
    let before_fd = file.metadata()?;
    if !before_fd.file_type().is_file()
        || Identity::of_metadata(&before_fd) != Identity::of_stat(&before_path)
    {
        return Err(invalid(format!("input changed while opening: {relative}")));
    }
    let mut digest = Sha256::new();
    let mut total: u64 = 0;
    file.seek(SeekFrom::Start(0))?;
    let mut chunk = vec![0u8; CHUNK_BYTES];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if let Some(hook) = hooks.after_capture_chunk.as_mut() {
            hook();
        }
        total += count as u64;
        if total > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "input file exceeds the per-file byte limit: {relative}"
            )));
        }
        spool.write_all(&chunk[..count])?;
        digest.update(&chunk[..count]);
    }
    let after_copy = file.metadata()?;
    let named_after = stat_named(&parent, &name)?;
    let identity = Identity::of_metadata(&before_fd);
    if identity != Identity::of_metadata(&after_copy) || identity != Identity::of_stat(&named_after)
    {
        return Err(invalid(format!("input changed during capture: {relative}")));
    }
    if total != before_fd.size() {
        return Err(invalid(format!(
            "input size changed during capture: {relative}"
        )));
    }
    let first_digest = format!("{:x}", digest.finalize());
    let (second_digest, second_size) = hash_file(&mut file)?;
    if second_digest != first_digest || second_size != total {
        return Err(invalid(format!(
            "input content changed during capture: {relative}"
        )));
    }
    let after_verify = file.metadata()?;
    let named_verified = stat_named(&parent, &name)?;
    if identity != Identity::of_metadata(&after_verify)
        || identity != Identity::of_stat(&named_verified)
    {
        return Err(invalid(format!(
            "input changed during verification: {relative}"
        )));
    }
    spool.seek(SeekFrom::Start(0))?;
    Ok((first_digest, total))
}

/// One manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Archive member path.
    pub archive_path: String,
    /// Lowercase SHA-256 hex of the captured bytes.
    pub sha256: String,
    /// Captured size.
    pub size: u64,
    /// 0o644 or 0o755.
    pub mode: u32,
}

/// The package manifest (`MANIFEST.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Supplied version label.
    pub version: String,
    /// Supplied target label.
    pub target: String,
    /// Supplied full source commit label.
    pub source_commit: String,
    /// Entries in archive-path order.
    pub entries: Vec<ManifestEntry>,
}

impl Manifest {
    /// The manifest as a Python-compatible JSON value.
    pub fn to_value(&self) -> PyValue {
        let entries: Vec<PyValue> = self
            .entries
            .iter()
            .map(|entry| {
                crate::py_dict! {
                    "archive_path" => entry.archive_path.as_str(),
                    "sha256" => entry.sha256.as_str(),
                    "size" => entry.size,
                    "mode" => entry.mode,
                }
            })
            .collect();
        crate::py_dict! {
            "schema_version" => 1,
            "version" => self.version.as_str(),
            "target" => self.target.as_str(),
            "source_commit" => self.source_commit.as_str(),
            "entries" => entries,
        }
    }

    /// `MANIFEST.json` bytes: sorted compact UTF-8 JSON plus a newline.
    pub fn to_bytes(&self) -> Vec<u8> {
        let options = Dump {
            ensure_ascii: false,
            ..Dump::COMPACT_SORTED
        };
        let mut bytes = py::dumps(&self.to_value(), options).into_bytes();
        bytes.push(b'\n');
        bytes
    }
}

// ---------------------------------------------------------------------------
// tarfile.PAX_FORMAT stream writer
// ---------------------------------------------------------------------------

fn stn(text: &str, length: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = text
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .take(length)
        .collect();
    bytes.resize(length, 0);
    bytes
}

fn itn(value: u64, digits: usize) -> io::Result<Vec<u8>> {
    if value >= 8u64.pow(digits as u32 - 1) {
        return Err(io::Error::other("tar numeric field overflow"));
    }
    let mut field = format!("{:0width$o}", value, width = digits - 1).into_bytes();
    field.push(0);
    Ok(field)
}

fn ustar_header(name: &str, mode: u32, size: u64, typeflag: u8) -> io::Result<[u8; BLOCK]> {
    let mut parts = Vec::with_capacity(BLOCK);
    parts.extend(stn(name, 100));
    parts.extend(itn(u64::from(mode & 0o7777), 8)?);
    parts.extend(itn(0, 8)?);
    parts.extend(itn(0, 8)?);
    parts.extend(itn(size, 12)?);
    parts.extend(itn(0, 12)?);
    parts.extend(b"        ");
    parts.push(typeflag);
    parts.extend(stn("", 100));
    parts.extend(b"ustar\x0000");
    parts.extend(stn("", 32));
    parts.extend(stn("", 32));
    parts.extend(stn("", 8));
    parts.extend(stn("", 8));
    parts.extend(stn("", 155));
    let mut block = [0u8; BLOCK];
    block[..parts.len()].copy_from_slice(&parts);
    let checksum: u32 = block.iter().map(|&byte| u32::from(byte)).sum();
    let field = format!("{checksum:06o}\0");
    block[148..155].copy_from_slice(field.as_bytes());
    Ok(block)
}

fn pax_records(keyword: &str, value: &str) -> Vec<u8> {
    let base = keyword.len() + value.len() + 3;
    let mut previous = 0usize;
    loop {
        let candidate = base + previous.to_string().len();
        if candidate == previous {
            break;
        }
        previous = candidate;
    }
    format!("{previous} {keyword}={value}\n").into_bytes()
}

struct TarStream<W: Write> {
    out: W,
    offset: usize,
}

impl<W: Write> TarStream<W> {
    fn write_padded(&mut self, data: &[u8]) -> io::Result<()> {
        self.out.write_all(data)?;
        self.offset += data.len();
        let remainder = data.len() % BLOCK;
        if remainder != 0 {
            self.out.write_all(&[0u8; BLOCK][..BLOCK - remainder])?;
            self.offset += BLOCK - remainder;
        }
        Ok(())
    }

    /// `TarFile.addfile(_tar_info(name, size, mode), source)`.
    fn add(&mut self, name: &str, mode: u32, size: u64, source: &mut dyn Read) -> io::Result<()> {
        if !name.is_ascii() || name.chars().count() > 100 {
            let records = pax_records("path", name);
            let header = ustar_header("././@PaxHeader", 0, records.len() as u64, b'x')?;
            self.write_padded(&header)?;
            self.write_padded(&records)?;
        }
        let header = ustar_header(name, mode, size, b'0')?;
        self.write_padded(&header)?;
        let copied = io::copy(&mut source.take(size), &mut self.out)?;
        if copied != size {
            return Err(io::Error::other("unexpected end of data"));
        }
        self.offset += size as usize;
        let remainder = (size as usize) % BLOCK;
        if remainder != 0 {
            self.out.write_all(&[0u8; BLOCK][..BLOCK - remainder])?;
            self.offset += BLOCK - remainder;
        }
        Ok(())
    }

    /// `TarFile.close()`: two zero blocks, then zero-fill to a 20-block record.
    fn finish(mut self) -> io::Result<W> {
        self.out.write_all(&[0u8; 2 * BLOCK])?;
        self.offset += 2 * BLOCK;
        let remainder = self.offset % RECORD;
        if remainder != 0 {
            self.out.write_all(&vec![0u8; RECORD - remainder])?;
        }
        Ok(self.out)
    }
}

fn write_archive(
    out: File,
    manifest_bytes: &[u8],
    staged: &[(&InputRecord, PathBuf)],
) -> PackageResult<File> {
    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(out, flate2::Compression::new(9));
    let mut tar = TarStream {
        out: encoder,
        offset: 0,
    };
    tar.add(
        MANIFEST_PATH,
        0o644,
        manifest_bytes.len() as u64,
        &mut &manifest_bytes[..],
    )?;
    for (record, spool_path) in staged {
        let size = fs::metadata(spool_path)?.len();
        let mut spool = File::open(spool_path)?;
        tar.add(&record.archive_path, record.mode, size, &mut spool)?;
    }
    let encoder = tar.finish()?;
    Ok(encoder.finish()?)
}

/// Build a package atomically; refuses every pre-existing destination path.
pub fn build_package(
    input_root: &Path,
    inputs_json: &Path,
    output: &Path,
    version: &str,
    target: &str,
    source_commit: &str,
) -> PackageResult<Manifest> {
    build_package_with_hooks(
        input_root,
        inputs_json,
        output,
        (version, target, source_commit),
        &mut Hooks::default(),
    )
}

pub(crate) fn build_package_with_hooks(
    input_root: &Path,
    inputs_json: &Path,
    output: &Path,
    labels: (&str, &str, &str),
    hooks: &mut Hooks<'_>,
) -> PackageResult<Manifest> {
    let (version, target, source_commit) = validate_metadata(labels.0, labels.1, labels.2)?;
    let root = validate_root(input_root)?;
    let input_bytes = read_input_json(inputs_json)?;
    let records = parse_inputs(&input_bytes)?;
    let mut total_declared: u64 = 0;
    for record in &records {
        let source = join_relative(&root, &record.source);
        no_symlink_components(&source)?;
        let metadata = fs::symlink_metadata(&source)?;
        if !metadata.file_type().is_file() || metadata.nlink() != 1 {
            return Err(invalid(format!(
                "input must be an unlinked regular file: {}",
                record.source
            )));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "input file exceeds the per-file byte limit: {}",
                record.source
            )));
        }
        total_declared += metadata.len();
        if total_declared > MAX_TOTAL_BYTES {
            return Err(invalid("declared inputs exceed the total byte limit"));
        }
    }

    let destination = abspath(output)?;
    let parent = destination
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    no_symlink_components(&parent)?;
    if !fs::symlink_metadata(&parent)?.file_type().is_dir() {
        return Err(invalid(
            "output parent must be an existing non-symlink directory",
        ));
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(invalid("output already exists; refusing to overwrite"));
    }

    let stage = tempfile::Builder::new()
        .prefix("adm-pkg-stage-")
        .tempdir_in(&parent)?;
    let mut entries = Vec::with_capacity(records.len());
    let mut staged = Vec::with_capacity(records.len());
    let mut actual_total: u64 = 0;
    for (index, record) in records.iter().enumerate() {
        let spool_path = stage.path().join(format!("{index:04}.data"));
        let mut spool = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&spool_path)?;
        let (digest, size) = capture_source(&root, &record.source, &mut spool, hooks)?;
        drop(spool);
        actual_total += size;
        if actual_total > MAX_TOTAL_BYTES {
            return Err(invalid("captured inputs exceed the total byte limit"));
        }
        entries.push(ManifestEntry {
            archive_path: record.archive_path.clone(),
            sha256: digest,
            size,
            mode: record.mode,
        });
        staged.push((record, spool_path));
    }
    let manifest = Manifest {
        version,
        target,
        source_commit,
        entries,
    };
    let manifest_bytes = manifest.to_bytes();
    let temporary = tempfile::Builder::new()
        .prefix(".adm-pkg-")
        .suffix(".tmp")
        .tempfile_in(&parent)?;
    let out = temporary.as_file().try_clone()?;
    let out = write_archive(out, &manifest_bytes, &staged)?;
    out.set_permissions(fs::Permissions::from_mode(0o644))?;
    out.sync_all()?;
    drop(out);
    if let Some(hook) = hooks.before_publish_link.as_mut() {
        hook(&destination);
    }
    // link() is an atomic no-replace operation on the same filesystem.
    match fs::hard_link(temporary.path(), &destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(invalid(
                "output appeared during creation; refusing to overwrite",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    temporary.close()?;
    File::open(&parent)?.sync_all()?;
    drop(stage);
    Ok(manifest)
}

/// `scripts/release/package.py` arguments.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct PackageArgs {
    /// Explicit input root (no symlink components).
    #[arg(long = "input-root")]
    pub input_root: Option<PathBuf>,
    /// JSON file with an explicit files array.
    #[arg(long)]
    pub inputs: Option<PathBuf>,
    /// Version label.
    #[arg(long)]
    pub version: Option<String>,
    /// Target label.
    #[arg(long)]
    pub target: Option<String>,
    /// Full lowercase 40- or 64-character source commit label.
    #[arg(long = "source-commit")]
    pub source_commit: Option<String>,
    /// New archive path (never replaced).
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Run the ten built-in package scenarios in a private temporary directory.
    #[arg(long = "self-test")]
    pub self_test: bool,
}

/// Run the packager CLI: print the sorted compact manifest (exit 0), report
/// usage or package errors (exit 2), or run the self-test (exit 0/1).
pub fn run_package(args: PackageArgs) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    if args.self_test {
        let report = self_test();
        for failure in &report.failures {
            eprintln!("FAIL: {failure}");
        }
        eprintln!("Ran {} tests", report.cases_run);
        if report.failures.is_empty() && report.cases_run > 0 {
            eprintln!("OK");
            return Ok(0);
        }
        eprintln!("FAILED (failures={})", report.failures.len());
        return Ok(1);
    }
    let (Some(input_root), Some(inputs), Some(version), Some(target), Some(commit), Some(output)) = (
        args.input_root,
        args.inputs,
        args.version,
        args.target,
        args.source_commit,
        args.output,
    ) else {
        eprintln!(
            "error: packaging requires --input-root, --inputs, --version, --target, --source-commit, and --output"
        );
        return Ok(2);
    };
    match build_package(&input_root, &inputs, &output, &version, &target, &commit) {
        Ok(manifest) => {
            println!("{}", py::dumps(&manifest.to_value(), Dump::COMPACT_SORTED));
            Ok(0)
        }
        Err(error) => {
            eprintln!("error: {error}");
            Ok(2)
        }
    }
}

/// Outcome of [`self_test`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfTestReport {
    /// Scenarios executed (10).
    pub cases_run: usize,
    /// `"<scenario>: <reason>"` for every failed scenario.
    pub failures: Vec<String>,
}

/// Execute the ten `PackageTests` scenarios of the retired Python packager
/// in private temporary directories (under `ADM_PACKAGE_TEST_SCRATCH` when
/// set, otherwise the system temporary directory).
pub fn self_test() -> SelfTestReport {
    let mut failures = Vec::new();
    for (name, scenario) in self_tests::SCENARIOS {
        let outcome = std::panic::catch_unwind(|| self_tests::run_scenario(scenario));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => failures.push(format!("{name}: {reason}")),
            Err(_) => failures.push(format!("{name}: panicked")),
        }
    }
    SelfTestReport {
        cases_run: self_tests::SCENARIOS.len(),
        failures,
    }
}

#[path = "source_package_self_tests.rs"]
mod self_tests;
