//! Round 8 P12/P13/P14 cockpit contract qualification.

use admiralty_server::{ServerConfig, app};
use admiralty_store::Store;
use admiralty_testkit::task_fixture;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::Value;
use std::{net::SocketAddr, path::PathBuf};
use tower::ServiceExt;

const COCKPIT: &str = include_str!("../../apps/dashboard/src/ui/App.tsx");

#[tokio::test]
async fn p13_p14_round8_tui_cockpit_receipt_qualification() {
    let mut directory_builder = tempfile::Builder::new();
    directory_builder.prefix("admiralty-round8-cockpit-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = directory_builder
        .tempdir()
        .expect("private fixture directory");
    let store = Store::open(directory.path().join("cockpit.sqlite")).expect("fixture store");
    let mut contract = task_fixture("ROUND8-COCKPIT");
    contract.title = "Cockpit qualification task".into();
    contract.project_id = admiralty_core::ProjectId::new("round8-project").expect("project ID");
    contract.simulated = true;
    store
        .append(
            admiralty_core::SourceIdentity {
                producer_id: "round8-fixture".into(),
                incarnation: "deterministic-boot".into(),
                sequence: "1".into(),
            },
            admiralty_core::DomainEvent::TaskSubmitted { contract },
            "2026-10-18T00:00:00Z".into(),
        )
        .expect("append deterministic task");

    let service = app(
        store,
        ServerConfig {
            listen: "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("loopback address"),
            simulated: true,
            token: None,
            assets: Some(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/dashboard/dist"),
            ),
            legacy: None,
            models_file: None,
        },
    )
    .expect("test router");

    let response = service
        .oneshot(
            Request::builder()
                .uri("/v1/fleet")
                .body(Body::empty())
                .expect("fleet request"),
        )
        .await
        .expect("fleet response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let fleet: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("bounded fleet response"),
    )
    .expect("fleet JSON");

    assert_eq!(fleet["schema_version"], 1);
    assert_eq!(fleet["simulated"], true);
    assert_eq!(fleet["native_execution_qualified"], false);
    assert_eq!(fleet["cursor"]["event_seq"], "1");
    assert_eq!(fleet["projects"][0]["id"], "round8-project");
    assert_eq!(fleet["projects"][0]["total"], 1);
    assert_eq!(fleet["projects"][0]["accepted"], 0);
    assert_eq!(fleet["tasks"][0]["contract"]["task_id"], "ROUND8-COCKPIT");
    assert_eq!(fleet["tasks"][0]["state"], "ready");
    assert_eq!(fleet["coordination"]["routine_model_turns"], 0);
    assert_eq!(fleet["coordination"]["measured_tokens"], Value::Null);

    for cockpit_contract in [
        "Fleet summary",
        "Task stage table",
        "Candidate",
        "native_execution_qualified",
        "Unknown is distinct from zero",
    ] {
        assert!(
            COCKPIT.contains(cockpit_contract),
            "cockpit contract: {cockpit_contract}"
        );
    }
}
