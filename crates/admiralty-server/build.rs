//! Optionally embed a built dashboard (`apps/dashboard/dist`) into `admiraltyd`.
//!
//! Embedding is opt-in so ordinary `cargo build`/`cargo test` never depend on
//! a prior `vite build`: set `ADMIRALTY_EMBED_DASHBOARD` to the absolute path
//! of a built `dist/` directory. The script copies each regular file into
//! `OUT_DIR`, precompresses text assets with gzip (level 9, mtime 0) and
//! generates a sorted table that `src/embedded.rs` includes. Without the
//! variable the table is empty and the daemon behaves exactly as before.
//!
//! Bounds mirror the runtime asset handler: at most 1,024 files, 16 MiB per
//! file and 64 MiB in total. Symlinks, special files, dot-files (for example the
//! e2e harness's `.admiralty-build-identity.json`) and non-portable names are
//! refused or skipped, so a release can never embed something unexpected.
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};
use std::{
    env,
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

#[path = "src/embedded_input.rs"]
mod embedded_input;

const MAX_ENTRIES: usize = 8192;
const MAX_FILES: usize = 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

struct Entry {
    relative: String,
    source: PathBuf,
}

fn portable(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component.len() <= 255
        && component
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@' | b'+'))
}

fn walk(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<Entry>,
    total: &mut u64,
    visited: &mut usize,
) {
    assert!(
        directory
            .strip_prefix(root)
            .expect("under root")
            .components()
            .count()
            <= 32,
        "dashboard directory is too deep"
    );
    let mut children: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .take(MAX_ENTRIES + 1)
        .map(|entry| entry.expect("dashboard directory entry"))
        .collect();
    assert!(children.len() <= MAX_ENTRIES, "too many dashboard entries");
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        *visited += 1;
        assert!(*visited <= MAX_ENTRIES, "too many dashboard entries");
        let name = child.file_name();
        let name = name
            .to_str()
            .unwrap_or_else(|| panic!("non-UTF-8 dashboard path in {}", directory.display()));
        if name.starts_with('.') {
            // Harness identity files and editor droppings are not shipped.
            continue;
        }
        assert!(portable(name), "non-portable dashboard file name: {name}");
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).expect("dashboard entry metadata");
        let file_type = metadata.file_type();
        assert!(
            !file_type.is_symlink(),
            "dashboard symlinks are refused: {}",
            path.display()
        );
        if file_type.is_dir() {
            walk(root, &path, entries, total, visited);
            continue;
        }
        assert!(
            file_type.is_file(),
            "dashboard special files are refused: {}",
            path.display()
        );
        assert!(
            metadata.len() <= MAX_FILE_BYTES,
            "dashboard file exceeds 16 MiB: {}",
            path.display()
        );
        *total += metadata.len();
        assert!(*total <= MAX_TOTAL_BYTES, "dashboard exceeds 64 MiB");
        let relative = path
            .strip_prefix(root)
            .expect("walked path stays under root")
            .components()
            .map(|c| c.as_os_str().to_str().expect("checked UTF-8").to_owned())
            .collect::<Vec<_>>()
            .join("/");
        entries.push(Entry {
            relative,
            source: path,
        });
        assert!(entries.len() <= MAX_FILES, "dashboard has too many files");
    }
}

