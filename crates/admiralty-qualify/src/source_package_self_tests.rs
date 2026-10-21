//! The ten `PackageTests` scenarios of the retired `scripts/release/package.py`,
//! runnable programmatically ([`super::self_test`]) and as `#[test]`s. The
//! Python mock-based race tests are reproduced with [`super::Hooks`].

use std::collections::BTreeMap;
use std::fs::{self, File, FileTimes};
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::{
    Hooks, MANIFEST_PATH, MAX_FILE_BYTES, MAX_INPUT_JSON, Manifest, PackageResult,
    SourcePackageError, build_package_with_hooks,
};
use crate::py::{self, Dump, PyValue};

pub(super) type Scenario = fn(&Fixture) -> Result<(), String>;

pub(super) const SCENARIOS: [(&str, Scenario); 10] = [
    (
        "test_reproducible_across_input_order_and_source_mtime",
        reproducible_across_input_order_and_source_mtime,
    ),
    (
        "test_manifest_hashes_and_archive_metadata",
        manifest_hashes_and_archive_metadata,
    ),
    (
        "test_duplicate_traversal_private_and_reserved_names_rejected",
        duplicate_traversal_private_and_reserved_names_rejected,
    ),
    (
        "test_symlink_files_and_parents_are_rejected",
        symlink_files_and_parents_are_rejected,
    ),
    (
        "test_missing_oversized_and_invalid_metadata_rejected",
        missing_oversized_and_invalid_metadata_rejected,
    ),
    (
        "test_output_no_overwrite_symlink_and_race",
        output_no_overwrite_symlink_and_race,
    ),
    (
        "test_source_mutation_during_capture_is_detected",
        source_mutation_during_capture_is_detected,
    ),
    (
        "test_bounded_json_and_hardlink_rejected",
        bounded_json_and_hardlink_rejected,
    ),
    (
        "test_portable_case_prefix_reserved_and_duplicate_json_collisions",
        portable_case_prefix_reserved_and_duplicate_json_collisions,
    ),
    (
        "test_pinned_source_parent_survives_ancestor_swap_without_following_link",
        pinned_source_parent_survives_ancestor_swap_without_following_link,
    ),
];

macro_rules! ensure {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(format!($($message)+));
        }
    };
}

fn io_err(context: &str) -> impl Fn(std::io::Error) -> String + '_ {
    move |error| format!("{context}: {error}")
}

/// `self.assertRaises(SourcePackageError)`.
fn expect_invalid(result: PackageResult<Manifest>, context: &str) -> Result<(), String> {
    match result {
        Err(SourcePackageError::Invalid(_)) => Ok(()),
        Err(SourcePackageError::Io(error)) => Err(format!(
            "{context}: expected SourcePackageError, got OSError {error}"
        )),
        Ok(_) => Err(format!(
            "{context}: expected SourcePackageError, package was built"
        )),
    }
}

/// `self.assertRaises((OSError, SourcePackageError))`.
fn expect_error(result: PackageResult<Manifest>, context: &str) -> Result<(), String> {
    match result {
        Err(_) => Ok(()),
        Ok(_) => Err(format!("{context}: expected an error, package was built")),
    }
}

fn record(source: &str, archive_path: &str, mode: i32) -> PyValue {
    crate::py_dict! { "source" => source, "archive_path" => archive_path, "mode" => mode }
}

fn with_field(base: &PyValue, key: &str, value: &str) -> PyValue {
    let mut copy = base.clone();
    if let Some(dict) = copy.as_dict_mut() {
        dict.insert(key, value);
    }
    copy
}

/// The per-test `setUp` fixture.
pub(super) struct Fixture {
    _temporary: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    inputs: PathBuf,
    output: PathBuf,
    records: Vec<PyValue>,
    commit: String,
}

