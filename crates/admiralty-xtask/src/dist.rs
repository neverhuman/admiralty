//! `cargo xtask dist`: one command from source to a single deployable binary.
//!
//! 1. Build the dashboard (`NODE_ENV=production pnpm --dir apps/dashboard build`).
//! 2. Build release binaries with `ADMIRALTY_EMBED_DASHBOARD` pointing at that
//!    `dist/`, so `admiralty serve` and `admiraltyd` carry the console inside
//!    the executable (see `crates/admiralty-server/build.rs`).
//! 3. Copy `admiralty` (plus the compatibility `admiraltyd` and
//!    `admiralty-observe` executables) into a fresh output directory with a
//!    `SHA256SUMS` file and a `BUILD.json` provenance record.
//!
//! It never installs, activates, signs or publishes anything; the existing
//! `admiralty release stage|archive|install` commands remain the reviewed path
//! for private version directories.
use crate::{Error, process};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};

/// Executables copied into the output directory, in a stable order.
pub const BINARIES: [&str; 3] = ["admiralty", "admiraltyd", "admiralty-observe"];

#[derive(clap::Args, Debug, Clone)]
pub struct DistArgs {
    /// New output directory (must not exist). Defaults to
    /// `target/dist/admiralty-<version>-<os>-<arch>`.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Reuse an existing `apps/dashboard/dist` instead of rebuilding it.
    #[arg(long)]
    pub skip_dashboard_build: bool,
}