fn mime(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("json") => "application/json",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn compressible(mime: &str) -> bool {
    mime.starts_with("text/") || matches!(mime, "application/json" | "image/svg+xml")
}

fn main() {
    println!("cargo:rerun-if-env-changed=ADMIRALTY_EMBED_DASHBOARD");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let payload = out.join("dashboard");
    if payload.exists() {
        fs::remove_dir_all(&payload).expect("reset embedded dashboard payload");
    }
    fs::create_dir_all(&payload).expect("create embedded dashboard payload");

    let mut entries = Vec::new();
    let mut asset_root = None;
    if let Some(root) = env::var_os("ADMIRALTY_EMBED_DASHBOARD").filter(|v| !v.is_empty()) {
        let root = PathBuf::from(root);
        assert!(
            root.is_absolute(),
            "ADMIRALTY_EMBED_DASHBOARD must be an absolute path to a built dist/ directory"
        );
        let metadata = fs::symlink_metadata(&root).unwrap_or_else(|error| {
            panic!(
                "ADMIRALTY_EMBED_DASHBOARD {} is unavailable: {error}",
                root.display()
            )
        });
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "ADMIRALTY_EMBED_DASHBOARD must name a real directory, not a symlink"
        );
        println!("cargo:rerun-if-changed={}", root.display());
        let mut total = 0;
        let mut visited = 0;
        walk(&root, &root, &mut entries, &mut total, &mut visited);
        asset_root = Some(root);
        assert!(
            entries.iter().any(|e| e.relative == "index.html"),
            "ADMIRALTY_EMBED_DASHBOARD has no index.html; run the dashboard build first"
        );
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));

    let mut table = String::new();
    let mut tree = Sha256::new();
    let mut embedded_bytes = 0u64;
    for (index, entry) in entries.iter().enumerate() {
        println!("cargo:rerun-if-changed={}", entry.source.display());
        let bytes = embedded_input::read_asset(
            asset_root.as_deref().expect("nonempty embedded root"),
            Path::new(&entry.relative),
            MAX_FILE_BYTES,
        )
        .expect("read pinned bounded dashboard file");
        embedded_bytes += bytes.len() as u64;
        assert!(
            embedded_bytes <= MAX_TOTAL_BYTES,
            "dashboard bytes exceed 64 MiB"
        );
        let digest = format!("{:x}", Sha256::digest(&bytes));
        tree.update(entry.relative.as_bytes());
        tree.update([0]);
        tree.update(digest.as_bytes());
        tree.update([b'\n']);
        let body = payload.join(format!("{index:04}.bin"));
        fs::write(&body, &bytes).expect("stage embedded dashboard file");
        let kind = mime(&entry.relative);
        let gzip = if compressible(kind) && bytes.len() >= 1024 {
            let mut encoder = GzBuilder::new()
                .mtime(0)
                .write(Vec::new(), Compression::best());
            encoder.write_all(&bytes).expect("gzip dashboard file");
            let compressed = encoder.finish().expect("finish gzip");
            (compressed.len() < bytes.len()).then(|| {
                let path = payload.join(format!("{index:04}.gz"));
                fs::write(&path, compressed).expect("stage gzip variant");
                path
            })
        } else {
            None
        };
        writeln!(
            table,
            "    EmbeddedAsset {{ path: {path:?}, mime: {kind:?}, sha256: {digest:?}, body: include_bytes!({body:?}), gzip: {gzip} }},",
            path = entry.relative,
            body = body.to_str().expect("UTF-8 OUT_DIR"),
            gzip = match gzip {
                Some(path) => format!(
                    "Some(include_bytes!({:?}))",
                    path.to_str().expect("UTF-8 OUT_DIR")
                ),
                None => "None".to_owned(),
            },
        )
        .expect("format embedded table");
    }
    let digest = if entries.is_empty() {
        "None".to_owned()
    } else {
        format!("Some({:?})", format!("{:x}", tree.finalize()))
    };
    let generated = format!(
        "// @generated by crates/admiralty-server/build.rs; do not edit.\n\
         pub(crate) static EMBEDDED_ASSETS: &[EmbeddedAsset] = &[\n{table}];\n\
         /// SHA-256 over sorted `path NUL sha256 LF` records; `None` when nothing is embedded.\n\
         pub(crate) const EMBEDDED_TREE_SHA256: Option<&str> = {digest};\n"
    );
    fs::write(out.join("embedded_dashboard.rs"), generated).expect("write embedded table");
}
