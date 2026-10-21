//! Build-time reads pinned below a directory, with actual-byte bounds.
use rustix::fs::{Mode, OFlags, open, openat};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

pub fn read_asset(root: &Path, relative: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components.len() > 32
        || !components.iter().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid asset path",
        ));
    }
    let directories = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open(root, directories, Mode::empty())?;
    for component in &components[..components.len() - 1] {
        directory = openat(
            &directory,
            component.as_os_str(),
            directories,
            Mode::empty(),
        )?;
    }
    let descriptor = openat(
        &directory,
        components.last().expect("nonempty").as_os_str(),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut file = File::from(descriptor);
    let before = file.metadata()?;
    if !before.is_file() || before.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "asset is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() as u64 > limit
        || bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || (
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        ) != (
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        )
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "asset changed while being read",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn bounded_reads_refuse_replaced_files_parents_and_special_files() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        fs::create_dir(root.path().join("assets")).expect("assets");
        fs::write(root.path().join("assets/view.js"), b"four").expect("file");
        assert_eq!(
            read_asset(root.path(), Path::new("assets/view.js"), 4).expect("bounded"),
            b"four"
        );
        assert!(read_asset(root.path(), Path::new("assets/view.js"), 3).is_err());
        fs::write(outside.path().join("view.js"), b"outside").expect("outside file");
        // The walk can have observed a regular file before replacement.
        assert!(
            fs::symlink_metadata(root.path().join("assets/view.js"))
                .expect("observed")
                .is_file()
        );
        fs::remove_file(root.path().join("assets/view.js")).expect("replace");
        symlink(
            outside.path().join("view.js"),
            root.path().join("assets/view.js"),
        )
        .expect("link");
        assert!(read_asset(root.path(), Path::new("assets/view.js"), 1024).is_err());
        symlink(outside.path(), root.path().join("linked")).expect("linked parent");
        assert!(read_asset(root.path(), Path::new("linked/view.js"), 1024).is_err());
        assert!(read_asset(root.path(), Path::new("../view.js"), 1024).is_err());
        let fifo = root.path().join("pipe.js");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        assert!(read_asset(root.path(), Path::new("pipe.js"), 1024).is_err());
        assert!(read_asset(root.path(), Path::new("assets"), 1024).is_err());
    }
}
