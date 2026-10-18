//! Bounded native release staging from an explicit allowlist.
//!
//! This module creates a deterministic directory with a SHA-256 manifest. It
//! does not create an archive or claim that an installer is complete.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::content_digest;
use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const MAX_FILES: usize = 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const BUFFER_SIZE: usize = 1024 * 1024;
const MANIFEST_NAME: &str = "MANIFEST.json";

/// One explicitly selected source file and its expected raw-byte SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInput {
    pub source: String,
    pub expected_sha256: String,
    pub mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEntry {
    pub archive_path: String,
    pub sha256: String,
    pub size: u64,
    pub mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct UnsignedManifest {
    schema_version: u32,
    version: String,
    target: String,
    source_commit: String,
    entries: Vec<PackageEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub schema_version: u32,
    pub version: String,
    pub target: String,
    pub source_commit: String,
    pub entries: Vec<PackageEntry>,
    pub manifest_sha256: String,
}

/// Reports staged files only; no archive or installer is produced here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageReceipt {
    pub output_dir: PathBuf,
    pub manifest: PackageManifest,
    pub archive_created: bool,
    pub installer_complete: bool,
}

#[derive(Debug)]
pub enum PackageError {
    InvalidInput(&'static str),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(reason) => write!(f, "invalid native package input: {reason}"),
            Self::Io(error) => write!(f, "native package I/O failed: {error}"),
            Self::Json(error) => write!(f, "native package manifest failed: {error}"),
        }
    }
}

impl Error for PackageError {}

impl From<io::Error> for PackageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rustix::io::Errno> for PackageError {
    fn from(value: rustix::io::Errno) -> Self {
        Self::Io(io::Error::from(value))
    }
}

impl From<serde_json::Error> for PackageError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum InputKind {
    Binary,
    Dashboard,
    Schema,
    Guard,
    Documentation,
}