impl Fixture {
    fn new() -> Result<Self, String> {
        let scratch = std::env::var_os("ADM_PACKAGE_TEST_SCRATCH")
            .map_or_else(std::env::temp_dir, PathBuf::from);
        fs::create_dir_all(&scratch).map_err(io_err("create scratch"))?;
        let temporary = tempfile::Builder::new()
            .prefix("p30a-test-")
            .tempdir_in(&scratch)
            .map_err(io_err("create test directory"))?;
        // Resolve platform temp aliases: production refuses symlink ancestors.
        let base = temporary
            .path()
            .canonicalize()
            .map_err(io_err("resolve test directory"))?;
        let root = base.join("input");
        fs::create_dir(&root).map_err(io_err("mkdir input"))?;
        fs::create_dir(root.join("bin")).map_err(io_err("mkdir bin"))?;
        fs::create_dir(root.join("assets")).map_err(io_err("mkdir assets"))?;
        fs::write(root.join("bin").join("demo"), b"binary-one").map_err(io_err("write demo"))?;
        fs::write(root.join("assets").join("index.html"), "hello")
            .map_err(io_err("write index"))?;
        let fixture = Self {
            _temporary: temporary,
            inputs: base.join("inputs.json"),
            output: base.join("package.tar.gz"),
            records: vec![
                record("bin/demo", "bin/demo", 493),
                record("assets/index.html", "share/admiralty/index.html", 420),
            ],
            commit: "a".repeat(40),
            base,
            root,
        };
        fixture.write_inputs(&fixture.records)?;
        Ok(fixture)
    }

    fn write_inputs(&self, records: &[PyValue]) -> Result<(), String> {
        let document = crate::py_dict! { "files" => records.to_vec() };
        fs::write(&self.inputs, py::dumps(&document, Dump::DEFAULT)).map_err(io_err("write inputs"))
    }

    fn package_with(
        &self,
        output: Option<&Path>,
        labels: (&str, &str, Option<&str>),
        hooks: &mut Hooks<'_>,
    ) -> PackageResult<Manifest> {
        build_package_with_hooks(
            &self.root,
            &self.inputs,
            output.unwrap_or(&self.output),
            (labels.0, labels.1, labels.2.unwrap_or(&self.commit)),
            hooks,
        )
    }

    fn package(&self, output: Option<&Path>) -> PackageResult<Manifest> {
        self.package_with(
            output,
            ("1.2.3", "linux-x86_64", None),
            &mut Hooks::default(),
        )
    }
}

struct Member {
    name: String,
    mode: u32,
    mtime: u64,
    uid: u64,
    gid: u64,
    uname: String,
    gname: String,
}

type Members = (Vec<Member>, BTreeMap<String, Vec<u8>>);

fn members(path: &Path) -> Result<Members, String> {
    let file = File::open(path).map_err(io_err("open archive"))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut infos = Vec::new();
    let mut contents = BTreeMap::new();
    for entry in archive.entries().map_err(io_err("read archive"))? {
        let mut entry = entry.map_err(io_err("read member"))?;
        let header = entry.header().clone();
        let name = entry
            .path()
            .map_err(io_err("member path"))?
            .to_string_lossy()
            .into_owned();
        let text = |field: Result<Option<&str>, std::str::Utf8Error>| {
            field.ok().flatten().unwrap_or_default().to_owned()
        };
        infos.push(Member {
            name: name.clone(),
            mode: header.mode().map_err(io_err("mode"))?,
            mtime: header.mtime().map_err(io_err("mtime"))?,
            uid: header.uid().map_err(io_err("uid"))?,
            gid: header.gid().map_err(io_err("gid"))?,
            uname: text(header.username()),
            gname: text(header.groupname()),
        });
        if header.entry_type().is_file() {
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(io_err("member data"))?;
            contents.insert(name, data);
        }
    }
    Ok((infos, contents))
}

fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn fail(error: SourcePackageError) -> String {
    format!("unexpected package error: {error}")
}

