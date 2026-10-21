//! Atomic `current`/`previous` version links for a clean, reversible deploy.
//!
//! Layout under an owned release root:
//!
//! ```text
//! ROOT/versions/<name>/bin/{admiralty,admiraltyd,admiralty-observe}
//! ROOT/versions/<name>/share/admiralty/dashboard/
//! ROOT/current  -> versions/<name>     (relative symlink)
//! ROOT/previous -> versions/<older>
//! ```
//!
//! `release install --destination ROOT/versions/<name>` creates a version;
//! `release activate` switches `current` with one atomic rename and records the
//! replaced target as `previous`; `release rollback` switches back. Service
//! units refer only to `ROOT/current`, so a restart picks up the new version.
//! These commands never start, stop or signal a service, never delete a
//! version and never follow a symlink inside `versions/`.
use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};

type ActivateResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const CURRENT: &str = "current";
pub const PREVIOUS: &str = "previous";
pub const VERSIONS: &str = "versions";
/// Files a version must contain before it can become current.
pub const REQUIRED_FILES: [&str; 3] = ["bin/admiraltyd", "bin/admiralty-observe", "bin/admiralty"];

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !matches!(name, "." | "..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(unix)]
fn check_root(root: &Path) -> ActivateResult<()> {
    use std::os::unix::fs::MetadataExt;
    if !root.is_absolute() {
        return Err("release root must be an absolute path".into());
    }
    let uid = rustix::process::geteuid().as_raw();
    let meta = std::fs::symlink_metadata(root)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o022 != 0 {
        return Err(
            "release root must be a directory owned by this user without group/other write".into(),
        );
    }
    let versions = std::fs::symlink_metadata(root.join(VERSIONS))?;
    if !versions.is_dir() || versions.uid() != uid || versions.mode() & 0o022 != 0 {
        return Err("versions/ must be an owned directory without group/other write".into());
    }
    Ok(())
}

#[cfg(unix)]
fn check_version(root: &Path, name: &str) -> ActivateResult<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    if !valid_name(name) {
        return Err("version name must be 1-128 of [A-Za-z0-9._-] and not . or ..".into());
    }
    let relative = Path::new(VERSIONS).join(name);
    let directory = root.join(&relative);
    let meta = std::fs::symlink_metadata(&directory)
        .map_err(|_| format!("version {name} is not installed under versions/"))?;
    if !meta.is_dir() {
        return Err("a version must be a real directory, not a symlink".into());
    }
    let uid = rustix::process::geteuid().as_raw();
    for file in REQUIRED_FILES {
        let path = directory.join(file);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| format!("version {name} is missing {file}"))?;
        if !meta.is_file() || meta.uid() != uid || meta.mode() & 0o111 == 0 {
            return Err(format!("{file} must be an owned executable regular file").into());
        }
    }
    Ok(relative)
}

/// Current target of a version link, as a version name.
#[cfg(unix)]
pub fn link_target(root: &Path, link: &str) -> ActivateResult<Option<String>> {
    match std::fs::symlink_metadata(root.join(link)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(meta) if !meta.file_type().is_symlink() => {
            Err(format!("{link} exists but is not a symlink; refusing to replace it").into())
        }
        Ok(_) => {
            let target = std::fs::read_link(root.join(link))?;
            let mut parts = target.components();
            match (parts.next(), parts.next(), parts.next()) {
                (
                    Some(std::path::Component::Normal(dir)),
                    Some(std::path::Component::Normal(name)),
                    None,
                ) if dir == VERSIONS => Ok(name.to_str().map(str::to_owned)),
                _ => Err(format!(
                    "{link} does not point at versions/<name>; refusing to manage it"
                )
                .into()),
            }
        }
    }
}

#[cfg(unix)]
fn replace_link(root: &Path, link: &str, relative: &Path) -> ActivateResult<()> {
    let staging = root.join(format!(".{link}.{}", uuid::Uuid::new_v4().simple()));
    std::os::unix::fs::symlink(relative, &staging)?;
    if let Err(error) = std::fs::rename(&staging, root.join(link)) {
        let _ = std::fs::remove_file(&staging);
        return Err(error.into());
    }
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}

/// Make `name` current. Idempotent when it already is.
#[cfg(unix)]
pub fn activate(root: &Path, name: &str) -> ActivateResult<Value> {
    check_root(root)?;
    let relative = check_version(root, name)?;
    let current = link_target(root, CURRENT)?;
    let previous_before = link_target(root, PREVIOUS)?;
    if current.as_deref() == Some(name) {
        return Ok(
            json!({"current":name,"previous":previous_before,"changed":false,
            "service_control":false,"next_step":"No change; restart services only if they run another version."}),
        );
    }
    // Record the outgoing version first: a crash between the two renames
    // leaves previous == current, never a lost rollback target.
    if let Some(old) = &current {
        replace_link(root, PREVIOUS, &Path::new(VERSIONS).join(old))?;
    }
    replace_link(root, CURRENT, &relative)?;
    Ok(
        json!({"current":name,"previous":current.or(previous_before),"changed":true,
        "service_control":false,
        "next_step":"Restart the services that run from current/, then check /readyz."}),
    )
}