/// Stage an explicit release allowlist into a new directory using an atomic,
/// no-replace rename. No directory traversal is used to discover inputs.
pub fn stage_native_package(
    input_root: &Path,
    output_dir: &Path,
    version: &str,
    target: &str,
    source_commit: &str,
    inputs: &[PackageInput],
) -> Result<PackageReceipt, PackageError> {
    validate_metadata(version, target, source_commit)?;
    if inputs.is_empty() || inputs.len() > MAX_FILES {
        return Err(PackageError::InvalidInput("input count is outside 1..1024"));
    }

    let mut seen_sources = BTreeSet::new();
    let mut seen_destinations = BTreeSet::new();
    let mut prepared = Vec::with_capacity(inputs.len());
    let mut kinds = BTreeSet::new();
    let mut declared_total = 0_u64;
    for input in inputs {
        let (destination, kind) = package_destination(&input.source)?;
        if !seen_sources.insert(input.source.clone())
            || !seen_destinations.insert(destination.to_ascii_lowercase())
        {
            return Err(PackageError::InvalidInput("duplicate package input"));
        }
        if input.mode != 0o644 && input.mode != 0o755 {
            return Err(PackageError::InvalidInput("file mode must be 0644 or 0755"));
        }
        if (kind == InputKind::Binary) != (input.mode == 0o755) {
            return Err(PackageError::InvalidInput(
                "mode does not match allowlisted input kind",
            ));
        }
        if !valid_sha256(&input.expected_sha256) {
            return Err(PackageError::InvalidInput(
                "expected SHA-256 must be 64 lowercase hex digits",
            ));
        }
        kinds.insert(kind);
        if kind == InputKind::Schema {
            kinds.insert(InputKind::Guard);
        }
        prepared.push((input, destination));
    }
    if !kinds.contains(&InputKind::Binary)
        || !kinds.contains(&InputKind::Dashboard)
        || !kinds.contains(&InputKind::Schema)
        || !kinds.contains(&InputKind::Guard)
        || !kinds.contains(&InputKind::Documentation)
    {
        return Err(PackageError::InvalidInput(
            "release allowlist must include a Rust binary, dashboard, v2 schema, v2 guard, and documentation",
        ));
    }

    let canonical_root = checked_directory(input_root)?;
    let root_fd = open_directory(&canonical_root)?;
    let mut sources = Vec::with_capacity(prepared.len());
    for (input, _) in &prepared {
        let file = open_source(&root_fd, &input.source)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_FILE_BYTES {
            return Err(PackageError::InvalidInput(
                "source must be an unlinked regular file within the per-file bound",
            ));
        }
        declared_total = declared_total
            .checked_add(metadata.len())
            .ok_or(PackageError::InvalidInput("total input size overflow"))?;
        if declared_total > MAX_TOTAL_BYTES {
            return Err(PackageError::InvalidInput("total input size exceeds 1 GiB"));
        }
        sources.push((file, identity(&metadata)));
    }

    let output_parent = output_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(PackageError::InvalidInput(
            "output must name a directory beneath an existing parent",
        ))?;
    let output_name = output_dir
        .file_name()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or(PackageError::InvalidInput(
            "output directory name is invalid",
        ))?;
    let canonical_parent = checked_directory(output_parent)?;
    let parent_fd = open_directory(&canonical_parent)?;
    if rustix::fs::statat(&parent_fd, output_name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Err(PackageError::InvalidInput("output already exists"));
    }

    let staging = tempfile::Builder::new()
        .prefix(".admiralty-native-package-")
        .tempdir_in(&canonical_parent)?;
    let mut entries = Vec::with_capacity(prepared.len());
    let mut actual_total = 0_u64;
    for ((input, destination), (mut source, before_identity)) in prepared.iter().zip(sources) {
        let staged_path = staging.path().join(destination);
        if let Some(parent) = staged_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(input.mode)
            .open(&staged_path)?;
        let (digest, size) = copy_and_hash(&mut source, &mut staged)?;
        if digest != input.expected_sha256 {
            return Err(PackageError::InvalidInput("source SHA-256 mismatch"));
        }
        actual_total = actual_total
            .checked_add(size)
            .ok_or(PackageError::InvalidInput("captured input size overflow"))?;
        if actual_total > MAX_TOTAL_BYTES {
            return Err(PackageError::InvalidInput("captured inputs exceed 1 GiB"));
        }
        let after = source.metadata()?;
        if identity(&after) != before_identity || !after.is_file() || after.nlink() != 1 {
            return Err(PackageError::InvalidInput("source changed during capture"));
        }
        let second_digest = hash_open_file(&mut source)?;
        let verified = source.metadata()?;
        if second_digest != digest || identity(&verified) != before_identity {
            return Err(PackageError::InvalidInput(
                "source changed during verification",
            ));
        }
        staged.flush()?;
        staged.sync_all()?;
        fs::set_permissions(&staged_path, fs::Permissions::from_mode(input.mode))?;
        entries.push(PackageEntry {
            archive_path: destination.clone(),
            sha256: digest,
            size,
            mode: input.mode,
        });
    }
    entries.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));

    let unsigned = UnsignedManifest {
        schema_version: 1,
        version: version.to_owned(),
        target: target.to_owned(),
        source_commit: source_commit.to_owned(),
        entries,
    };
    let manifest_sha256 = content_digest(&unsigned)?;
    let manifest = PackageManifest {
        schema_version: unsigned.schema_version,
        version: unsigned.version,
        target: unsigned.target,
        source_commit: unsigned.source_commit,
        entries: unsigned.entries,
        manifest_sha256,
    };
    let manifest_path = staging.path().join(MANIFEST_NAME);
    let mut manifest_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(manifest_path)?;
    serde_json::to_writer(&mut manifest_file, &manifest)?;
    manifest_file.write_all(b"\n")?;
    manifest_file.flush()?;
    manifest_file.sync_all()?;

    let staging_name = staging
        .path()
        .file_name()
        .ok_or(PackageError::InvalidInput(
            "staging directory name is missing",
        ))?;
    rustix::fs::renameat_with(
        &parent_fd,
        staging_name,
        &parent_fd,
        output_name,
        RenameFlags::NOREPLACE,
    )?;
    rustix::fs::fsync(&parent_fd)?;

    Ok(PackageReceipt {
        output_dir: canonical_parent.join(output_name),
        manifest,
        archive_created: false,
        installer_complete: false,
    })
}

fn validate_metadata(version: &str, target: &str, source_commit: &str) -> Result<(), PackageError> {
    if !valid_metadata_token(version, b".+-", 128) || !valid_metadata_token(target, b"._+-", 128) {
        return Err(PackageError::InvalidInput(
            "version or target metadata is invalid",
        ));
    }
    if !matches!(source_commit.len(), 40 | 64)
        || !source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(PackageError::InvalidInput(
            "source commit must be a full lowercase object ID",
        ));
    }
    Ok(())
}