/// Ported from scripts/release/package.py::PackageTests::test_reproducible_across_input_order_and_source_mtime
fn reproducible_across_input_order_and_source_mtime(fx: &Fixture) -> Result<(), String> {
    fx.package(None).map_err(fail)?;
    let first = fs::read(&fx.output).map_err(io_err("read first"))?;
    let stamp = UNIX_EPOCH + Duration::from_secs(123_456_789);
    File::options()
        .write(true)
        .open(fx.root.join("bin").join("demo"))
        .and_then(|file| file.set_times(FileTimes::new().set_accessed(stamp).set_modified(stamp)))
        .map_err(io_err("utime"))?;
    let reversed: Vec<PyValue> = fx.records.iter().rev().cloned().collect();
    fx.write_inputs(&reversed)?;
    let second_output = fx.base.join("second.tar.gz");
    fx.package(Some(&second_output)).map_err(fail)?;
    let second = fs::read(&second_output).map_err(io_err("read second"))?;
    ensure!(
        first == second,
        "archives differ across input order and source mtime"
    );
    Ok(())
}

/// Ported from scripts/release/package.py::PackageTests::test_manifest_hashes_and_archive_metadata
fn manifest_hashes_and_archive_metadata(fx: &Fixture) -> Result<(), String> {
    fx.package(None).map_err(fail)?;
    let raw = fs::read(&fx.output).map_err(io_err("read archive"))?;
    ensure!(
        raw.len() > 10 && raw[4..8] == [0, 0, 0, 0],
        "gzip mtime is not zero"
    );
    ensure!(raw[3] & 0x08 == 0, "gzip FNAME flag is set");
    let (infos, contents) = members(&fx.output)?;
    let names: Vec<&str> = infos.iter().map(|info| info.name.as_str()).collect();
    ensure!(
        names == ["MANIFEST.json", "bin/demo", "share/admiralty/index.html"],
        "unexpected members {names:?}"
    );
    let manifest_bytes = contents.get(MANIFEST_PATH).ok_or("manifest missing")?;
    let manifest = py::loads_bytes(manifest_bytes, false).map_err(|e| e.to_string())?;
    let field = |key: &str| {
        manifest
            .get(key)
            .and_then(PyValue::as_str)
            .map(str::to_owned)
    };
    ensure!(
        field("version").as_deref() == Some("1.2.3"),
        "version mismatch"
    );
    ensure!(
        field("target").as_deref() == Some("linux-x86_64"),
        "target mismatch"
    );
    ensure!(
        field("source_commit").as_deref() == Some(fx.commit.as_str()),
        "commit mismatch"
    );
    let entries = manifest
        .get("entries")
        .and_then(PyValue::as_list)
        .ok_or("entries missing")?;
    for entry in entries {
        let path = entry
            .get("archive_path")
            .and_then(PyValue::as_str)
            .ok_or("entry path missing")?;
        let data = contents
            .get(path)
            .ok_or_else(|| format!("member {path} missing"))?;
        let digest = entry.get("sha256").and_then(PyValue::as_str);
        ensure!(
            digest == Some(sha256_hex(data).as_str()),
            "digest mismatch for {path}"
        );
        let size = entry.get("size").cloned().unwrap_or(PyValue::Null);
        ensure!(
            py::py_eq(&size, &PyValue::from(data.len())),
            "size mismatch for {path}"
        );
    }
    for info in &infos {
        ensure!(info.mtime == 0, "{} mtime {}", info.name, info.mtime);
        ensure!(info.uid == 0, "{} uid {}", info.name, info.uid);
        ensure!(info.gid == 0, "{} gid {}", info.name, info.gid);
        ensure!(
            info.uname.is_empty(),
            "{} uname {:?}",
            info.name,
            info.uname
        );
        ensure!(
            info.gname.is_empty(),
            "{} gname {:?}",
            info.name,
            info.gname
        );
    }
    let modes: Vec<u32> = infos.iter().map(|info| info.mode).collect();
    ensure!(modes == [0o644, 0o755, 0o644], "unexpected modes {modes:?}");
    Ok(())
}

