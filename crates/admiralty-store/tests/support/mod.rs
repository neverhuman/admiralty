//! Owner-only fixtures, including on shared runners with umask0002.
pub fn private_tempdir() -> tempfile::TempDir {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("create private Store fixture")
    }
    #[cfg(not(unix))]
    {
        tempfile::tempdir().expect("create Store fixture")
    }
}