fn valid_metadata_token(value: &str, extra: &[u8], max_len: usize) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && value.len() <= max_len
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || extra.contains(&byte))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn package_destination(source: &str) -> Result<(String, InputKind), PackageError> {
    if source.len() > MAX_PATH_BYTES
        || source.starts_with('/')
        || source.contains('\\')
        || source
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PackageError::InvalidInput(
            "source path is not a bounded relative POSIX path",
        ));
    }
    let components: Vec<&str> = source.split('/').collect();
    if components.iter().any(|part| denied_component(part)) {
        return Err(PackageError::InvalidInput(
            "source path contains a private or credential component",
        ));
    }

    let (destination, kind) = match components.as_slice() {
        ["target", "release", binary] if *binary == "admiralty" || *binary == "admiraltyd" => {
            (format!("bin/{binary}"), InputKind::Binary)
        }
        ["apps", "dashboard", "dist", rest @ ..] if !rest.is_empty() => (
            format!("share/admiralty/dashboard/{}", rest.join("/")),
            InputKind::Dashboard,
        ),
        ["schemas", "domain-event-v2.schema.json"] => (
            "share/admiralty/schemas/domain-event-v2.schema.json".to_owned(),
            InputKind::Schema,
        ),
        ["schemas", "guards", "v2", rest @ ..] if !rest.is_empty() => (
            format!("share/admiralty/guards/v2/{}", rest.join("/")),
            InputKind::Guard,
        ),
        ["docs", rest @ ..] if !rest.is_empty() => (
            format!("share/doc/admiralty/{}", rest.join("/")),
            InputKind::Documentation,
        ),
        _ => {
            return Err(PackageError::InvalidInput(
                "source is outside the explicit release allowlist",
            ));
        }
    };
    if destination.len() > MAX_PATH_BYTES
        || destination
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || destination.eq_ignore_ascii_case(MANIFEST_NAME)
    {
        return Err(PackageError::InvalidInput(
            "package destination is invalid or reserved",
        ));
    }
    Ok((destination, kind))
}

fn denied_component(component: &str) -> bool {
    let lower = component.to_ascii_lowercase();
    lower.starts_with(".env")
        || matches!(
            lower.as_str(),
            ".git"
                | ".worktrees"
                | ".claude"
                | ".codex"
                | "private"
                | "credentials"
                | "secrets"
                | "tokens"
        )
        || [
            "token",
            "secret",
            "credential",
            "private",
            "id_rsa",
            "id_ed25519",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        || [".pem", ".key", ".p12", ".pfx", ".token"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

fn checked_directory(path: &Path) -> Result<PathBuf, PackageError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(PackageError::InvalidInput(
                    "directory path contains a parent component",
                ));
            }
            Component::Normal(part) => {
                current.push(part);
                let metadata = fs::symlink_metadata(&current)?;
                if metadata.file_type().is_symlink() {
                    return Err(PackageError::InvalidInput(
                        "symlink path components are forbidden",
                    ));
                }
            }
        }
    }
    if !fs::metadata(&absolute)?.is_dir() {
        return Err(PackageError::InvalidInput(
            "path must name an existing directory",
        ));
    }
    Ok(fs::canonicalize(absolute)?)
}

