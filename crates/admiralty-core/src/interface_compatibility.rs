//! Consumer-task interface revision compatibility policy, checks, and revocation closure
//! (`P02` / `MASTER_PLAN` §5.4).
//!
//! Binds producer `TaskRevisionRef`, consumer `TaskRevisionRef`s, artifact/schema digests,
//! compatibility policy, required acceptance check IDs, and binding status (`provisional`,
//! `verified`, `revoked`, `superseded`). Permits provisional consumer development when
//! policy allows, requires exact verified producer revision + checks for final acceptance,
//! and invalidates affected consumer readiness/receipts on producer revocation without
//! erasing prior consumer work references.

use crate::{
    ProjectId, RootId, TaskId, content_digest,
    coordination::envelope::{Sha256Digest, TaskRevisionRef},
    interfaces::{DecimalU64, InterfaceId},
    limits::bounded_json_size,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const INTERFACE_COMPAT_SCHEMA_VERSION: u32 = 1;
pub const MAX_INTERFACE_COMPAT_JSON_BYTES: usize = 256 * 1024;
pub const MAX_CONSUMER_TASK_REFS: usize = 256;
pub const MAX_ACCEPTANCE_CHECKS: usize = 64;
pub const MAX_PRIOR_WORK_REFS: usize = 128;
pub const MAX_REASON_BYTES: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConsumerCompatibilityPolicy {
    ExactProducerRevision {
        required_producer_revision: u32,
    },
    BackwardCompatibleRange {
        min_producer_revision: u32,
        max_producer_revision: u32,
        required_final_producer_revision: u32,
    },
    ProvisionalDevelopmentAllowed {
        min_provisional_producer_revision: u32,
        required_final_producer_revision: u32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum InterfaceBindingStatus {
    Provisional,
    Verified,
    Revoked {
        reason: String,
    },
    Superseded {
        superseded_by_interface_revision: DecimalU64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskInterfaceContractV2 {
    #[schemars(extend("const" = INTERFACE_COMPAT_SCHEMA_VERSION))]
    pub schema_version: u32,
    pub interface_id: InterfaceId,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub authority_ref: String,
    pub interface_revision: DecimalU64,
    pub producer_task: TaskRevisionRef,
    pub consumer_tasks: Vec<TaskRevisionRef>,
    pub artifact_digest: Sha256Digest,
    pub schema_digest: Sha256Digest,
    pub compatibility_policy: ConsumerCompatibilityPolicy,
    pub acceptance_check_ids: Vec<String>,
    pub status: InterfaceBindingStatus,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerEvaluationStage {
    ProvisionalDevelopment,
    FinalAcceptance,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConsumerCompatibilityInput {
    pub consumer_task: TaskRevisionRef,
    pub observed_producer_task: TaskRevisionRef,
    pub observed_artifact_digest: Sha256Digest,
    pub observed_schema_digest: Sha256Digest,
    pub passed_check_ids: Vec<String>,
    pub stage: ConsumerEvaluationStage,
    pub prior_work_refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConsumerCompatibilityVerdict {
    CompatibleProvisional {
        retained_work_refs: Vec<String>,
    },
    CompatibleFinalAcceptance {
        verified_binding_digest: String,
        retained_work_refs: Vec<String>,
    },
    IncompatibleRevision {
        reason: String,
        retained_work_refs: Vec<String>,
    },
    MissingAcceptanceChecks {
        missing_check_ids: Vec<String>,
        retained_work_refs: Vec<String>,
    },
    InvalidatedByRevocation {
        reason: String,
        readiness_invalidated: bool,
        receipts_invalidated: bool,
        retained_work_refs: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProducerRevocationImpact {
    pub interface_id: InterfaceId,
    pub revoked_producer_task: TaskRevisionRef,
    pub reason: String,
    pub affected_consumer_tasks: Vec<TaskId>,
    pub readiness_invalidated: bool,
    pub verification_receipts_invalidated: bool,
    pub prior_work_erased: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InterfaceCompatibilityError {
    #[error("interface compatibility input exceeds size or collection bounds")]
    BoundExceeded,
    #[error("invalid interface compatibility contract: {0}")]
    Invalid(&'static str),
}

fn valid_text(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
}

impl TaskInterfaceContractV2 {
    pub fn validate(&self) -> Result<(), InterfaceCompatibilityError> {
        bounded_json_size(self, MAX_INTERFACE_COMPAT_JSON_BYTES)
            .map_err(|_| InterfaceCompatibilityError::BoundExceeded)?;
        if self.schema_version != INTERFACE_COMPAT_SCHEMA_VERSION {
            return Err(InterfaceCompatibilityError::Invalid(
                "unsupported schema_version",
            ));
        }
        if !valid_text(&self.authority_ref, 512) || self.interface_revision.get() == 0 {
            return Err(InterfaceCompatibilityError::Invalid(
                "authority_ref and positive interface_revision are required",
            ));
        }
        if self.producer_task.revision == 0 {
            return Err(InterfaceCompatibilityError::Invalid(
                "producer_task.revision must be positive",
            ));
        }
        if self.consumer_tasks.is_empty() || self.consumer_tasks.len() > MAX_CONSUMER_TASK_REFS {
            return Err(InterfaceCompatibilityError::Invalid(
                "consumer_tasks must be non-empty and within bounds",
            ));
        }
        let mut consumer_ids = BTreeSet::new();
        for consumer in &self.consumer_tasks {
            if consumer.revision == 0
                || consumer.task_id == self.producer_task.task_id
                || !consumer_ids.insert(consumer.task_id.as_str())
            {
                return Err(InterfaceCompatibilityError::Invalid(
                    "consumer_tasks contains invalid revision, self-reference, or duplicate task_id",
                ));
            }
        }
        if self.acceptance_check_ids.is_empty()
            || self.acceptance_check_ids.len() > MAX_ACCEPTANCE_CHECKS
        {
            return Err(InterfaceCompatibilityError::Invalid(
                "acceptance_check_ids must be non-empty and within bounds",
            ));
        }
        let mut check_ids = BTreeSet::new();
        for check_id in &self.acceptance_check_ids {
            if !valid_text(check_id, 128) || !check_ids.insert(check_id.as_str()) {
                return Err(InterfaceCompatibilityError::Invalid(
                    "acceptance_check_ids contains invalid or duplicate check ID",
                ));
            }
        }
        match &self.compatibility_policy {
            ConsumerCompatibilityPolicy::ExactProducerRevision {
                required_producer_revision,
            } => {
                if *required_producer_revision != self.producer_task.revision {
                    return Err(InterfaceCompatibilityError::Invalid(
                        "exact policy required_producer_revision must match producer_task.revision",
                    ));
                }
            }
            ConsumerCompatibilityPolicy::BackwardCompatibleRange {
                min_producer_revision,
                max_producer_revision,
                required_final_producer_revision,
            } => {
                if *min_producer_revision == 0
                    || min_producer_revision > max_producer_revision
                    || *required_final_producer_revision < *min_producer_revision
                    || *required_final_producer_revision > *max_producer_revision
                    || *required_final_producer_revision != self.producer_task.revision
                {
                    return Err(InterfaceCompatibilityError::Invalid(
                        "backward_compatible_range bounds invalid or inconsistent with producer_task.revision",
                    ));
                }
            }
            ConsumerCompatibilityPolicy::ProvisionalDevelopmentAllowed {
                min_provisional_producer_revision,
                required_final_producer_revision,
            } => {
                if *min_provisional_producer_revision == 0
                    || min_provisional_producer_revision > required_final_producer_revision
                    || *required_final_producer_revision != self.producer_task.revision
                {
                    return Err(InterfaceCompatibilityError::Invalid(
                        "provisional policy bounds invalid or inconsistent with producer_task.revision",
                    ));
                }
            }
        }
        match &self.status {
            InterfaceBindingStatus::Provisional | InterfaceBindingStatus::Verified => {}
            InterfaceBindingStatus::Revoked { reason } => {
                if !valid_text(reason, MAX_REASON_BYTES) {
                    return Err(InterfaceCompatibilityError::Invalid(
                        "revocation reason must be non-empty and bounded",
                    ));
                }
            }
            InterfaceBindingStatus::Superseded {
                superseded_by_interface_revision,
            } => {
                if superseded_by_interface_revision.get() <= self.interface_revision.get() {
                    return Err(InterfaceCompatibilityError::Invalid(
                        "superseded_by_interface_revision must exceed current interface_revision",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn evaluate_consumer(
        &self,
        input: &ConsumerCompatibilityInput,
    ) -> Result<ConsumerCompatibilityVerdict, InterfaceCompatibilityError> {
        self.validate()?;
        if input.prior_work_refs.len() > MAX_PRIOR_WORK_REFS
            || input.prior_work_refs.iter().any(|r| !valid_text(r, 256))
            || input.passed_check_ids.len() > MAX_ACCEPTANCE_CHECKS
            || input.passed_check_ids.iter().any(|c| !valid_text(c, 128))
        {
            return Err(InterfaceCompatibilityError::BoundExceeded);
        }
        let retained = input.prior_work_refs.clone();

        let Some(registered_consumer) = self
            .consumer_tasks
            .iter()
            .find(|c| c.task_id == input.consumer_task.task_id)
        else {
            return Ok(ConsumerCompatibilityVerdict::IncompatibleRevision {
                reason: "consumer task is not registered on this interface contract".into(),
                retained_work_refs: retained,
            });
        };
        if registered_consumer != &input.consumer_task {
            return Ok(ConsumerCompatibilityVerdict::IncompatibleRevision {
                reason: "consumer task revision or contract_digest does not match binding".into(),
                retained_work_refs: retained,
            });
        }
        if input.observed_producer_task.task_id != self.producer_task.task_id {
            return Ok(ConsumerCompatibilityVerdict::IncompatibleRevision {
                reason: "observed producer task_id does not match interface producer".into(),
                retained_work_refs: retained,
            });
        }

        // Handle revoked or superseded interface status first: invalidates readiness and receipts
        // without erasing prior work.
        match &self.status {
            InterfaceBindingStatus::Revoked { reason } => {
                return Ok(ConsumerCompatibilityVerdict::InvalidatedByRevocation {
                    reason: reason.clone(),
                    readiness_invalidated: true,
                    receipts_invalidated: true,
                    retained_work_refs: retained,
                });
            }
            InterfaceBindingStatus::Superseded {
                superseded_by_interface_revision,
            } => {
                return Ok(ConsumerCompatibilityVerdict::InvalidatedByRevocation {
                    reason: format!(
                        "interface revision {} superseded by {}",
                        self.interface_revision.get(),
                        superseded_by_interface_revision.get()
                    ),
                    readiness_invalidated: true,
                    receipts_invalidated: true,
                    retained_work_refs: retained,
                });
            }
            InterfaceBindingStatus::Provisional | InterfaceBindingStatus::Verified => {}
        }

        match input.stage {
            ConsumerEvaluationStage::ProvisionalDevelopment => {
                let revision_permitted = match &self.compatibility_policy {
                    ConsumerCompatibilityPolicy::ExactProducerRevision {
                        required_producer_revision,
                    } => {
                        input.observed_producer_task == self.producer_task
                            && input.observed_producer_task.revision == *required_producer_revision
                    }
                    ConsumerCompatibilityPolicy::BackwardCompatibleRange {
                        min_producer_revision,
                        max_producer_revision,
                        ..
                    } => (min_producer_revision..=max_producer_revision)
                        .contains(&&input.observed_producer_task.revision),
                    ConsumerCompatibilityPolicy::ProvisionalDevelopmentAllowed {
                        min_provisional_producer_revision,
                        required_final_producer_revision,
                    } => (min_provisional_producer_revision..=required_final_producer_revision)
                        .contains(&&input.observed_producer_task.revision),
                };
                if !revision_permitted || input.observed_schema_digest != self.schema_digest {
                    return Ok(ConsumerCompatibilityVerdict::IncompatibleRevision {
                        reason: "observed producer revision or schema_digest outside compatibility policy".into(),
                        retained_work_refs: retained,
                    });
                }
                Ok(ConsumerCompatibilityVerdict::CompatibleProvisional {
                    retained_work_refs: retained,
                })
            }
            ConsumerEvaluationStage::FinalAcceptance => {
                // Final acceptance strictly requires Verified status, exact producer revision + digest,
                // exact artifact_digest + schema_digest, and all acceptance_check_ids passed.
                if self.status != InterfaceBindingStatus::Verified {
                    return Ok(ConsumerCompatibilityVerdict::IncompatibleRevision {
                        reason: "provisional interface binding cannot satisfy final acceptance"
                            .into(),
                        retained_work_refs: retained,
                    });
                }
                if input.observed_producer_task != self.producer_task
                    || input.observed_artifact_digest != self.artifact_digest
                    || input.observed_schema_digest != self.schema_digest
                {
                    return Ok(ConsumerCompatibilityVerdict::IncompatibleRevision {
                        reason: "final acceptance requires exact producer TaskRevisionRef, artifact_digest, and schema_digest".into(),
                        retained_work_refs: retained,
                    });
                }
                let passed: BTreeSet<&str> =
                    input.passed_check_ids.iter().map(String::as_str).collect();
                let missing: Vec<String> = self
                    .acceptance_check_ids
                    .iter()
                    .filter(|req| !passed.contains(req.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    return Ok(ConsumerCompatibilityVerdict::MissingAcceptanceChecks {
                        missing_check_ids: missing,
                        retained_work_refs: retained,
                    });
                }
                let verified_binding_digest = content_digest(self).map_err(|_| {
                    InterfaceCompatibilityError::Invalid("cannot digest TaskInterfaceContractV2")
                })?;
                Ok(ConsumerCompatibilityVerdict::CompatibleFinalAcceptance {
                    verified_binding_digest,
                    retained_work_refs: retained,
                })
            }
        }
    }

    pub fn revoke(
        &self,
        reason: impl Into<String>,
    ) -> Result<(Self, ProducerRevocationImpact), InterfaceCompatibilityError> {
        self.validate()?;
        let reason = reason.into();
        if !valid_text(&reason, MAX_REASON_BYTES) {
            return Err(InterfaceCompatibilityError::Invalid(
                "revocation reason must be non-empty and bounded",
            ));
        }
        let mut next = self.clone();
        next.status = InterfaceBindingStatus::Revoked {
            reason: reason.clone(),
        };
        next.validate()?;
        let affected_consumer_tasks = self
            .consumer_tasks
            .iter()
            .map(|c| c.task_id.clone())
            .collect();
        let impact = ProducerRevocationImpact {
            interface_id: self.interface_id.clone(),
            revoked_producer_task: self.producer_task.clone(),
            reason,
            affected_consumer_tasks,
            readiness_invalidated: true,
            verification_receipts_invalidated: true,
            prior_work_erased: false,
        };
        Ok((next, impact))
    }
}