/// Ported from scripts/release/package.py::PackageTests::test_duplicate_traversal_private_and_reserved_names_rejected
fn duplicate_traversal_private_and_reserved_names_rejected(fx: &Fixture) -> Result<(), String> {
    let bad_sets = [
        vec![
            fx.records[0].clone(),
            with_field(&fx.records[1], "archive_path", "bin/./demo"),
        ],
        vec![with_field(&fx.records[0], "archive_path", "../escape")],
        vec![with_field(
            &fx.records[0],
            "archive_path",
            ".env.production",
        )],
        vec![with_field(&fx.records[0], "archive_path", "MANIFEST.json")],
    ];
    for (index, records) in bad_sets.iter().enumerate() {
        fx.write_inputs(records)?;
        let output = fx.base.join(format!("bad-{index}.tgz"));
        expect_invalid(fx.package(Some(&output)), &format!("bad set {index}"))?;
    }
    fx.write_inputs(&[with_field(&fx.records[0], "source", ".ssh/id_ed25519")])?;
    expect_invalid(
        fx.package(Some(&fx.base.join("private.tgz"))),
        "private source",
    )
}

/// Ported from scripts/release/package.py::PackageTests::test_symlink_files_and_parents_are_rejected
fn symlink_files_and_parents_are_rejected(fx: &Fixture) -> Result<(), String> {
    let link = fx.root.join("bin").join("linked");
    symlink(fx.root.join("bin").join("demo"), &link).map_err(io_err("symlink file"))?;
    fx.write_inputs(&[record("bin/linked", "bin/linked", 420)])?;
    expect_invalid(
        fx.package(Some(&fx.base.join("symlink.tgz"))),
        "symlink file",
    )?;
    let parent = fx.root.join("linked-dir");
    symlink(fx.root.join("assets"), &parent).map_err(io_err("symlink dir"))?;
    fx.write_inputs(&[record("linked-dir/index.html", "index.html", 420)])?;
    expect_invalid(
        fx.package(Some(&fx.base.join("symlink-parent.tgz"))),
        "symlink parent",
    )?;
    let output_parent = fx.base.join("output-link");
    symlink(&fx.base, &output_parent).map_err(io_err("symlink output parent"))?;
    expect_invalid(
        fx.package(Some(&output_parent.join("unsafe.tgz"))),
        "symlink output parent",
    )
}

/// Ported from scripts/release/package.py::PackageTests::test_missing_oversized_and_invalid_metadata_rejected
fn missing_oversized_and_invalid_metadata_rejected(fx: &Fixture) -> Result<(), String> {
    fx.write_inputs(&[record("missing", "missing", 420)])?;
    expect_error(
        fx.package(Some(&fx.base.join("missing.tgz"))),
        "missing source",
    )?;
    let big = fx.root.join("oversized");
    File::create(&big)
        .and_then(|file| file.set_len(MAX_FILE_BYTES + 1))
        .map_err(io_err("truncate oversized"))?;
    fx.write_inputs(&[record("oversized", "oversized", 420)])?;
    expect_invalid(
        fx.package(Some(&fx.base.join("oversized.tgz"))),
        "oversized source",
    )?;
    let commit = fx.commit.clone();
    for (version, target, commit) in [
        ("../bad", "linux", commit.as_str()),
        ("1.0", "../bad", commit.as_str()),
        ("1", "linux", "short"),
    ] {
        let output = fx.base.join(format!("invalid-{version}.tgz"));
        let result = fx.package_with(
            Some(&output),
            (version, target, Some(commit)),
            &mut Hooks::default(),
        );
        expect_invalid(result, &format!("metadata {version}/{target}/{commit}"))?;
    }
    Ok(())
}

