//! Read-only P31 release evidence preflight. Never deploys or grants acceptance.
//!
//! Native port of `tests/qualification/release_evidence.py` with the same
//! gates, bounds and output contract:
//! * the manifest is at most 1 MiB and duplicate JSON keys are invalid;
//! * every passed gate binds the exact expected source revision, a UTC
//!   interval that is not in the future, and the SHA-256 of a regular evidence
//!   file (at most 64 MiB) opened component-by-component without following
//!   symlinks;
//! * gates that need actual journeys reject synthetic evidence, the soak needs
//!   at least 24 observed hours, and the cost trial can never authorize a
//!   savings claim.
//!
//! Exit status: 0 ready for independent review, 1 incomplete, 2 invalid.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value, json};
use std::path::Path;

pub const MAX_MANIFEST: u64 = 1024 * 1024;
pub const MAX_ARTIFACT: u64 = 64 * 1024 * 1024;
pub const REQUIRED: [&str; 12] = [
    "invariant_matrix",
    "load_query",
    "synthetic_soak",
    "clean_install",
    "provider_transport",
    "restart_continuity",
    "backup_restore",
    "rollback",
    "staged_canary",
    "fleet_handoff",
    "context_fidelity",
    "complete_cost_trial",
];
const SYNTHETIC_ALLOWED: [&str; 3] = ["invariant_matrix", "load_query", "synthetic_soak"];

type Invalid = String;

/// JSON value whose objects may not repeat a key at any depth.
struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor).map(Unique)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON without duplicate object keys")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Unique(value)) = seq.next_element()? {
            items.push(value);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            let Unique(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

pub fn parse_unique(bytes: &[u8]) -> Result<Value, Invalid> {
    serde_json::from_slice::<Unique>(bytes)
        .map(|Unique(v)| v)
        .map_err(|e| e.to_string())
}

fn timestamp(value: &Value) -> Result<chrono::DateTime<chrono::Utc>, Invalid> {
    let text = value
        .as_str()
        .filter(|s| s.ends_with('Z'))
        .ok_or("UTC timestamps must end in Z")?;
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|_| "UTC timestamp required".into())
}