fn open_directory(path: &Path) -> Result<OwnedFd, PackageError> {
    Ok(rustix::fs::openat(
        rustix::fs::CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?)
}

fn open_source(root: &OwnedFd, source: &str) -> Result<File, PackageError> {
    let components: Vec<&str> = source.split('/').collect();
    let mut directory = rustix::fs::openat(
        root,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for component in &components[..components.len() - 1] {
        directory = rustix::fs::openat(
            &directory,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
    }
    let fd = rustix::fs::openat(
        &directory,
        components[components.len() - 1],
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(fd))
}

fn identity(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.nlink(),
    )
}

fn copy_and_hash(source: &mut File, destination: &mut File) -> Result<(String, u64), PackageError> {
    source.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or(PackageError::InvalidInput("file size overflow"))?;
        if total > MAX_FILE_BYTES {
            return Err(PackageError::InvalidInput("file exceeds 512 MiB"));
        }
        digest.update(&buffer[..count]);
        destination.write_all(&buffer[..count])?;
    }
    Ok((format!("{:x}", digest.finalize()), total))
}

fn hash_open_file(source: &mut File) -> Result<String, PackageError> {
    source.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn fixture() -> (tempfile::TempDir, Vec<PackageInput>) {
        let root = tempfile::tempdir().expect("temporary root");
        let files = [
            ("target/release/admiralty", b"binary".as_slice(), 0o755),
            (
                "apps/dashboard/dist/index.html",
                b"<main>ok</main>".as_slice(),
                0o644,
            ),
            (
                "docs/operators/release.md",
                b"release notes".as_slice(),
                0o644,
            ),
        ];
        let mut inputs = Vec::new();
        for (source, bytes, mode) in files {
            let path = root.path().join(source);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(path, bytes).expect("create source");
            inputs.push(PackageInput {
                source: source.to_owned(),
                expected_sha256: digest(bytes),
                mode,
            });
        }
        let schema = include_bytes!("../../../schemas/domain-event-v2.schema.json");
        let schema_path = root.path().join("schemas/domain-event-v2.schema.json");
        fs::create_dir_all(schema_path.parent().expect("schema parent"))
            .expect("create schema parent");
        fs::write(&schema_path, schema).expect("copy v2 schema");
        inputs.push(PackageInput {
            source: "schemas/domain-event-v2.schema.json".to_owned(),
            expected_sha256: digest(schema),
            mode: 0o644,
        });
        (root, inputs)
    }

    fn stage(
        root: &Path,
        output: &Path,
        inputs: &[PackageInput],
    ) -> Result<PackageReceipt, PackageError> {
        stage_native_package(
            root,
            output,
            "2.0.0",
            "linux-x86_64",
            &"a".repeat(40),
            inputs,
        )
    }

    #[test]
    fn stages_explicit_v2_release_bundle_with_manifest_digest() {
        let (root, inputs) = fixture();
        let output = root.path().join("bundle");
        let receipt = stage(root.path(), &output, &inputs).expect("stage package");
        assert!(!receipt.archive_created);
        assert!(!receipt.installer_complete);
        assert!(receipt.output_dir.is_dir());
        let manifest_bytes = fs::read(output.join(MANIFEST_NAME)).expect("manifest");
        let read_back: PackageManifest =
            serde_json::from_slice(&manifest_bytes).expect("manifest JSON");
        assert_eq!(read_back, receipt.manifest);
        assert!(receipt.manifest.entries.iter().any(|entry| {
            entry.archive_path == "share/admiralty/schemas/domain-event-v2.schema.json"
        }));
        let schema: serde_json::Value = serde_json::from_slice(
            &fs::read(output.join("share/admiralty/schemas/domain-event-v2.schema.json"))
                .expect("staged v2 schema"),
        )
        .expect("v2 schema JSON");
        assert!(schema["$defs"]["CandidateDecisionEvidence"].is_object());
        let unsigned = UnsignedManifest {
            schema_version: receipt.manifest.schema_version,
            version: receipt.manifest.version.clone(),
            target: receipt.manifest.target.clone(),
            source_commit: receipt.manifest.source_commit.clone(),
            entries: receipt.manifest.entries.clone(),
        };
        assert_eq!(
            receipt.manifest.manifest_sha256,
            content_digest(&unsigned).expect("manifest digest")
        );
        for entry in &receipt.manifest.entries {
            let bytes = fs::read(output.join(&entry.archive_path)).expect("staged file");
            assert_eq!(digest(&bytes), entry.sha256);
            assert_eq!(bytes.len() as u64, entry.size);
        }
    }

    #[test]
    fn invalid_allowlist_digest_or_duplicate_leaves_no_partial_output() {
        let (root, inputs) = fixture();
        let output = root.path().join("bad-bundle");
        let mut wrong_digest = inputs.clone();
        wrong_digest[0].expected_sha256 = "0".repeat(64);
        assert!(matches!(
            stage(root.path(), &output, &wrong_digest),
            Err(PackageError::InvalidInput("source SHA-256 mismatch"))
        ));
        assert!(!output.exists());

        let mut traversal = inputs.clone();
        traversal[0].source = "target/release/../private/token".to_owned();
        assert!(stage(root.path(), &output, &traversal).is_err());
        assert!(!output.exists());

        let mut duplicate = inputs.clone();
        duplicate.push(duplicate[0].clone());
        assert!(matches!(
            stage(root.path(), &output, &duplicate),
            Err(PackageError::InvalidInput("duplicate package input"))
        ));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_symlinks_and_preserves_existing_output() {
        let (root, mut inputs) = fixture();
        let original = root.path().join("target/release/admiralty");
        fs::remove_file(&original).expect("remove original binary");
        symlink(root.path().join("docs/operators/release.md"), &original).expect("link binary");
        let output = root.path().join("bundle");
        assert!(stage(root.path(), &output, &inputs).is_err());
        assert!(!output.exists());

        fs::remove_file(&original).expect("remove symlink");
        fs::write(&original, b"binary").expect("restore binary");
        inputs[0].expected_sha256 = digest(b"binary");
        let receipt = stage(root.path(), &output, &inputs).expect("stage clean bundle");
        let manifest_before = fs::read(output.join(MANIFEST_NAME)).expect("manifest bytes");
        assert!(stage(root.path(), &output, &inputs).is_err());
        assert_eq!(
            fs::read(output.join(MANIFEST_NAME)).expect("preserved manifest"),
            manifest_before
        );
        assert!(receipt.output_dir.is_dir());
    }

    #[test]
    fn metadata_and_non_allowlisted_sources_fail_before_output() {
        let (root, inputs) = fixture();
        let output = root.path().join("bundle");
        assert!(stage_native_package(
            root.path(),
            &output,
            "../bad",
            "linux",
            &"a".repeat(40),
            &inputs
        )
        .is_err());
        let mut private = inputs.clone();
        private[0].source = ".git/config".to_owned();
        assert!(stage(root.path(), &output, &private).is_err());
        assert!(!output.exists());
    }
}