/// Ported from scripts/release/package.py::PackageTests::test_output_no_overwrite_symlink_and_race
fn output_no_overwrite_symlink_and_race(fx: &Fixture) -> Result<(), String> {
    fs::write(&fx.output, b"keep").map_err(io_err("write existing output"))?;
    expect_invalid(fx.package(None), "existing output")?;
    ensure!(
        fs::read(&fx.output).map_err(io_err("read kept output"))? == b"keep",
        "existing output was modified"
    );
    fs::remove_file(&fx.output).map_err(io_err("unlink output"))?;
    let link = fx.base.join("out-link.tgz");
    symlink(fx.base.join("missing-target"), &link).map_err(io_err("symlink output"))?;
    expect_invalid(fx.package(Some(&link)), "dangling output symlink")?;
    fs::remove_file(&link).map_err(io_err("unlink output symlink"))?;
    let mut hooks = Hooks {
        before_publish_link: Some(Box::new(|destination: &Path| {
            let _ = fs::write(destination, b"racer");
        })),
        ..Hooks::default()
    };
    let result = fx.package_with(None, ("1.2.3", "linux-x86_64", None), &mut hooks);
    expect_invalid(result, "racing output")?;
    ensure!(
        fs::read(&fx.output).map_err(io_err("read racer"))? == b"racer",
        "racing output was replaced"
    );
    Ok(())
}

/// Ported from scripts/release/package.py::PackageTests::test_source_mutation_during_capture_is_detected
fn source_mutation_during_capture_is_detected(fx: &Fixture) -> Result<(), String> {
    let demo = fx.root.join("bin").join("demo");
    let mut fired = false;
    let mut hooks = Hooks {
        after_capture_chunk: Some(Box::new(|| {
            if !fired {
                fired = true;
                let _ = fs::write(&demo, b"binary-two");
            }
        })),
        ..Hooks::default()
    };
    let result = fx.package_with(None, ("1.2.3", "linux-x86_64", None), &mut hooks);
    drop(hooks);
    expect_invalid(result, "mutated source")?;
    ensure!(
        fs::symlink_metadata(&fx.output).is_err(),
        "output exists after mutation"
    );
    Ok(())
}

/// Ported from scripts/release/package.py::PackageTests::test_bounded_json_and_hardlink_rejected
fn bounded_json_and_hardlink_rejected(fx: &Fixture) -> Result<(), String> {
    fs::write(&fx.inputs, vec![b' '; MAX_INPUT_JSON + 1]).map_err(io_err("write big inputs"))?;
    expect_invalid(fx.package(None), "oversized input JSON")?;
    fs::remove_file(&fx.inputs).map_err(io_err("unlink inputs"))?;
    symlink(fx.base.join("missing-input-json"), &fx.inputs).map_err(io_err("symlink inputs"))?;
    expect_invalid(fx.package(None), "symlinked input JSON")?;
    fs::remove_file(&fx.inputs).map_err(io_err("unlink inputs symlink"))?;
    fs::hard_link(
        fx.root.join("bin").join("demo"),
        fx.root.join("bin").join("hardlink"),
    )
    .map_err(io_err("hard link"))?;
    fx.write_inputs(&[record("bin/demo", "bin/demo", 420)])?;
    expect_invalid(fx.package(None), "hard-linked source")
}

