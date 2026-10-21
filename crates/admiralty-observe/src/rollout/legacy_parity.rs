//! Parity with the retired `dashboard/rollout.py` regression suites
//! (`tests/legacy/test_rollout.py` and `tests/legacy/test_rollout_live_workers.py`).
//! Each test names the Python case it replaces; adapted assertions follow the
//! Rust observer's input/output shape (a configured manifest path, the fleet
//! screen inventory array and the published `rollout` object).
use super::*;
use crate::config::ObserveConfig;
use chrono::{DateTime, Duration, TimeZone, Utc};
use std::{fs, path::PathBuf, process::Command};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0)
        .single()
        .expect("fixed fixture clock")
}
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339()
}
fn manifest() -> Value {
    json!({"schema_version":1,"project":{"id":"build-123","name":"Example build"},
        "updated_utc":stamp(now()),"producer":"test/v1",
        "packets":[{"id":"P01","title":"Foundation","status":"ready","depends_on":[]}],
        "lanes":[],"pr":{"url":"https://example.com/repo/pull/1","head":"abc123"}})
}
fn verification(revision: &str) -> Value {
    json!({"receipt":"receipts/verification.json","revision":revision,"verifier":"reviewer","observed_utc":stamp(now())})
}
fn integration(revision: &str) -> Value {
    json!({"receipt":"receipts/integration.json","revision":revision,"target_ref":"refs/heads/main","observed_utc":stamp(now())})
}
fn load(value: &Value) -> crate::Result<Value> {
    normalize(
        value.clone(),
        json!({"loaded_utc":stamp(now())}),
        &json!([]),
        "node",
        now(),
    )
}
fn ready(value: &Value) -> Value {
    load(value).expect("valid manifest fixture")
}

/// A private directory holding a configured manifest path.
struct Loader {
    dir: tempfile::TempDir,
}
impl Loader {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("private fixture directory"),
        }
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("rollout.json")
    }
    fn config(&self, manifest: Option<PathBuf>) -> ObserveConfig {
        let mut config: ObserveConfig = serde_json::from_value(
            json!({"schema_version":1,"node_id":"node","output_dir":self.dir.path()}),
        )
        .expect("observer configuration fixture");
        config.rollout_manifest = manifest;
        config
    }
    fn collect_at(&self, path: PathBuf) -> Value {
        collect(&self.config(Some(path)), &json!([])).expect("configured manifest is reported")
    }
    fn write(&self, value: &Value) {
        fs::write(
            self.path(),
            serde_json::to_vec(value).expect("fixture JSON"),
        )
        .expect("write manifest fixture");
    }
    fn state(&self, bytes: &[u8]) -> Value {
        fs::write(self.path(), bytes).expect("write manifest fixture");
        self.collect_at(self.path())["state"].clone()
    }
}

/// Parity with tests/legacy/test_rollout.py::test_missing_default_is_optional
/// (adapted: Rust has no implicit runtime default path; an unconfigured manifest
/// is optional and publishes no rollout section).
#[test]
fn missing_default_is_optional() {
    let loader = Loader::new();
    loader.write(&manifest());
    assert!(collect(&loader.config(None), &json!([])).is_none());
}

