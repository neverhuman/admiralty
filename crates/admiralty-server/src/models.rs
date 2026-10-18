//! Local model configuration helpers. Route owners must authenticate both reads
//! and writes and deny demo writes. These helpers never launch a provider.
use admiralty_runtime::models::{
    CapacitySnapshot, ModelRegistry, ModelSpec, Provider, RegistryError, RegistryFile,
    SelectionOutcome, SelectionRequest, UsageCost,
};
use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize)]
pub struct PublicModel {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub provider_model: String,
    pub tier: i32,
    pub enabled: bool,
    pub node_id: Option<String>,
    pub max_concurrency: u16,
    pub capabilities: BTreeSet<String>,
    pub task_max_attempts: u16,
    pub escalate_to: Option<String>,
    pub usage_cost: UsageCost,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelsResponse {
    pub schema_version: u32,
    pub revision: String,
    pub models: Vec<PublicModel>,
    pub native_execution_qualified: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsUpdateRequest {
    pub expected_revision: String,
    pub models: Vec<ModelSpec>,
}

pub type ModelsApiError = (StatusCode, Json<serde_json::Value>);

pub fn public_models(registry: ModelRegistry) -> ModelsResponse {
    ModelsResponse {
        schema_version: 1,
        revision: registry.revision,
        models: registry
            .models
            .into_iter()
            .map(|model| PublicModel {
                id: model.id,
                label: model.label,
                provider: model.provider,
                provider_model: model.provider_model,
                tier: model.tier,
                enabled: model.enabled,
                node_id: model.node_id,
                max_concurrency: model.max_concurrency,
                capabilities: model.capabilities,
                task_max_attempts: model.task_max_attempts,
                escalate_to: model.escalate_to,
                usage_cost: UsageCost::Unknown,
            })
            .collect(),
        native_execution_qualified: false,
    }
}

fn failure(error: RegistryError) -> ModelsApiError {
    match error {
        RegistryError::RevisionConflict { current } => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error":"revision_conflict","current_revision":current})),
        ),
        RegistryError::Invalid(message) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error":"invalid_model_registry","detail":message})),
        ),
        RegistryError::Busy => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"model_registry_busy","retry_after_s":1})),
        ),
        RegistryError::Io(_) | RegistryError::Json(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"model_registry_unavailable"})),
        ),
    }
}

pub async fn list_models(registry: RegistryFile) -> Result<Json<ModelsResponse>, ModelsApiError> {
    let result = tokio::task::spawn_blocking(move || registry.read())
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":"model_registry_worker_unavailable"})),
            )
        })?;
    result.map(public_models).map(Json).map_err(failure)
}

pub async fn update_models(
    registry: RegistryFile,
    request: ModelsUpdateRequest,
) -> Result<Json<ModelsResponse>, ModelsApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let current = registry.read()?;
        let mut models = request.models;
        for model in &mut models {
            if let Some(previous) = current
                .models
                .iter()
                .find(|previous| previous.id == model.id)
            {
                if model.endpoint.is_none() {
                    model.endpoint = previous.endpoint.clone();
                }
                if model.credential_file_ref.is_none() {
                    model.credential_file_ref = previous.credential_file_ref.clone();
                }
            }
        }
        registry.replace(&request.expected_revision, models)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"model_registry_worker_unavailable"})),
        )
    })?;
    result.map(public_models).map(Json).map_err(failure)
}

pub async fn select_model(
    registry: RegistryFile,
    request: SelectionRequest,
    capacity: CapacitySnapshot,
) -> Result<Json<SelectionOutcome>, ModelsApiError> {
    let result = tokio::task::spawn_blocking(move || registry.read()?.select(&request, &capacity))
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":"model_registry_worker_unavailable"})),
            )
        })?;
    result.map(Json).map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_list_excludes_connection_and_credential_references_and_unknown_is_not_zero() {
        let registry: ModelRegistry = serde_json::from_value(serde_json::json!({
            "schema_version":1,"revision":"9007199254740993","models":[{
                "id":"self-hosted","label":"Own model","provider":"openai_compatible","provider_model":"fixture/own-model","tier":0,"enabled":true,
                "node_id":"inference-node","endpoint":"http://127.0.0.1:8000/v1",
                "credential_file_ref":"credentials/local-provider","max_concurrency":16,
                "capabilities":["coding"],"task_max_attempts":2,"escalate_to":null
            }]
        })).expect("private model fixture");
        registry.validate().expect("valid private model fixture");
        let public = serde_json::to_value(public_models(registry)).expect("public model JSON");
        assert_eq!(public["revision"], "9007199254740993");
        assert_eq!(public["native_execution_qualified"], false);
        assert!(public["models"][0].get("endpoint").is_none());
        assert!(public["models"][0].get("credential_file_ref").is_none());
        assert_eq!(
            public["models"][0]["usage_cost"],
            serde_json::json!({"status":"unknown"})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn public_row_updates_preserve_private_connections_and_respect_revision_conflicts() {
        use std::os::unix::fs::DirBuilderExt;
        let directory =
            std::env::temp_dir().join(format!("admiralty-model-test-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .expect("isolated private model fixture");
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let file =
            RegistryFile::new(directory.join("models.json")).expect("private registry fixture");
        let example: ModelRegistry =
            serde_json::from_str(include_str!("../../../configs/examples/models.json"))
                .expect("example policy");
        let first = file.replace("0", example.models).expect("initial policy");
        let endpoint = first.models[0].endpoint.clone();
        let credentials = first.models[0].credential_file_ref.clone();
        let mut editable = first.models.clone();
        editable[0].endpoint = None;
        editable[0].credential_file_ref = None;
        editable[0].max_concurrency = 8;
        let response = update_models(
            file.clone(),
            ModelsUpdateRequest {
                expected_revision: "1".into(),
                models: editable.clone(),
            },
        )
        .await
        .expect("public row update");
        assert_eq!(response.0.revision, "2");
        let persisted = file.read().expect("updated policy");
        assert_eq!(persisted.models[0].endpoint, endpoint);
        assert_eq!(persisted.models[0].credential_file_ref, credentials);
        assert_eq!(persisted.models[0].max_concurrency, 8);
        let error = update_models(
            file.clone(),
            ModelsUpdateRequest {
                expected_revision: "1".into(),
                models: editable,
            },
        )
        .await
        .expect_err("stale update is rejected");
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(error.1.0["current_revision"], "2");
        let blocked = SelectionRequest {
            current_model: Some("ornith".into()),
            last_failure: Some(admiralty_runtime::models::FailureReason::SetupError),
            allow_escalation: true,
            ..Default::default()
        };
        let outcome = select_model(file, blocked, CapacitySnapshot::default())
            .await
            .expect("pure dry-run selection");
        assert!(matches!(
            outcome.0,
            SelectionOutcome::Waiting {
                reason: admiralty_runtime::models::WaitReason::SetupRequired,
                ..
            }
        ));
    }
}