/// Ported from scripts/release/package.py::PackageTests::test_portable_case_prefix_reserved_and_duplicate_json_collisions
fn portable_case_prefix_reserved_and_duplicate_json_collisions(fx: &Fixture) -> Result<(), String> {
    for (first, second) in [
        ("bin/Foo", "bin/foo"),
        ("bin/demo", "bin/demo/child"),
        ("manifest.JSON", "ok"),
        ("bin/CON.exe", "ok"),
    ] {
        fx.write_inputs(&[
            with_field(&fx.records[0], "archive_path", first),
            with_field(&fx.records[1], "archive_path", second),
        ])?;
        expect_invalid(fx.package(None), &format!("collision {first}/{second}"))?;
    }
    fs::write(&fx.inputs, br#"{"files":[],"files":[]}"#).map_err(io_err("write dup JSON"))?;
    expect_invalid(fx.package(None), "duplicate JSON key")
}

/// Ported from scripts/release/package.py::PackageTests::test_pinned_source_parent_survives_ancestor_swap_without_following_link
fn pinned_source_parent_survives_ancestor_swap_without_following_link(
    fx: &Fixture,
) -> Result<(), String> {
    let source_dir = fx.root.join("bin");
    let moved_dir = fx.root.join("moved-bin");
    let outside = fx.base.join("outside");
    fs::create_dir(&outside).map_err(io_err("mkdir outside"))?;
    fs::write(outside.join("demo"), b"foreign").map_err(io_err("write foreign"))?;
    let mut fired = false;
    let mut swap_error = None;
    let mut hooks = Hooks {
        before_source_open: Some(Box::new(|name: &std::ffi::OsStr| {
            if name == "demo" && !fired {
                fired = true;
                let swapped = fs::rename(&source_dir, &moved_dir)
                    .and_then(|()| symlink(&outside, &source_dir));
                if let Err(error) = swapped {
                    swap_error = Some(error.to_string());
                }
            }
        })),
        ..Hooks::default()
    };
    let result = fx.package_with(None, ("1.2.3", "linux-x86_64", None), &mut hooks);
    drop(hooks);
    if let Some(error) = swap_error {
        return Err(format!("ancestor swap failed: {error}"));
    }
    result.map_err(fail)?;
    let (_, contents) = members(&fx.output)?;
    ensure!(
        contents.get("bin/demo").map(Vec::as_slice) == Some(&b"binary-one"[..]),
        "captured bytes followed the swapped ancestor"
    );
    ensure!(fired, "swap hook did not fire");
    Ok(())
}

/// Run one scenario in a fresh fixture (`setUp`/`tearDown`).
pub(super) fn run_scenario(scenario: Scenario) -> Result<(), String> {
    let fixture = Fixture::new()?;
    scenario(&fixture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(scenario: Scenario) {
        if let Err(reason) = run_scenario(scenario) {
            panic!("{reason}");
        }
    }

    /// Ported from scripts/release/package.py::PackageTests::test_reproducible_across_input_order_and_source_mtime
    #[test]
    fn test_reproducible_across_input_order_and_source_mtime() {
        check(reproducible_across_input_order_and_source_mtime);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_manifest_hashes_and_archive_metadata
    #[test]
    fn test_manifest_hashes_and_archive_metadata() {
        check(manifest_hashes_and_archive_metadata);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_duplicate_traversal_private_and_reserved_names_rejected
    #[test]
    fn test_duplicate_traversal_private_and_reserved_names_rejected() {
        check(duplicate_traversal_private_and_reserved_names_rejected);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_symlink_files_and_parents_are_rejected
    #[test]
    fn test_symlink_files_and_parents_are_rejected() {
        check(symlink_files_and_parents_are_rejected);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_missing_oversized_and_invalid_metadata_rejected
    #[test]
    fn test_missing_oversized_and_invalid_metadata_rejected() {
        check(missing_oversized_and_invalid_metadata_rejected);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_output_no_overwrite_symlink_and_race
    #[test]
    fn test_output_no_overwrite_symlink_and_race() {
        check(output_no_overwrite_symlink_and_race);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_source_mutation_during_capture_is_detected
    #[test]
    fn test_source_mutation_during_capture_is_detected() {
        check(source_mutation_during_capture_is_detected);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_bounded_json_and_hardlink_rejected
    #[test]
    fn test_bounded_json_and_hardlink_rejected() {
        check(bounded_json_and_hardlink_rejected);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_portable_case_prefix_reserved_and_duplicate_json_collisions
    #[test]
    fn test_portable_case_prefix_reserved_and_duplicate_json_collisions() {
        check(portable_case_prefix_reserved_and_duplicate_json_collisions);
    }

    /// Ported from scripts/release/package.py::PackageTests::test_pinned_source_parent_survives_ancestor_swap_without_following_link
    #[test]
    fn test_pinned_source_parent_survives_ancestor_swap_without_following_link() {
        check(pinned_source_parent_survives_ancestor_swap_without_following_link);
    }

    /// Ported from scripts/release/package.py `--self-test`: ten scenarios, no failures.
    #[test]
    fn self_test_runs_all_ten_scenarios() {
        let report = super::super::self_test();
        assert_eq!(report.cases_run, 10);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
    }
}
