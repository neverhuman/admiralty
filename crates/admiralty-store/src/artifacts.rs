//! Immutable, content-addressed artifact files.
//!
//! This module publishes and verifies bytes only. Database availability, pins,
//! retention, garbage collection, network transfer, and execution authority are
//! owned by higher layers and are deliberately absent here.

use super::{Result, StoreError};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRef {
    /// Lowercase SHA-256 in the form `sha256:<64 hex digits>`.
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ArtifactStore {
    root: PathBuf,
    max_object_bytes: u64,
}

impl ArtifactStore {
    /// Open an existing, owner-only directory for immutable objects.
    pub fn open(root: impl AsRef<Path>, max_object_bytes: u64) -> Result<Self> {
        let root = checked_root(root.as_ref())?;
        Ok(Self {
            root,
            max_object_bytes,
        })
    }

    /// Durably publish bytes once under their digest. Existing objects are
    /// verified and never replaced, including when their contents are corrupt.
    pub fn publish_bytes(&self, bytes: &[u8]) -> Result<ArtifactRef> {
        let size_bytes = u64::try_from(bytes.len())
            .map_err(|_| StoreError::Invalid("artifact size is outside supported range".into()))?;
        self.check_size(size_bytes)?;
        let sha256 = digest(bytes);
        let reference = ArtifactRef { sha256, size_bytes };
        let target = self.object_path(&reference)?;

        if path_exists(&target)? {
            self.verify_path(&target, &reference)?;
            return Ok(reference);
        }

        let temporary = self
            .root
            .join(format!(".tmp-{}-{}", std::process::id(), Uuid::new_v4()));
        let mut temporary_created = false;
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary)?;
            temporary_created = true;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);

            match fs::hard_link(&temporary, &target) {
                Ok(()) => {
                    File::open(&self.root)?.sync_all()?;
                    fs::remove_file(&temporary)?;
                    File::open(&self.root)?.sync_all()?;
                    Ok(reference.clone())
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.verify_path(&target, &reference)?;
                    fs::remove_file(&temporary)?;
                    File::open(&self.root)?.sync_all()?;
                    Ok(reference.clone())
                }
                Err(error) => Err(StoreError::Io(error)),
            }
        })();
        if result.is_err() && temporary_created {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Read a published object only after checking its path, length and digest.
    pub fn read_verified(&self, reference: &ArtifactRef) -> Result<Vec<u8>> {
        let target = self.object_path(reference)?;
        self.read_path(&target, reference)
    }

    fn check_size(&self, size: u64) -> Result<()> {
        if size > self.max_object_bytes {
            return Err(StoreError::Invalid(format!(
                "artifact exceeds configured limit of {} bytes",
                self.max_object_bytes
            )));
        }
        Ok(())
    }

    fn object_path(&self, reference: &ArtifactRef) -> Result<PathBuf> {
        let hex = parse_digest(&reference.sha256)?;
        self.check_size(reference.size_bytes)?;
        Ok(self.root.join(hex))
    }

    fn verify_path(&self, path: &Path, reference: &ArtifactRef) -> Result<()> {
        let _ = self.read_path(path, reference)?;
        Ok(())
    }

    fn read_path(&self, path: &Path, reference: &ArtifactRef) -> Result<Vec<u8>> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StoreError::Invalid(
                "artifact object is not a regular file".into(),
            ));
        }
        let size = metadata.len();
        self.check_size(size)?;
        if size != reference.size_bytes {
            return Err(StoreError::Invalid(
                "artifact size does not match reference".into(),
            ));
        }

        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        let mut file = options.open(path)?;
        let capacity = usize::try_from(size)
            .map_err(|_| StoreError::Invalid("artifact size exceeds platform limits".into()))?;
        let mut bytes = vec![0; capacity];
        file.read_exact(&mut bytes)?;
        let mut trailing = [0; 1];
        if file.read(&mut trailing)? != 0
            || file.metadata()?.len() != size
            || digest(&bytes) != reference.sha256
        {
            return Err(StoreError::Invalid(
                "artifact digest or size verification failed".into(),
            ));
        }
        Ok(bytes)
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn parse_digest(value: &str) -> Result<&str> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(StoreError::Invalid(
            "artifact digest must use sha256".into(),
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(StoreError::Invalid("artifact digest is malformed".into()));
    }
    Ok(hex)
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(StoreError::Io(error)),
    }
}