/// Swap back to `previous`.
#[cfg(unix)]
pub fn rollback(root: &Path) -> ActivateResult<Value> {
    check_root(root)?;
    let previous = link_target(root, PREVIOUS)?.ok_or("no previous version is recorded")?;
    activate(root, &previous)
}

#[cfg(unix)]
pub fn status(root: &Path) -> ActivateResult<Value> {
    check_root(root)?;
    let mut versions = std::fs::read_dir(root.join(VERSIONS))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| valid_name(n))
        .collect::<Vec<_>>();
    versions.sort();
    Ok(
        json!({"current":link_target(root, CURRENT)?,"previous":link_target(root, PREVIOUS)?,
        "installed":versions,"service_control":false}),
    )
}

#[cfg(not(unix))]
pub fn activate(_: &Path, _: &str) -> ActivateResult<Value> {
    Err("release activation is qualified on Unix only".into())
}
#[cfg(not(unix))]
pub fn rollback(_: &Path) -> ActivateResult<Value> {
    Err("release activation is qualified on Unix only".into())
}
#[cfg(not(unix))]
pub fn status(_: &Path) -> ActivateResult<Value> {
    Err("release activation is qualified on Unix only".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn version(root: &Path, name: &str) {
        let bin = root.join(VERSIONS).join(name).join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        for file in ["admiraltyd", "admiralty-observe", "admiralty"] {
            std::fs::write(bin.join(file), "#!/bin/false\n").expect("binary");
            std::fs::set_permissions(bin.join(file), std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
    }

    fn root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("root");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod");
        std::fs::create_dir(dir.path().join(VERSIONS)).expect("versions");
        std::fs::set_permissions(
            dir.path().join(VERSIONS),
            std::fs::Permissions::from_mode(0o700),
        )
        .expect("chmod");
        dir
    }

    #[test]
    fn activate_switches_atomically_records_previous_and_rolls_back() {
        let dir = root();
        let root = dir.path();
        version(root, "v1");
        version(root, "v2");
        let first = activate(root, "v1").expect("activate v1");
        assert_eq!(first["changed"], true);
        assert_eq!(first["previous"], Value::Null);
        assert_eq!(
            std::fs::read_link(root.join(CURRENT)).expect("link"),
            Path::new("versions/v1")
        );
        assert!(root.join(CURRENT).join("bin/admiraltyd").is_file());
        let second = activate(root, "v2").expect("activate v2");
        assert_eq!(second["previous"], "v1");
        assert_eq!(
            link_target(root, PREVIOUS).expect("prev").as_deref(),
            Some("v1")
        );
        assert_eq!(activate(root, "v2").expect("idempotent")["changed"], false);
        let back = rollback(root).expect("rollback");
        assert_eq!(back["current"], "v1");
        assert_eq!(back["previous"], "v2");
        let listed = status(root).expect("status");
        assert_eq!(listed["installed"], json!(["v1", "v2"]));
        // No staging links are left behind.
        assert!(
            std::fs::read_dir(root)
                .expect("list")
                .filter_map(Result::ok)
                .all(|e| !e.file_name().to_string_lossy().starts_with('.'))
        );
    }

    #[test]
    fn incomplete_symlinked_or_unsafe_versions_are_refused() {
        let dir = root();
        let root = dir.path();
        version(root, "good");
        for name in ["", "..", "a/b", "../good", "missing"] {
            assert!(activate(root, name).is_err(), "{name:?}");
        }
        std::os::unix::fs::symlink(
            root.join(VERSIONS).join("good"),
            root.join(VERSIONS).join("alias"),
        )
        .expect("alias");
        assert!(activate(root, "alias").is_err());
        version(root, "partial");
        std::fs::remove_file(root.join(VERSIONS).join("partial/bin/admiralty-observe"))
            .expect("rm");
        assert!(activate(root, "partial").is_err());
        assert!(rollback(root).is_err(), "nothing to roll back to");
        // A foreign `current` file is never replaced.
        std::fs::write(root.join(CURRENT), "not a link").expect("file");
        assert!(activate(root, "good").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(CURRENT)).expect("kept"),
            "not a link"
        );
    }

    #[test]
    fn shared_writable_root_is_refused() {
        let dir = root();
        version(dir.path(), "v1");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777))
            .expect("chmod");
        assert!(activate(dir.path(), "v1").is_err());
        assert!(activate(Path::new("relative"), "v1").is_err());
    }
}