/// Parity with tests/legacy/test_rollout.py::test_explicit_missing_manifest_is_visible.
/// The Rust state label is `invalid` (Python: `unavailable`); either way an
/// explicitly configured but missing manifest stays visible and read-only.
#[test]
fn explicit_missing_manifest_is_visible() {
    let loader = Loader::new();
    let state = loader.collect_at(loader.dir.path().join("missing.json"));
    assert_eq!(state["state"], "invalid");
    assert_eq!(state["read_only"], true);
    assert_eq!(state["source"]["filename"], "missing.json");
    assert!(
        state["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
    assert!(state.get("packets").is_none());
}

/// Parity with tests/legacy/test_rollout.py::test_explicit_manifest_overrides_default.
#[test]
fn explicit_manifest_overrides_default() {
    let loader = Loader::new();
    loader.write(&json!({"schema_version":2}));
    let other = loader.dir.path().join("another.json");
    fs::write(&other, manifest().to_string()).expect("write configured manifest");
    let state = loader.collect_at(other);
    assert_eq!(state["source"]["filename"], "another.json");
    assert_eq!(state["state"], "ready");
}

/// Parity with tests/legacy/test_rollout.py::test_valid_manifest_has_computed_counts_hash_and_source.
#[test]
fn valid_manifest_has_computed_counts_hash_and_source() {
    let loader = Loader::new();
    let mut value = manifest();
    value["updated_utc"] = json!(Utc::now().to_rfc3339());
    loader.write(&value);
    let state = loader.collect_at(loader.path());
    assert_eq!(state["counts"]["ready"], 1);
    assert_eq!(state["total"], 1);
    let bytes = fs::read(loader.path()).expect("manifest bytes");
    assert_eq!(
        state["source"]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(state["freshness"]["state"], "fresh");
    assert_eq!(state["project"]["id"], "build-123");
}

/// Parity with tests/legacy/test_rollout.py::test_source_time_not_mtime_controls_freshness.
#[test]
fn source_time_not_mtime_controls_freshness() {
    let mut value = manifest();
    value["updated_utc"] = json!(stamp(now() - Duration::seconds(301)));
    let state = ready(&value);
    assert_eq!(state["freshness"]["state"], "stale");
    assert_eq!(state["freshness"]["age_s"], 301);
    // A freshly written file (new mtime) cannot refresh an old source time.
    let loader = Loader::new();
    loader.write(&value);
    assert_eq!(
        loader.collect_at(loader.path())["freshness"]["state"],
        "stale"
    );
}

/// Parity with tests/legacy/test_rollout.py::test_lane_identity_and_source_age_reach_react_without_claiming_acceptance.
/// Intentional Rust difference: a manifest `model` is a claim, so the lane
/// model stays `unknown` until a screen footer is observed.
#[test]
fn lane_identity_and_source_age_reach_react_without_claiming_acceptance() {
    let mut value = manifest();
    value["lanes"] = json!([{"id":"build-lane","screen":"custom_worker","model":"local-model",
        "packet_id":"P01A","source_updated_utc":"original timestamp","status":"stale; screen present",
        "screen_state":"present","untrusted_extra":"discard"}]);
    let lane = ready(&value)["lanes"][0].clone();
    assert_eq!(lane["screen"], "custom_worker");
    assert_eq!(lane["packet_id"], "P01A");
    assert_eq!(lane["source_updated_utc"], "original timestamp");
    assert_eq!(lane["reported_status"], "stale; screen present");
    assert!(lane.get("untrusted_extra").is_none());
    assert!(lane.get("verification").is_none());
    assert_eq!(lane["model"], "unknown");
    assert_eq!(lane["status"], "unknown");
    assert_eq!(lane["progress_observed"], false);
}

/// Parity with tests/legacy/test_rollout.py::test_future_source_is_flagged.
#[test]
fn future_source_is_flagged() {
    let mut value = manifest();
    value["updated_utc"] = json!(stamp(now() + Duration::minutes(10)));
    assert_eq!(ready(&value)["freshness"]["state"], "future");
}

/// Parity with tests/legacy/test_rollout.py::test_bad_json_and_deep_json_fail_closed.
#[test]
fn bad_json_and_deep_json_fail_closed() {
    let loader = Loader::new();
    let deep = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
    for raw in [b"{broken".to_vec(), b"\xff".to_vec(), deep.into_bytes()] {
        assert_eq!(loader.state(&raw), "invalid");
    }
}

/// Parity with tests/legacy/test_rollout.py::test_schema_and_type_errors_are_bounded.
/// Fixed in the observer: non-list `lanes`, non-integer `stale_after_s` and a
/// non-object manifest combined with a configured lanes file (previously a
/// panic) are now invalid instead of silently defaulted.
#[test]
fn schema_and_type_errors_are_bounded() {
    let loader = Loader::new();
    let mut variants = Vec::new();
    for (field, bad) in [
        ("schema_version", json!(2)),
        ("schema_version", json!(true)),
        ("packets", json!({})),
        ("updated_utc", json!("2026-10-18T12:00:00")),
        ("lanes", json!("bad")),
        ("lanes", Value::Null),
        ("stale_after_s", json!(-1)),
        ("stale_after_s", json!(true)),
        ("stale_after_s", json!(300.5)),
    ] {
        let mut item = manifest();
        item[field] = bad;
        variants.push(item);
    }
    variants.extend([json!([]), json!({"schema_version":1}), Value::Null]);
    for value in &variants {
        assert!(load(value).is_err(), "{value}");
        let bytes = serde_json::to_vec(value).expect("fixture JSON");
        assert_eq!(loader.state(&bytes), "invalid", "{value}");
    }
    let lanes = loader.dir.path().join("lanes.json");
    fs::write(&lanes, json!({"lanes":[]}).to_string()).expect("lanes fixture");
    let mut config = loader.config(Some(loader.path()));
    config.rollout_lanes_file = Some(lanes);
    for value in [json!([]), json!("text"), json!(1)] {
        loader.write(&value);
        let state = collect(&config, &json!([])).expect("configured manifest");
        assert_eq!(state["state"], "invalid", "{value}");
    }
}

/// Parity with tests/legacy/test_rollout.py::test_duplicate_packets_and_unknown_status_are_invalid.
#[test]
fn duplicate_packets_and_unknown_status_are_invalid() {
    let mut value = manifest();
    let packet = value["packets"][0].clone();
    value["packets"] = json!([packet.clone(), packet]);
    assert!(load(&value).is_err());
    let mut value = manifest();
    value["packets"][0]["status"] = json!("done");
    assert!(load(&value).is_err());
}

/// Parity with tests/legacy/test_rollout.py::test_oversized_or_special_input_cannot_hang_collector.
/// The Rust reader additionally refuses to follow a symlinked manifest.
#[test]
fn oversized_or_special_input_cannot_hang_collector() {
    let loader = Loader::new();
    assert_eq!(loader.state(&vec![b' '; MAX_BYTES as usize + 1]), "invalid");
    fs::remove_file(loader.path()).expect("remove oversized fixture");
    fs::create_dir(loader.path()).expect("directory fixture");
    let directory = loader.collect_at(loader.path())["state"].clone();
    assert!(directory == "invalid" || directory == "unavailable");
    fs::remove_dir(loader.path()).expect("remove directory fixture");
    let target = loader.dir.path().join("target.json");
    fs::write(&target, manifest().to_string()).expect("symlink target");
    std::os::unix::fs::symlink(&target, loader.path()).expect("symlink fixture");
    assert_eq!(loader.collect_at(loader.path())["state"], "invalid");
    fs::remove_file(loader.path()).expect("remove symlink fixture");
    if Command::new("mkfifo")
        .arg(loader.path())
        .status()
        .is_ok_and(|status| status.success())
    {
        let started = std::time::Instant::now();
        assert_eq!(loader.collect_at(loader.path())["state"], "invalid");
        assert!(started.elapsed().as_secs() < 5);
    }
}

/// Parity with tests/legacy/test_rollout.py::test_embedded_null_path_reports_invalid.
#[test]
fn embedded_null_path_reports_invalid() {
    let loader = Loader::new();
    let state = loader.collect_at(PathBuf::from("bad\0path"));
    assert_eq!(state["state"], "invalid");
    assert_eq!(state["read_only"], true);
}

/// Parity with tests/legacy/test_rollout.py::test_references_remain_text_never_read_or_executed.
#[test]
fn references_remain_text_never_read_or_executed() {
    let loader = Loader::new();
    let secret = loader.dir.path().join("never-open-this.txt");
    fs::write(&secret, "PRIVATE CONTENT").expect("secret fixture");
    let mut value = manifest();
    value["packets"][0]["capsule_path"] = json!(secret);
    value["packets"][0]["evidence"] =
        json!([{"kind":"candidate_receipt","path":"$(touch unexpected)","revision":"abc"}]);
    value["packets"][0]["command"] = json!("touch unexpected");
    loader.write(&value);
    let state = loader.collect_at(loader.path());
    assert!(!state.to_string().contains("PRIVATE CONTENT"));
    assert_eq!(state["packets"][0]["capsule_path"], json!(secret));
    assert_eq!(
        state["packets"][0]["evidence"][0]["path"],
        "$(touch unexpected)"
    );
    assert!(state["packets"][0].get("command").is_none());
    assert!(!loader.dir.path().join("unexpected").exists());
    assert_eq!(
        fs::read_to_string(&secret).expect("secret fixture"),
        "PRIVATE CONTENT"
    );
}

/// Parity with tests/legacy/test_rollout.py::test_loader_does_not_mutate_manifest.
#[test]
fn loader_does_not_mutate_manifest() {
    let loader = Loader::new();
    loader.write(&manifest());
    let before = fs::read(loader.path()).expect("manifest bytes");
    loader.collect_at(loader.path());
    loader.collect_at(loader.path());
    assert_eq!(fs::read(loader.path()).expect("manifest bytes"), before);
}

/// Parity with tests/legacy/test_rollout.py::test_self_report_cannot_establish_verification_or_integration.
#[test]
fn self_report_cannot_establish_verification_or_integration() {
    for status in ["verified", "integrated"] {
        let mut value = manifest();
        value["packets"][0]["status"] = json!(status);
        let packet = ready(&value)["packets"][0].clone();
        assert_eq!(packet["status"], "claimed");
        assert!(
            packet["warnings"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty())
        );
        assert_eq!(packet["reported_status"], status);
    }
}

/// Parity with tests/legacy/test_rollout.py::test_matching_provenance_distinguishes_observed_stages.
#[test]
fn matching_provenance_distinguishes_observed_stages() {
    let mut value = manifest();
    value["packets"][0]["status"] = json!("integrated");
    value["packets"][0]["verification"] = verification("abc123");
    value["packets"][0]["integration"] = integration("abc123");
    let state = ready(&value);
    assert_eq!(state["counts"]["integrated"], 1);
    assert!(
        state["assurance"]
            .as_str()
            .is_some_and(|text| text.contains("not independently validated"))
    );
    value["packets"][0]
        .as_object_mut()
        .expect("packet object")
        .remove("integration");
    assert_eq!(ready(&value)["packets"][0]["status"], "verified");
}

/// Parity with tests/legacy/test_rollout.py::test_revision_mismatch_and_self_verifier_cannot_establish_acceptance.
#[test]
fn revision_mismatch_and_self_verifier_cannot_establish_acceptance() {
    let mut value = manifest();
    value["packets"][0]["status"] = json!("integrated");
    value["packets"][0]["candidate_revision"] = json!("different");
    value["packets"][0]["verification"] = verification("abc123");
    value["packets"][0]["integration"] = integration("abc123");
    assert_eq!(ready(&value)["packets"][0]["status"], "claimed");
    value["packets"][0]["candidate_revision"] = json!("abc123");
    value["packets"][0]["integration"] = integration("other");
    assert_eq!(ready(&value)["packets"][0]["status"], "verified");
    value["packets"][0]["integration"] = integration("abc123");
    value["packets"][0]["claim"] = json!({"owner":"reviewer"});
    assert_eq!(ready(&value)["packets"][0]["status"], "claimed");
}

/// Parity with tests/legacy/test_rollout.py::test_active_alias_and_arbitrary_nodes_are_preserved.
#[test]
fn active_alias_and_arbitrary_nodes_are_preserved() {
    let mut value = manifest();
    value["packets"][0]["status"] = json!("active");
    value["lanes"] =
        json!([{"id":"worker-1","owner":"agent","node":"my-laptop","branch":"feature/a"}]);
    let state = ready(&value);
    assert_eq!(state["packets"][0]["status"], "in_progress");
    assert_eq!(state["lanes"][0]["node"], "my-laptop");
    assert_eq!(state["lanes"][0]["branch"], "feature/a");
}

/// Parity with tests/legacy/test_rollout.py::test_untrusted_links_are_discarded.
/// Fixed in the observer: `pr.url` and `project.repository_url` were copied
/// verbatim, publishing script/data links and URL-embedded credentials.
#[test]
fn untrusted_links_are_discarded() {
    for url in [
        "javascript:alert(1)",
        "data:text/html,bad",
        "https://user:secret@example.com",
        "//example.com",
        "https://",
        "https://exa mple.com",
    ] {
        let mut value = manifest();
        value["pr"]["url"] = json!(url);
        value["project"]["repository_url"] = json!(url);
        let state = ready(&value);
        assert_eq!(state["pr"]["url"], "", "{url}");
        assert_eq!(state["project"]["repository_url"], "", "{url}");
        assert_eq!(state["pr"]["head"], "abc123");
    }
    for url in [
        "https://example.com/repo/pull/1",
        "HTTP://Example.com:8080/x?y#z",
    ] {
        let mut value = manifest();
        value["pr"]["url"] = json!(url);
        assert_eq!(ready(&value)["pr"]["url"], url);
    }
}

/// Parity with tests/legacy/test_rollout.py::test_bounded_worktree_activity_survives_without_changing_packet_acceptance.
/// Intentional Rust difference: lanes are rebuilt from an allowlist and omit
/// manifest `activity` entirely (more conservative than the narrowed Python
/// copy), so no forged acceptance, command or file name survives.
#[test]
fn bounded_worktree_activity_survives_without_changing_packet_acceptance() {
    let mut value = manifest();
    value["lanes"] = json!([{"id":"worker","status":"stale; reported integrated; owned scope files 2","activity":{
        "source":"owned_worktree_metadata/v1","state":"observed","observed_utc":stamp(now()),
        "head_sha":"a".repeat(40),"reported_head_matches":true,"owned_scope_files":2,
        "scanned_entries":3,"scope_paths":1,"skipped_entries":0,"bounded":false,
        "latest_owned_path_mtime_utc":stamp(now()),"accepted":true,"command":"never execute",
        "filenames":["private.rs"],"assurance":"forged acceptance"}}]);
    let state = ready(&value);
    let lane = &state["lanes"][0];
    assert!(lane.get("activity").is_none());
    assert!(lane.get("accepted").is_none());
    let text = lane.to_string();
    for forged in ["never execute", "private.rs", "forged acceptance"] {
        assert!(!text.contains(forged), "{forged}");
    }
    assert_eq!(state["packets"][0]["status"], "ready");
    assert!(
        lane["reported_status"]
            .as_str()
            .is_some_and(|status| status.contains("stale"))
    );
    assert_eq!(lane["status"], "unknown");
}

/// Parity with tests/legacy/test_rollout.py::test_malformed_worktree_activity_is_dropped_or_narrowed.
#[test]
fn malformed_worktree_activity_is_dropped_or_narrowed() {
    let base = json!({"source":"owned_worktree_metadata/v1","state":"bounded","observed_utc":stamp(now()),
        "owned_scope_files":2001,"scanned_entries":true,"scope_paths":-1,
        "head_sha":"not-a-head","reported_head_matches":true,"bounded":"false"});
    for (key, bad) in [
        ("source", json!("unknown")),
        ("state", json!("accepted")),
        ("observed_utc", json!("bad")),
        ("state", json!("bounded")),
    ] {
        let mut activity = base.clone();
        activity[key] = bad;
        let mut value = manifest();
        value["lanes"] = json!([{"id":"worker","activity":activity}]);
        let lane = ready(&value)["lanes"][0].clone();
        assert!(lane.get("activity").is_none());
        assert!(!lane.to_string().contains("not-a-head"));
    }
}

/// Parity with tests/legacy/test_rollout.py::test_actual_observer_statuses_survive_loader_and_remain_conservative
/// (loader half; rendering is covered by the dashboard's own tests). Rust keeps
/// a lane self-report only as bounded `reported_status` text, never as stage.
#[test]
fn actual_observer_statuses_survive_loader_and_remain_conservative() {
    let statuses = [
        "candidate_committed_unverified",
        "implementing_on_frozen_seed",
        "dispatched_switching_private_worktree",
        "candidate_committed_magic",
    ];
    let mut value = manifest();
    value["lanes"] = json!(
        statuses
            .iter()
            .enumerate()
            .map(
                |(index, status)| json!({"id":format!("lane-{index}"),"status":status,"screen":null,
                "agent_session":format!("/root/lane-{index}")})
            )
            .collect::<Vec<_>>()
    );
    let state = ready(&value);
    assert_eq!(state["packets"][0]["status"], "ready");
    assert_eq!(state["counts"]["ready"], 1);
    for (lane, status) in state["lanes"]
        .as_array()
        .expect("lanes")
        .iter()
        .zip(statuses)
    {
        assert_eq!(lane["status"], "unknown");
        assert_eq!(lane["progress_observed"], false);
        assert_eq!(lane["identity_state"], "not_observed");
        assert_eq!(lane["reported_status"], &status[..status.len().min(40)]);
    }
}

fn screen_on(pid: u64, name: &str, host: &str) -> Value {
    json!({"host":host,"id":format!("{pid}.{name}"),"name":name,"pid":pid,"alive":true,
        "agent_running":true,"agent":"codex","model":"gpt-6-luna","effort":"medium",
        "model_observed_utc":stamp(now() - Duration::seconds(5)),"tail":["safe observed output"],
        "state":"unknown","project":"unassigned"})
}
fn screen(pid: u64, name: &str) -> Value {
    screen_on(pid, name, "node")
}
fn live(lanes: Value, project: Value) -> Value {
    json!({"schema_version":1,"project":project,"updated_utc":stamp(now()),
        "packets":[{"id":"P01","status":"claimed"}],"lanes":lanes})
}
fn custom() -> Value {
    json!({"id":"custom-build","name":"Custom build"})
}
fn worker_manifest() -> Value {
    live(
        json!([{"id":"worker-one","screen":"custom_worker","packet_id":"P01"}]),
        custom(),
    )
}
fn observe(manifest: &Value, screens: Value) -> Value {
    normalize(manifest.clone(), json!({}), &screens, "node", now()).expect("valid manifest")
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_activity_is_checked_after_capture_not_against_manifest_load_time
/// (adapted): the Rust observer publishes no screen-change activity times;
/// lanes copy only allowlisted identity fields, so no activity time or state
/// from any source can reach a lane.
#[test]
fn activity_is_checked_after_capture_not_against_manifest_load_time() {
    let mut observed = screen(101, "custom_worker");
    observed["screen_changed_utc"] = json!(stamp(now() + Duration::seconds(2)));
    observed["state"] = json!("active");
    let lane = observe(&worker_manifest(), json!([observed]))["lanes"][0].clone();
    assert_eq!(lane["identity_state"], "observed");
    assert_eq!(
        lane["model_observed_utc"],
        stamp(now() - Duration::seconds(5))
    );
    for key in ["screen_changed_utc", "state", "tail", "observed_utc"] {
        assert!(lane.get(key).is_none(), "{key}");
    }
    assert_eq!(lane["status"], "unknown");
    assert_eq!(lane["progress_observed"], false);
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_normalization_preserves_pid_pins_and_rejects_bad_pid_values.
/// Intentional Rust difference: a malformed pin makes that lane
/// `identity_mismatch` (nothing adopted) instead of invalidating the manifest.
#[test]
fn normalization_preserves_pid_pins_and_rejects_bad_pid_values() {
    let inventory = json!([screen(101, "custom_worker")]);
    for bad in [
        json!(true),
        json!(0),
        json!(-1),
        json!(1_u64 << 31),
        json!("101"),
    ] {
        for key in ["screen_pid", "reported_screen_pid"] {
            let mut lane = json!({"id":"worker","screen":"custom_worker"});
            lane[key] = bad.clone();
            let state = observe(&live(json!([lane]), custom()), inventory.clone());
            let lane = &state["lanes"][0];
            assert_eq!(lane["identity_state"], "identity_mismatch", "{key}={bad}");
            assert_eq!(lane["model"], "unknown");
            assert!(lane["agent_running"].is_null());
        }
    }
    let pinned = live(
        json!([{"id":"worker","screen":"101.custom_worker","screen_pid":101,"reported_screen_pid":101}]),
        custom(),
    );
    let lane = observe(&pinned, inventory)["lanes"][0].clone();
    assert_eq!(lane["identity_state"], "observed");
    assert_eq!(lane["screen_pid"], 101);
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_actual_observation_assigns_project_without_mutating_counts_claims_or_existing_projects.
/// Producer-project immutability is covered by
/// `lib::legacy_parity_tests::cycle_loads_manifest_and_joins_before_extras_without_adding_projects`.
#[test]
fn actual_observation_assigns_project_without_mutating_counts_claims_or_existing_projects() {
    let manifest = worker_manifest();
    let before = manifest.clone();
    let unobserved = observe(&manifest, json!([]));
    let state = observe(&manifest, json!([screen(101, "custom_worker")]));
    assert_eq!(manifest, before);
    let lane = &state["lanes"][0];
    assert_eq!(lane["identity_state"], "observed");
    assert_eq!(lane["screen_id"], "101.custom_worker");
    assert_eq!(lane["agent_running"], true);
    assert_eq!(lane["packet_id"], "P01");
    assert_eq!(lane["effort"], "medium");
    assert!(lane.get("accepted").is_none());
    assert_eq!(state["counts"], unobserved["counts"]);
    assert_eq!(state["packets"], unobserved["packets"]);
    assert_eq!(state["project"], unobserved["project"]);
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_unknown_absent_and_identity_mismatch_never_capture_or_fake_an_agent.
/// The Rust join is pure (no capture callback exists); unknown and absent
/// inventory both stay `not_observed` with unknown agent state.
#[test]
fn unknown_absent_and_identity_mismatch_never_capture_or_fake_an_agent() {
    for inventory in [Value::Null, json!([])] {
        let lane = observe(&worker_manifest(), inventory)["lanes"][0].clone();
        assert_eq!(lane["identity_state"], "not_observed");
        assert!(lane["agent_running"].is_null());
        assert_eq!(lane["model"], "unknown");
    }
    let inventory = json!([screen_on(101, "custom_worker", "this-node")]);
    for lane in [
        json!({"id":"worker","screen":"102.custom_worker"}),
        json!({"id":"worker","screen":"custom_worker","screen_pid":102}),
        json!({"id":"worker","screen":"custom_worker","screen_pid":101,"reported_screen_pid":102}),
        json!({"id":"worker","screen":"custom_worker","node":"different-node"}),
        json!({"id":"worker","screen":"custom_worker; touch /tmp/no"}),
    ] {
        let state = normalize(
            live(json!([lane]), custom()),
            json!({}),
            &inventory,
            "this-node",
            now(),
        )
        .expect("valid manifest");
        let observed = &state["lanes"][0];
        assert_ne!(observed["identity_state"], "observed", "{lane}");
        assert!(observed["agent_running"].is_null());
        assert_eq!(observed["model"], "unknown");
        assert!(observed.get("screen_id").is_none());
    }
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_aliases_duplicate_inventory_names_and_producer_collisions_are_not_adopted.
/// Fixed in the observer: two lanes resolving to one screen were both adopted,
/// and a screen the fleet attributed to another configured project could be
/// adopted. Intentional Rust difference: a pin does not disambiguate duplicate
/// screen names (Python adopted the pinned screen); Rust adopts nothing.
/// The project-id collision half is covered in `lib::legacy_parity_tests`.
#[test]
fn aliases_duplicate_inventory_names_and_producer_collisions_are_not_adopted() {
    let aliases = live(
        json!([{"id":"a","screen":"custom_worker"},{"id":"b","screen":"101.custom_worker"}]),
        custom(),
    );
    let state = observe(&aliases, json!([screen(101, "custom_worker")]));
    for lane in state["lanes"].as_array().expect("lanes") {
        assert_eq!(lane["identity_state"], "aliased");
        assert_eq!(lane["model"], "unknown");
        assert!(lane["agent_running"].is_null());
        assert!(lane.get("screen_id").is_none());
    }
    let ambiguous = observe(
        &worker_manifest(),
        json!([screen(101, "custom_worker"), screen(102, "custom_worker")]),
    );
    assert_eq!(ambiguous["lanes"][0]["identity_state"], "not_observed");
    assert_eq!(ambiguous["lanes"][0]["model"], "unknown");
    let mut owned = screen(101, "custom_worker");
    owned["project"] = json!("old");
    let collision = observe(&worker_manifest(), json!([owned]));
    assert_eq!(
        collision["lanes"][0]["identity_state"],
        "producer_collision"
    );
    assert_eq!(collision["lanes"][0]["model"], "unknown");
    let mut own = screen(101, "custom_worker");
    own["project"] = json!("custom-build");
    assert_eq!(
        observe(&worker_manifest(), json!([own]))["lanes"][0]["identity_state"],
        "observed"
    );
    let pinned = live(
        json!([{"id":"pinned","screen":"custom_worker","screen_pid":102}]),
        custom(),
    );
    let state = observe(
        &pinned,
        json!([screen(101, "custom_worker"), screen(102, "custom_worker")]),
    );
    assert_eq!(state["lanes"][0]["identity_state"], "not_observed");
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_captain_is_configurable_and_no_process_or_failed_observation_stays_honest.
/// The failed-capture half does not apply: the pure Rust join performs no
/// capture; fleet capture failures leave footer/tail unknown upstream.
#[test]
fn captain_is_configurable_and_no_process_or_failed_observation_stays_honest() {
    let project =
        json!({"id":"custom-build","captain_screen":"201.custom_captain","captain_screen_pid":201});
    let manifest = live(
        json!([{"id":"worker-one","screen":"custom_worker","packet_id":"P01"}]),
        project,
    );
    let state = observe(
        &manifest,
        json!([screen(101, "custom_worker"), screen(201, "custom_captain")]),
    );
    let captain = &state["live_observation"]["captain"];
    assert_eq!(captain["id"], "201.custom_captain");
    assert_eq!(captain["identity_state"], "observed");
    assert_eq!(state["lanes"][0]["identity_state"], "observed");
    let mut stopped = screen(101, "custom_worker");
    stopped["agent_running"] = json!(false);
    stopped["agent"] = Value::Null;
    assert_eq!(
        observe(&manifest, json!([stopped]))["lanes"][0]["agent_running"],
        false
    );
    let mut unknown = screen(101, "custom_worker");
    unknown["agent_running"] = Value::Null;
    assert!(observe(&manifest, json!([unknown]))["lanes"][0]["agent_running"].is_null());
    let mut foreign = screen(201, "custom_captain");
    foreign["project"] = json!("old");
    assert!(observe(&manifest, json!([foreign]))["live_observation"]["captain"].is_null());
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_tail_is_cleaned_bounded_and_never_copied_without_a_sanitizer.
/// Lanes never copy screen tails; captain tails are the fleet record's
/// already-sanitized tail (see `fleet::legacy_parity`).
#[test]
fn tail_is_cleaned_bounded_and_never_copied_without_a_sanitizer() {
    let mut noisy = screen(101, "custom_worker");
    noisy["tail"] = json!(vec![format!("secret-value {}", "x".repeat(500)); 20]);
    let state = observe(&worker_manifest(), json!([noisy]));
    assert_eq!(state["lanes"][0]["identity_state"], "observed");
    assert!(state["lanes"][0].get("tail").is_none());
    assert!(!state["lanes"].to_string().contains("secret-value"));
}