fn checked_root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(StoreError::Invalid(
            "artifact root must be an absolute normalized path".into(),
        ));
    }
    let mut current = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(name) = component {
            current.push(name);
            let metadata = fs::symlink_metadata(&current)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StoreError::Invalid(
                    "artifact root contains a symlink or non-directory component".into(),
                ));
            }
        }
    }
    let metadata = fs::symlink_metadata(&current)?;
    #[cfg(unix)]
    {
        let uid = rustix::process::geteuid().as_raw();
        let mode = metadata.permissions().mode();
        if metadata.uid() != uid || mode & 0o077 != 0 {
            return Err(StoreError::Invalid(format!(
                "artifact root must be owned by this user and private (owner {}, current {}, mode {:o})",
                metadata.uid(),
                uid,
                mode & 0o777
            )));
        }
        if mode & 0o300 != 0o300 {
            return Err(StoreError::Invalid(
                "artifact root must be writable by its owner".into(),
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        return Err(StoreError::Invalid(
            "artifact store requires Unix ownership and no-follow checks".into(),
        ));
    }
    Ok(current)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{os::unix::fs::symlink, sync::Arc, thread};

    fn store() -> (tempfile::TempDir, ArtifactStore) {
        let directory = tempfile::tempdir().expect("artifact fixture operation succeeds");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("artifact fixture operation succeeds");
        let store = ArtifactStore::open(
            directory
                .path()
                .canonicalize()
                .expect("canonical fixture root"),
            1024,
        )
        .expect("artifact fixture operation succeeds");
        (directory, store)
    }

    #[test]
    fn round_trip_has_stable_sha256() {
        let (_directory, store) = store();
        let reference = store
            .publish_bytes(b"hello")
            .expect("artifact fixture operation succeeds");
        assert_eq!(
            reference.sha256,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(reference.size_bytes, 5);
        assert_eq!(
            store
                .read_verified(&reference)
                .expect("artifact fixture operation succeeds"),
            b"hello"
        );
    }

    #[test]
    fn corrupt_existing_object_is_never_replaced() {
        let (directory, store) = store();
        let reference = ArtifactRef {
            sha256: digest(b"expected"),
            size_bytes: b"corrupt!".len() as u64,
        };
        fs::write(
            directory.path().join(
                parse_digest(&reference.sha256).expect("artifact fixture operation succeeds"),
            ),
            b"corrupt!",
        )
        .expect("artifact fixture operation succeeds");
        assert!(store.publish_bytes(b"expected").is_err());
        assert_eq!(
            fs::read(directory.path().join(
                parse_digest(&reference.sha256).expect("artifact fixture operation succeeds")
            ))
            .expect("artifact fixture operation succeeds"),
            b"corrupt!"
        );
    }

    #[test]
    fn rejects_oversized_input_before_publication() {
        let directory = tempfile::tempdir().expect("artifact fixture operation succeeds");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("artifact fixture operation succeeds");
        let store = ArtifactStore::open(
            directory
                .path()
                .canonicalize()
                .expect("canonical fixture root"),
            4,
        )
        .expect("artifact fixture operation succeeds");
        assert!(store.publish_bytes(b"12345").is_err());
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("artifact fixture operation succeeds")
                .count(),
            0
        );
    }

    #[test]
    fn rejects_symlink_root_and_object() {
        let (directory, store) = store();
        let outside = tempfile::tempdir().expect("artifact fixture operation succeeds");
        let root_link = directory.path().with_extension("link");
        symlink(directory.path(), &root_link).expect("artifact fixture operation succeeds");
        assert!(ArtifactStore::open(&root_link, 100).is_err());

        let reference = ArtifactRef {
            sha256: digest(b"safe"),
            size_bytes: 4,
        };
        symlink(
            outside.path().join("absent"),
            directory.path().join(
                parse_digest(&reference.sha256).expect("artifact fixture operation succeeds"),
            ),
        )
        .expect("artifact fixture operation succeeds");
        assert!(store.read_verified(&reference).is_err());
        assert!(
            store
                .read_verified(&ArtifactRef {
                    sha256: "sha256:../../etc/passwd".into(),
                    size_bytes: 0,
                })
                .is_err()
        );
        assert!(
            store
                .read_verified(&ArtifactRef {
                    sha256: format!("sha256:{}A", "0".repeat(63)),
                    size_bytes: 0,
                })
                .is_err()
        );
        fs::remove_file(root_link).expect("artifact fixture operation succeeds");
    }

    #[test]
    fn concurrent_publishers_converge_on_one_object() {
        let (_directory, store) = store();
        let store = Arc::new(store);
        let publishers: Vec<_> = (0..8)
            .map(|_| {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    store
                        .publish_bytes(b"same immutable bytes")
                        .expect("artifact fixture operation succeeds")
                })
            })
            .collect();
        let references: Vec<_> = publishers
            .into_iter()
            .map(|publisher| {
                publisher
                    .join()
                    .expect("artifact fixture operation succeeds")
            })
            .collect();
        assert!(
            references
                .iter()
                .all(|reference| reference == &references[0])
        );
        assert_eq!(
            store
                .read_verified(&references[0])
                .expect("artifact fixture operation succeeds"),
            b"same immutable bytes"
        );
    }
}