/// Default output directory name for this host.
pub fn default_out(root: &Path, target_dir: &Path) -> PathBuf {
    debug_assert!(target_dir.starts_with(root) || target_dir.is_absolute());
    target_dir.join("dist").join(format!(
        "admiralty-{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

/// The release build invocation; kept separate so tests pin the exact flags.
pub fn release_build_command() -> Vec<OsString> {
    [
        "cargo",
        "build",
        "--release",
        "--locked",
        "-p",
        "admiralty-cli",
        "-p",
        "admiralty-server",
        "-p",
        "admiralty-observe",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// Lowercase hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<String, Error> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// `sha256sum`-compatible manifest for `files` (name, digest) pairs.
pub fn sha256sums(entries: &[(String, String)]) -> String {
    entries
        .iter()
        .map(|(name, digest)| format!("{digest}  {name}\n"))
        .collect()
}

fn run(root: &Path, argv: &[OsString], overlay: &[(OsString, OsString)]) -> Result<i32, Error> {
    let mut command = process::command(argv, overlay)?;
    command.current_dir(root).stdin(Stdio::null());
    let status = process::status(&mut command)?;
    Ok(process::exit_code(status))
}

fn capture(root: &Path, argv: &[&str]) -> Option<String> {
    let argv: Vec<OsString> = argv.iter().map(OsString::from).collect();
    let mut command = process::command(&argv, &[]).ok()?;
    let output = command
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn create_new_dir(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::DirBuilder::new()
        .mode(0o755)
        .create(path)
        .map_err(|error| {
            format!(
                "output directory {} must not already exist: {error}",
                path.display()
            )
            .into()
        })
}

fn copy_executable(source: &Path, destination: &Path) -> Result<(), Error> {
    let bytes = fs::read(source)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(destination)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    file.sync_all()?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// `cargo xtask dist`.
pub fn run_dist(root: &Path, args: &DistArgs) -> Result<i32, Error> {
    let target_dir =
        crate::launch::launcher_target_dir(root, std::env::var_os("ADMIRALTY_TARGET_DIR"));
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| default_out(root, &target_dir));
    if out.exists() {
        eprintln!(
            "xtask dist: {} already exists; choose a new --out",
            out.display()
        );
        return Ok(2);
    }
    let dist = root.join("apps/dashboard/dist");
    if !args.skip_dashboard_build {
        let build: Vec<OsString> = ["pnpm", "--dir", "apps/dashboard", "build"]
            .into_iter()
            .map(OsString::from)
            .collect();
        let code = run(
            root,
            &build,
            &[(OsString::from("NODE_ENV"), OsString::from("production"))],
        )?;
        if code != 0 {
            eprintln!("xtask dist: dashboard build failed (exit {code})");
            return Ok(code);
        }
    }
    let dist = dist.canonicalize().map_err(|error| {
        format!("dashboard dist is missing ({error}); run without --skip-dashboard-build")
    })?;
    if !dist.join("index.html").is_file() {
        eprintln!("xtask dist: {} has no index.html", dist.display());
        return Ok(2);
    }
    let overlay = vec![
        (OsString::from("CARGO_BUILD_JOBS"), OsString::from("2")),
        (
            OsString::from("ADMIRALTY_EMBED_DASHBOARD"),
            dist.clone().into_os_string(),
        ),
        (
            OsString::from("CARGO_TARGET_DIR"),
            target_dir.clone().into_os_string(),
        ),
    ];
    let code = run(root, &release_build_command(), &overlay)?;
    if code != 0 {
        eprintln!("xtask dist: release build failed (exit {code})");
        return Ok(code);
    }
    create_new_dir(&out)?;
    let bin = out.join("bin");
    fs::DirBuilder::new().mode(0o755).create(&bin)?;
    let mut sums = Vec::new();
    for name in BINARIES {
        let source = target_dir.join("release").join(name);
        let destination = bin.join(name);
        copy_executable(&source, &destination)?;
        sums.push((format!("bin/{name}"), sha256_file(&destination)?));
    }
    let doctor = capture(
        root,
        &[bin.join("admiralty").to_string_lossy().as_ref(), "doctor"],
    )
    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let embedded = doctor
        .as_ref()
        .map(|value| value["embedded_dashboard"].clone())
        .unwrap_or(serde_json::Value::Null);
    if embedded.is_null() {
        eprintln!("xtask dist: the built binary reports no embedded dashboard");
        return Ok(1);
    }
    let build = serde_json::json!({
        "schema_version": 1,
        "version": env!("CARGO_PKG_VERSION"),
        "target": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "source_commit": capture(root, &["git", "rev-parse", "HEAD"]),
        "source_dirty": capture(root, &["git", "status", "--porcelain"]).map(|text| !text.is_empty()),
        "rustc": capture(root, &["rustc", "--version"]),
        "embedded_dashboard": embedded,
        "binaries": sums.iter().map(|(name, digest)| serde_json::json!({"path": name, "sha256": digest})).collect::<Vec<_>>(),
        "note": "Build provenance only. Not signed, installed, activated or qualified."
    });
    fs::write(
        out.join("BUILD.json"),
        format!("{}\n", serde_json::to_string_pretty(&build)?),
    )?;
    fs::write(out.join("SHA256SUMS"), sha256sums(&sums))?;
    println!("Admiralty distribution: {}", out.display());
    print!("{}", sha256sums(&sums));
    println!(
        "Run it with: {}/bin/admiralty serve --database <db> --token-file <token> (dashboard embedded)",
        out.display()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_build_embeds_every_shipped_binary_package() {
        let argv: Vec<String> = release_build_command()
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv[..4], ["cargo", "build", "--release", "--locked"]);
        for package in ["admiralty-cli", "admiralty-server", "admiralty-observe"] {
            assert!(argv.windows(2).any(|pair| pair == ["-p", package]));
        }
    }

    #[test]
    fn sums_use_sha256sum_layout_and_default_out_names_the_host() {
        let text = sha256sums(&[("bin/admiralty".into(), "ab".repeat(32))]);
        assert_eq!(text, format!("{}  bin/admiralty\n", "ab".repeat(32)));
        let out = default_out(Path::new("/repo"), Path::new("/repo/target"));
        let name = out.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        assert!(name.starts_with("admiralty-") && name.contains(std::env::consts::ARCH));
        assert!(out.starts_with("/repo/target/dist"));
    }

    #[test]
    fn file_digest_matches_known_vector() -> Result<(), Error> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("abc");
        fs::write(&path, b"abc")?;
        assert_eq!(
            sha256_file(&path)?,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        Ok(())
    }
}
