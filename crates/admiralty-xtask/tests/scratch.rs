//! Private scratch creation changes the process umask, so it runs in its own
//! test binary (one test, no concurrent file creation).

use admiralty_xtask::check::create_scratch;
use rustix::{fs::Mode, process::umask};
use std::os::unix::fs::MetadataExt;

#[test]
fn scratch_is_private_canonical_and_restores_the_caller_umask() {
    let parent = tempfile::tempdir().expect("tempdir");
    let previous = umask(Mode::from_raw_mode(0o002));
    let result = create_scratch(parent.path());
    let restored = umask(previous);
    let (scratch, caller) = result.expect("create scratch");
    assert_eq!(caller, 0o002, "reports the caller umask");
    assert_eq!(restored.bits(), 0o002, "caller umask restored for children");
    assert_eq!(scratch, scratch.canonicalize().expect("canonical"));
    assert!(scratch.starts_with(parent.path().canonicalize().expect("canonical parent")));
    let name = scratch
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with("adm."));
    assert_eq!(name.len(), "adm.".len() + 6);
    for dir in [
        scratch.clone(),
        scratch.join("logs"),
        scratch.join("canary"),
    ] {
        let meta = std::fs::symlink_metadata(&dir).expect("stat");
        assert!(meta.is_dir());
        assert_eq!(meta.mode() & 0o7777, 0o700, "{}", dir.display());
    }
    std::fs::remove_dir_all(&scratch).expect("cleanup");
}
