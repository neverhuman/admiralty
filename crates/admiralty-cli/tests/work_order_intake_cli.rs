#![cfg(unix)]

use admiralty_core::{
    CommandId, TaskContract, content_digest,
    intake::{
        DeliveryRequirement, DeliveryStage, DeliveryTarget, DeliveryTargetKind, IntakeBatch,
        IntakeFormat, IntakeItem, OutcomeInput, RecordIntakeRequest, RequesterAttribution,
        SelectedTask, SourcePayload, SourceProvenance, SourceSpan, TargetRequirement,
        WorkOrderRevision, WorkOrderTaskBinding,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

const TOKEN: &str = "cli-intake-private-012345678901234567890123456789";
const ACTIONS: [&str; 6] = [
    "preview_raw",
    "capture_raw",
    "record_enriched",
    "read_captured",
    "read_work_order",
    "query_duplicates",
];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("wo-cli-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let f = Self(path);
        f.write("token", TOKEN.as_bytes());
        f
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    fn config(&self, grant_project: &str, credential_token: &str) -> PathBuf {
        let token_file = if credential_token == TOKEN {
            self.0.join("token")
        } else {
            self.write("revoked-token", credential_token.as_bytes())
        };
        let config = json!({
            "schema_version":1,
            "origins":["http://127.0.0.1:18890"],
            "policies":[{"id":"bounded","revision":"intake-v1","max_source_bytes":"1048576","max_requests":"256","max_outcomes":"1000","max_json_item_text_bytes":"65536"}],
            "credentials":[{"credential_id":"intake-user","token_file":token_file,"actor":"submitter","grants":[{
                "project_id":grant_project,"root_id":"root","authority_ref":"grant:intake","authority_generation":"9007199254740993","policy_id":"bounded","actions":ACTIONS
            }]}],
            "action_rules":[{"credential_id":"intake-user","actor":"submitter","project_id":grant_project,"root_id":"root","authority_ref":"grant:intake","authority_generation":"9007199254740993","policy_id":"bounded","actions":ACTIONS}]
        });
        let path = self.path("intake-config.json");
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        self.write("intake-config.json", &serde_json::to_vec(&config).unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn invoke(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_admiralty"))
        .args(args)
        .output()
        .unwrap()
}

fn intake_args<'a>(
    f: &'a Fixture,
    operation: &str,
    database: Option<&Path>,
    request: &Path,
) -> Vec<String> {
    let mut args = vec!["intake".into(), operation.into()];
    if let Some(database) = database {
        args.extend(["--database".into(), database.display().to_string()]);
    }
    args.extend(["--request".into(), request.display().to_string()]);
    args.extend([
        "--intake-config".into(),
        f.path("intake-config.json").display().to_string(),
    ]);
    args.extend(["--token-file".into(), f.path("token").display().to_string()]);
    args
}

fn checked(args: &[String]) -> Value {
    let output = invoke(&args.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn record_source() -> (String, SourceProvenance) {
    let task: TaskContract = serde_json::from_value(json!({
        "schema_version":1,"project_id":"allowed","root_id":"root","task_id":"task-cli","revision":1,
        "title":"Review source","objective":"Record one independently testable outcome","non_goals":["No launch"],
        "owned_paths":["src/lib.rs"],"forbidden_paths":["private"],"read_first":[],"dependencies":[],
        "criteria":[{"id":"c1","text":"Unit check passes","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":60}],
        "authority_ref":"grant:intake","max_attempts":1,"simulated":false
    })).unwrap();
    let delivery = DeliveryRequirement {
        schema_version: 1,
        task_id: task.task_id.clone(),
        contract_revision: 1,
        contract_digest: content_digest(&task).unwrap(),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    };
    let outcome_text = "Explicit reviewed outcome";
    let mut batch = IntakeBatch {
        schema_version: 1,
        items: vec![IntakeItem {
            text: outcome_text.into(),
            title: Some("Work order".into()),
            rationale: Some("Reviewed before recording".into()),
            requested_timing: None,
            constraints: vec![],
            claimed_project_id: Some(task.project_id.clone()),
            outcomes: vec![OutcomeInput {
                id: "outcome-cli".into(),
                description: "Review source".into(),
                source_spans: vec![],
                constraints: vec![],
                task: Some(task.clone()),
                delivery: Some(delivery.clone()),
            }],
        }],
    };
    let first_source = serde_json::to_string(&batch).unwrap();
    let start = first_source.find(outcome_text).unwrap() as u32;
    batch.items[0].outcomes[0].source_spans = vec![SourceSpan {
        start,
        end: start + outcome_text.len() as u32,
    }];
    let source = serde_json::to_string(&batch).unwrap();
    let provenance = SourceProvenance {
        schema_version: 1,
        source_id: "record-source-cli".into(),
        revision: "r1".into(),
        captured_at: "2026-10-19T07:00:00Z".into(),
        requester: RequesterAttribution::Unknown {},
        origin: None,
    };
    (source, provenance)
}

fn enriched_record(
    preview: &Value,
    command_id: &str,
    source: String,
    provenance: SourceProvenance,
) -> RecordIntakeRequest {
    let task: TaskContract =
        serde_json::from_value(preview["requests"][0]["outcomes"][0]["outcome"]["task"].clone())
            .unwrap();
    let delivery: DeliveryRequirement = serde_json::from_value(
        preview["requests"][0]["outcomes"][0]["outcome"]["delivery"].clone(),
    )
    .unwrap();
    let item = &preview["requests"][0];
    let outcome = &item["outcomes"][0];
    RecordIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new(command_id).unwrap(),
        expected_work_order_revision: 0,
        plan_id: "plan-cli".into(),
        expected_plan_revision: 0,
        preview_manifest_digest: preview["manifest_digest"].as_str().unwrap().into(),
        policy_revision: preview["policy_revision"].as_str().unwrap().into(),
        policy_digest: preview["policy_digest"].as_str().unwrap().into(),
        authority_generation: 9_007_199_254_740_993,
        work_order: WorkOrderRevision {
            schema_version: 1,
            id: "order-cli".into(),
            revision: 1,
            scope: serde_json::from_value(preview["authorized_scope"].clone()).unwrap(),
            provenance,
            title: Some("Work order".into()),
            outcome: Some("Review source".into()),
            rationale: Some("Reviewed before recording".into()),
            constraints: vec![],
            requested_timing: None,
            raw_digest: preview["raw_digest"].as_str().unwrap().into(),
            canonical_digest: preview["canonical_digest"].as_str().unwrap().into(),
            parser_revision: preview["parser_revision"].as_str().unwrap().into(),
            format: IntakeFormat::JsonBatch,
            source_payload: SourcePayload::InlineUtf8 { text: source },
        },
        selected_tasks: vec![SelectedTask {
            binding: WorkOrderTaskBinding {
                work_order_id: "order-cli".into(),
                work_order_revision: 1,
                request_index: 0,
                outcome_id: outcome["outcome"]["id"].as_str().unwrap().into(),
                outcome_digest: outcome["outcome_digest"].as_str().unwrap().into(),
                source_spans: serde_json::from_value(outcome["outcome"]["source_spans"].clone())
                    .unwrap(),
                task_id: task.task_id.clone(),
                contract_revision: 1,
                contract_digest: content_digest(&task).unwrap(),
            },
            task,
            delivery,
        }],
    }
}

#[test]
fn preview_capture_replay_and_scoped_show_preserve_unicode_without_dispatch() {
    let f = Fixture::new();
    f.config("allowed", TOKEN);
    let source = "One request\n\n第二段 remains part of the same request.";
    let preview_input = json!({
        "project_id":"allowed","root_id":"root","format":"plain_text",
        "provenance":{"schema_version":1,"source_id":"cli-request","revision":"r1","captured_at":"2026-10-19T07:00:00Z","requester":{"kind":"unknown"},"origin":null},
        "source_payload":{"kind":"inline_utf8","text":source}
    });
    let preview_file = f.write("preview.json", &serde_json::to_vec(&preview_input).unwrap());
    let preview = checked(&intake_args(&f, "preview", None, &preview_file));
    let preview = &preview;
    assert_eq!(preview["requests"].as_array().unwrap().len(), 1);
    assert_eq!(preview["requests"][0]["text"], source);

    let request = json!({
        "schema_version":1,"command_id":"capture-cli-once","capture_id":"cli-original",
        "revision":1,"expected_revision":0,"parent_receipt_digest":null,
        "scope":preview["authorized_scope"].clone(),"format":"plain_text",
        "provenance":preview_input["provenance"].clone(),"source_payload":preview_input["source_payload"].clone(),
        "preview_manifest_digest":preview["manifest_digest"].clone(),
        "policy_revision":preview["policy_revision"].clone(),"policy_digest":preview["policy_digest"].clone()
    });
    let capture_file = f.write("capture.json", &serde_json::to_vec(&request).unwrap());
    let database = f.path("offline.sqlite");
    let first = checked(&intake_args(&f, "capture", Some(&database), &capture_file));
    assert_eq!(first["replayed"], false);
    assert!(
        !String::from_utf8_lossy(
            &invoke(
                &intake_args(&f, "capture", Some(&database), &capture_file)
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            )
            .stdout
        )
        .contains("第二段")
    );
    let retry = checked(&intake_args(&f, "capture", Some(&database), &capture_file));
    assert_eq!(retry["replayed"], true);

    let read_file = f.write(
        "show-capture.json",
        &serde_json::to_vec(&json!({
            "scope":preview["authorized_scope"],"capture_id":"cli-original","revision":null
        }))
        .unwrap(),
    );
    let shown = checked(&intake_args(
        &f,
        "show-capture",
        Some(&database),
        &read_file,
    ));
    assert_eq!(shown["request"]["source_payload"]["text"], source);
    assert_eq!(shown["request"]["scope"], preview["authorized_scope"]);
    assert_eq!(shown["preview"]["requests"].as_array().unwrap().len(), 1);
}

#[test]
fn record_replay_exact_revision_and_duplicate_index_are_scoped() {
    let f = Fixture::new();
    f.config("allowed", TOKEN);
    let (source, provenance) = record_source();
    let preview_input = json!({
        "project_id":"allowed","root_id":"root","format":"json_batch",
        "provenance":serde_json::to_value(&provenance).unwrap(),
        "source_payload":{"kind":"inline_utf8","text":source}
    });
    let preview_request = f.write(
        "record-preview.json",
        &serde_json::to_vec(&preview_input).unwrap(),
    );
    let preview = checked(&intake_args(&f, "preview", None, &preview_request));
    assert!(
        preview["requests"][0]["outcomes"][0]["unresolved"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let record = enriched_record(&preview, "record-cli-once", source, provenance);
    let record_file = f.write("record.json", &serde_json::to_vec(&record).unwrap());
    let database = f.path("record.sqlite");
    let first = checked(&intake_args(&f, "record", Some(&database), &record_file));
    assert_eq!(first["replayed"], false);
    assert_eq!(first["receipt"]["work_order_revision"], 1);
    let replay = checked(&intake_args(&f, "record", Some(&database), &record_file));
    assert_eq!(replay["replayed"], true);

    let show_request = f.write(
        "show-work-order.json",
        &serde_json::to_vec(&json!({
            "scope":record.work_order.scope,"work_order_id":"order-cli","revision":1
        }))
        .unwrap(),
    );
    let shown = checked(&intake_args(&f, "show", Some(&database), &show_request));
    assert_eq!(shown["work_order"]["id"], "order-cli");
    assert_eq!(shown["work_order"]["revision"], 1);
    assert_eq!(shown["receipt"]["plan_revision"], 1);

    let duplicate_request = f.write("record-duplicates.json", &serde_json::to_vec(&json!({
        "project_id":"allowed","root_id":"root",
        "query":{"schema_version":1,"project_id":"allowed","raw_digest":preview["raw_digest"],"canonical_digest":preview["canonical_digest"],"origin":null,"command_id":null,"cursor":null,"limit":10}
    })).unwrap());
    let duplicates = checked(&intake_args(
        &f,
        "duplicates",
        Some(&database),
        &duplicate_request,
    ));
    let candidates = duplicates["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["task_id"], "task-cli");
    assert!(duplicates.get("coverage").is_some());
}

#[test]
fn configured_wrong_scope_revoked_token_and_oversize_input_fail_closed() {
    let f = Fixture::new();
    let input = json!({
        "project_id":"allowed","root_id":"root","format":"plain_text",
        "provenance":{"schema_version":1,"source_id":"request","revision":"r1","captured_at":"2026-10-19T07:00:00Z","requester":{"kind":"unknown"},"origin":null},
        "source_payload":{"kind":"inline_utf8","text":"hello"}
    });
    let request = f.write("preview.json", &serde_json::to_vec(&input).unwrap());

    f.config("another-project", TOKEN);
    let wrong_scope = invoke(
        &intake_args(&f, "preview", None, &request)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(!wrong_scope.status.success());
    assert!(!String::from_utf8_lossy(&wrong_scope.stdout).contains(TOKEN));

    f.config(
        "allowed",
        "cli-intake-private-revoked-012345678901234567890123",
    );
    let revoked = invoke(
        &intake_args(&f, "preview", None, &request)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(!revoked.status.success());

    f.config("allowed", TOKEN);
    let large = f.write("oversize.json", &vec![b' '; 1024 * 1024 + 1]);
    let oversize = invoke(
        &intake_args(&f, "preview", None, &large)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    assert!(!oversize.status.success());
}

#[test]
fn duplicate_command_uses_service_query_and_returns_bounded_coverage() {
    let f = Fixture::new();
    f.config("allowed", TOKEN);
    let input = json!({
        "project_id":"allowed","root_id":"root","format":"plain_text",
        "provenance":{"schema_version":1,"source_id":"request","revision":"r1","captured_at":"2026-10-19T07:00:00Z","requester":{"kind":"unknown"},"origin":null},
        "source_payload":{"kind":"inline_utf8","text":"text"}
    });
    let request = f.write("preview.json", &serde_json::to_vec(&input).unwrap());
    let preview_args = intake_args(&f, "preview", None, &request);
    let preview = checked(&preview_args);
    let database = f.path("offline.sqlite");
    let mut capture_args = intake_args(&f, "capture", Some(&database), &f.write("capture.json", &serde_json::to_vec(&json!({
        "schema_version":1,"command_id":"capture-for-duplicate","capture_id":"dup-source","revision":1,"expected_revision":0,"parent_receipt_digest":null,
        "scope":preview["authorized_scope"],"format":"plain_text","provenance":input["provenance"],"source_payload":input["source_payload"],
        "preview_manifest_digest":preview["manifest_digest"],"policy_revision":preview["policy_revision"],"policy_digest":preview["policy_digest"]
    })).unwrap()));
    checked(&capture_args);
    capture_args.clear();
    let duplicate_file = f.write("duplicates.json", &serde_json::to_vec(&json!({
        "project_id":"allowed","root_id":"root",
        "query":{"schema_version":1,"project_id":"allowed","raw_digest":preview["raw_digest"],"canonical_digest":preview["canonical_digest"],"origin":null,"command_id":null,"cursor":null,"limit":10}
    })).unwrap());
    let output = checked(&intake_args(
        &f,
        "duplicates",
        Some(&database),
        &duplicate_file,
    ));
    assert_eq!(output["candidates"].as_array().unwrap().len(), 0);
    assert!(output.get("coverage").is_some());
}