fn lower_hex(text: &str, lengths: &[usize]) -> bool {
    lengths.contains(&text.len()) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// SHA-256 of `relative` under `root`, opening each component relative to a
/// held directory descriptor with no-follow, so a symlink or directory swap
/// cannot redirect the read outside the evidence root.
#[cfg(unix)]
pub fn artifact_hash(root: &Path, relative: &Value) -> Result<String, Invalid> {
    use rustix::fs::{Mode, OFlags, fstat, openat};
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let relative = relative
        .as_str()
        .filter(|r| !r.is_empty() && r.len() <= 1024)
        .ok_or("bounded relative artifact path required")?;
    let parts = relative.split('/').collect::<Vec<_>>();
    if parts.iter().any(|p| matches!(*p, "" | "." | "..")) {
        return Err("artifact path must stay within the evidence root".into());
    }
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rustix::fs::open(root, dir_flags, Mode::empty())
        .map_err(|e| format!("evidence root unavailable: {e}"))?;
    for part in &parts[..parts.len() - 1] {
        directory = openat(&directory, *part, dir_flags, Mode::empty())
            .map_err(|e| format!("artifact directory unavailable: {e}"))?;
    }
    let descriptor = openat(
        &directory,
        parts[parts.len() - 1],
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("artifact unavailable: {e}"))?;
    let before = fstat(&descriptor).map_err(|e| e.to_string())?;
    let regular =
        rustix::fs::FileType::from_raw_mode(before.st_mode) == rustix::fs::FileType::RegularFile;
    let size = u64::try_from(before.st_size).unwrap_or(u64::MAX);
    if !regular || size > MAX_ARTIFACT {
        return Err("artifact must be a regular file up to 64 MiB".into());
    }
    let mut file = std::fs::File::from(descriptor);
    let mut digest = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = vec![0_u8; 65_536];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > MAX_ARTIFACT {
            return Err("artifact exceeds 64 MiB".into());
        }
        digest.update(&buffer[..read]);
    }
    let after = fstat(&file).map_err(|e| e.to_string())?;
    let stamp = |s: &rustix::fs::Stat| {
        (
            s.st_size,
            s.st_mtime,
            s.st_mtime_nsec,
            s.st_ctime,
            s.st_ctime_nsec,
        )
    };
    if count != size || stamp(&before) != stamp(&after) {
        return Err("artifact changed while hashing".into());
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(not(unix))]
pub fn artifact_hash(_: &Path, _: &Value) -> Result<String, Invalid> {
    Err("no-follow evidence hashing is qualified on Unix only".into())
}

pub fn qualify(
    manifest: &Value,
    root: &Path,
    expected_revision: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, Invalid> {
    if !lower_hex(expected_revision, &[40, 64]) {
        return Err("full expected source revision required".into());
    }
    if manifest["schema_version"] != 1 || manifest["source_revision"] != expected_revision {
        return Err("manifest must bind the exact expected source revision".into());
    }
    let checks = manifest["checks"]
        .as_array()
        .filter(|c| c.len() <= 64)
        .ok_or("up to 64 named checks required")?;
    let mut seen = Vec::<&str>::new();
    let mut failures = Vec::<String>::new();
    let mut verified = Vec::<&str>::new();
    for check in checks {
        let name = check["id"]
            .as_str()
            .and_then(|id| REQUIRED.iter().copied().find(|r| *r == id))
            .filter(|id| !seen.contains(id))
            .ok_or("unknown or duplicate qualification gate")?;
        seen.push(name);
        let status = check["status"]
            .as_str()
            .filter(|s| matches!(*s, "passed" | "failed" | "unavailable"))
            .ok_or("explicit gate status required")?;
        if status != "passed" {
            failures.push(format!("{name}: {status}"));
            continue;
        }
        if check["revision"] != expected_revision {
            failures.push(format!("{name}: stale source revision"));
            continue;
        }
        let kind = check["kind"].as_str();
        if !matches!(kind, Some("actual" | "synthetic"))
            || (!SYNTHETIC_ALLOWED.contains(&name) && kind != Some("actual"))
        {
            failures.push(format!("{name}: actual supported journey required"));
            continue;
        }
        let (start, finish) = (
            timestamp(&check["started_utc"])?,
            timestamp(&check["finished_utc"])?,
        );
        if finish < start || finish > now {
            failures.push(format!("{name}: invalid or future observation interval"));
            continue;
        }
        if name == "synthetic_soak" && (finish - start).num_seconds() < 86_400 {
            failures.push(format!(
                "{name}: at least 24 hours of observed elapsed time required"
            ));
            continue;
        }
        let digest = check["sha256"]
            .as_str()
            .filter(|d| lower_hex(d, &[64]))
            .ok_or("exact SHA256 required for passed evidence")?;
        // As in the retired Python preflight, an unreadable, unsafe or
        // non-regular artifact makes the manifest invalid; a readable file
        // with different bytes is a failed gate.
        if artifact_hash(root, &check["evidence"])? != digest {
            failures.push(format!("{name}: artifact hash mismatch"));
            continue;
        }
        if name == "complete_cost_trial" && check["savings_claim"] != false {
            // Economics is reviewed separately. Complete evidence may retain a
            // negative/inconclusive trial without manufacturing savings.
            failures.push(format!(
                "{name}: preflight cannot authorize a savings claim"
            ));
            continue;
        }
        verified.push(name);
    }
    failures.extend(
        REQUIRED
            .iter()
            .filter(|name| !seen.contains(name))
            .map(|name| format!("{name}: missing")),
    );
    Ok(json!({
        "schema_version":1,"source_revision":expected_revision,
        "status":if failures.is_empty() {"ready_for_independent_review"} else {"incomplete"},
        "verified_artifacts":verified,"remaining":failures,
        "full_packet_acceptance":false,"deployment_authorized":false,"savings_claim":false,
        "scope":"Source/time/hash preflight of declared evidence; independent review of contents and actual fleet ownership is required."
    }))
}

/// Run the CLI command; returns the process exit status (0, 1 or 2).
pub fn run(manifest: &Path, evidence_root: &Path, expected_revision: &str) -> u8 {
    let result = (|| -> Result<Value, Invalid> {
        use std::io::Read;
        let mut raw = Vec::new();
        std::fs::File::open(manifest)
            .map_err(|e| e.to_string())?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut raw)
            .map_err(|e| e.to_string())?;
        if raw.len() as u64 > MAX_MANIFEST {
            return Err("manifest exceeds 1 MiB".into());
        }
        let manifest = parse_unique(&raw)?;
        qualify(
            &manifest,
            evidence_root,
            expected_revision,
            chrono::Utc::now(),
        )
    })();
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            if report["status"] == "ready_for_independent_review" {
                0
            } else {
                1
            }
        }
        Err(error) => {
            println!(
                "{}",
                json!({"status":"invalid","error":error,"full_packet_acceptance":false,"deployment_authorized":false})
            );
            2
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    struct Fixture {
        _scratch: tempfile::TempDir,
        root: std::path::PathBuf,
        revision: String,
        digest: String,
        now: chrono::DateTime<chrono::Utc>,
    }

    fn fixture() -> Fixture {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().to_path_buf();
        let artifact = root.join("evidence.json");
        std::fs::write(&artifact, "{\"synthetic_fixture\":true}\n").expect("artifact");
        let digest = format!(
            "{:x}",
            Sha256::digest(std::fs::read(&artifact).expect("read"))
        );
        Fixture {
            _scratch: scratch,
            root,
            revision: "a".repeat(40),
            digest,
            now: "2026-10-19T14:20:00Z".parse().expect("now"),
        }
    }

    fn manifest(f: &Fixture) -> Value {
        json!({"schema_version":1,"source_revision":f.revision,"checks":REQUIRED.iter().map(|name| json!({
            "id":name,"status":"passed","kind":if SYNTHETIC_ALLOWED.contains(name) {"synthetic"} else {"actual"},
            "revision":f.revision,"started_utc":"2026-10-18T14:00:00Z","finished_utc":"2026-10-19T14:00:00Z",
            "evidence":"evidence.json","sha256":f.digest,"savings_claim":false})).collect::<Vec<_>>()})
    }

    fn qualified(f: &Fixture, m: &Value) -> Result<Value, Invalid> {
        qualify(m, &f.root, &f.revision, f.now)
    }

    #[test]
    fn complete_synthetic_metadata_fixture_only_admits_independent_review() {
        let f = fixture();
        let result = qualified(&f, &manifest(&f)).expect("valid");
        assert_eq!(result["status"], "ready_for_independent_review");
        assert_eq!(
            result["verified_artifacts"].as_array().expect("list").len(),
            12
        );
        assert_eq!(result["full_packet_acceptance"], false);
        assert_eq!(result["deployment_authorized"], false);
        assert_eq!(result["savings_claim"], false);
    }

    #[test]
    fn missing_failed_unknown_and_short_soak_remain_incomplete() {
        let f = fixture();
        let mut m = manifest(&f);
        let checks = m["checks"].as_array_mut().expect("checks");
        checks.pop();
        checks[0]["status"] = json!("failed");
        checks[1]["status"] = json!("unavailable");
        checks[2]["started_utc"] = json!("2026-10-19T13:00:00Z");
        let result = qualified(&f, &m).expect("valid");
        assert_eq!(result["status"], "incomplete");
        assert_eq!(result["remaining"].as_array().expect("list").len(), 4);
    }

    #[test]
    fn stale_head_future_timestamp_and_synthetic_provider_cannot_pass() {
        let f = fixture();
        for (key, value) in [
            ("revision", json!("b".repeat(40))),
            ("finished_utc", json!("2026-10-20T14:00:00Z")),
            ("kind", json!("synthetic")),
        ] {
            let mut m = manifest(&f);
            m["checks"][4][key] = value;
            assert_eq!(
                qualified(&f, &m).expect("valid")["status"],
                "incomplete",
                "{key}"
            );
        }
    }

    #[test]
    fn modified_evidence_and_savings_assertions_are_not_accepted() {
        let f = fixture();
        let mut m = manifest(&f);
        m["checks"][11]["savings_claim"] = json!(true);
        let result = qualified(&f, &m).expect("valid");
        assert!(
            result["remaining"][0]
                .as_str()
                .expect("reason")
                .contains("cannot authorize a savings claim")
        );
        std::fs::write(f.root.join("evidence.json"), "changed").expect("change");
        let result = qualified(&f, &manifest(&f)).expect("valid");
        assert!(
            result["verified_artifacts"]
                .as_array()
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn duplicate_unknown_and_wrong_manifest_revision_are_invalid() {
        let f = fixture();
        let mut m = manifest(&f);
        let first = m["checks"][0].clone();
        m["checks"].as_array_mut().expect("checks").push(first);
        assert!(qualified(&f, &m).is_err());
        let mut m = manifest(&f);
        m["source_revision"] = json!("b".repeat(40));
        assert!(qualified(&f, &m).is_err());
        assert!(parse_unique(br#"{"checks":[],"checks":[]}"#).is_err());
        assert!(parse_unique(br#"{"a":{"b":1,"b":2}}"#).is_err());
        assert_eq!(
            parse_unique(br#"{"a":[1,{"b":2}]}"#).expect("valid")["a"][1]["b"],
            2
        );
    }

    #[test]
    fn traversal_symlink_and_nonregular_artifacts_are_refused() {
        let f = fixture();
        std::os::unix::fs::symlink(f.root.join("evidence.json"), f.root.join("alias"))
            .expect("alias");
        std::fs::create_dir(f.root.join("directory")).expect("dir");
        std::os::unix::fs::symlink(&f.root, f.root.join("dir-alias")).expect("dir alias");
        for path in [
            "../evidence.json",
            "/etc/passwd",
            "alias",
            "dir-alias/evidence.json",
            "directory",
        ] {
            assert!(artifact_hash(&f.root, &json!(path)).is_err(), "{path}");
        }
        assert_eq!(
            artifact_hash(&f.root, &json!("evidence.json")).expect("hash"),
            f.digest
        );
    }

    #[test]
    fn missing_or_symlinked_evidence_invalidates_the_manifest() {
        let f = fixture();
        let mut m = manifest(&f);
        m["checks"][3]["evidence"] = json!("absent.json");
        assert!(qualified(&f, &m).is_err());
        std::os::unix::fs::symlink(f.root.join("evidence.json"), f.root.join("alias"))
            .expect("alias");
        let mut m = manifest(&f);
        m["checks"][3]["evidence"] = json!("alias");
        assert!(qualified(&f, &m).is_err());
    }

    #[test]
    fn zero_checks_reports_every_missing_gate() {
        let f = fixture();
        let result = qualified(
            &f,
            &json!({"schema_version":1,"source_revision":f.revision,"checks":[]}),
        )
        .expect("valid");
        assert_eq!(result["remaining"].as_array().expect("list").len(), 12);
    }

    #[test]
    fn cli_exit_status_matches_the_retired_python_contract() {
        let f = fixture();
        let path = f.root.join("manifest.json");
        std::fs::write(&path, manifest(&f).to_string()).expect("manifest");
        // The fixture interval ends in the past relative to the real clock.
        assert_eq!(run(&path, &f.root, &f.revision), 0);
        std::fs::write(
            &path,
            br#"{"schema_version":1,"source_revision":"x","checks":[]}"#,
        )
        .expect("bad");
        assert_eq!(run(&path, &f.root, &f.revision), 2);
        std::fs::write(
            &path,
            json!({"schema_version":1,"source_revision":f.revision,"checks":[]}).to_string(),
        )
        .expect("empty");
        assert_eq!(run(&path, &f.root, &f.revision), 1);
    }
}
