//! Portable verification receipt parsing and consistency checks.
//!
//! This module validates receipt contents and digest shape. It does not run
//! checks, authenticate the named verifier, or grant acceptance.

use crate::{CheckSpec, PrincipalId, ProjectId, RootId, TaskContract, TaskId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
pub const MAX_RECEIPT_CHECKS: usize = 256;
const MAX_RECEIPT_ID_BYTES: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationInputs {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub source_artifact_digest: String,
    pub protected_oracle_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptProducer {
    IndependentVerifier,
    ProviderClaim,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub check: CheckSpec,
    pub exit_code: Option<i32>,
    pub stdout_digest: String,
    pub stderr_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationReceipt {
    pub schema_version: u32,
    pub receipt_id: String,
    pub producer: ReceiptProducer,
    pub verifier: PrincipalId,
    pub inputs: VerificationInputs,
    pub checks: Vec<CheckResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidatedReceipt {
    pub receipt_digest: String,
    pub all_checks_passed: bool,
    /// Exact-check consistency is not task acceptance or integration approval.
    pub acceptance_granted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid verification receipt: {0}")]
pub struct VerificationError(pub String);

/// Parse bounded JSON and reject unknown fields through the receipt types.
pub fn parse_receipt(bytes: &[u8]) -> Result<VerificationReceipt, VerificationError> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(VerificationError("receipt exceeds 1 MiB".into()));
    }
    let receipt: VerificationReceipt = serde_json::from_slice(bytes)
        .map_err(|error| VerificationError(format!("invalid receipt JSON: {error}")))?;
    if receipt.checks.len() > MAX_RECEIPT_CHECKS {
        return Err(VerificationError(
            "receipt contains more than 256 checks".into(),
        ));
    }
    Ok(receipt)
}

/// Validate immutable inputs and exact argv checks against a task contract.
/// A passing result means only that the receipt is internally consistent.
pub fn validate_receipt(
    contract: &TaskContract,
    expected: &VerificationInputs,
    receipt: &VerificationReceipt,
) -> Result<ValidatedReceipt, VerificationError> {
    contract
        .validate()
        .map_err(|error| VerificationError(format!("invalid task contract: {error}")))?;
    validate_inputs(expected)?;

    if expected.project_id != contract.project_id
        || expected.root_id != contract.root_id
        || expected.task_id != contract.task_id
        || expected.contract_revision != contract.revision
    {
        return Err(VerificationError(
            "expected contract identity or revision differs from original contract".into(),
        ));
    }
    let original_contract_digest = crate::content_digest(contract)
        .map_err(|error| VerificationError(format!("cannot digest original contract: {error}")))?;
    if expected.contract_digest != original_contract_digest {
        return Err(VerificationError(
            "expected contract digest differs from original contract".into(),
        ));
    }
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
        return Err(VerificationError(
            "unsupported receipt schema version".into(),
        ));
    }
    if receipt.receipt_id.trim().is_empty() || receipt.receipt_id.len() > MAX_RECEIPT_ID_BYTES {
        return Err(VerificationError(
            "receipt ID must contain 1..128 bytes".into(),
        ));
    }
    if PrincipalId::new(receipt.verifier.as_str()).is_err() {
        return Err(VerificationError("invalid verifier principal ID".into()));
    }
    if receipt.producer != ReceiptProducer::IndependentVerifier {
        return Err(VerificationError(
            "provider claims are not independent verification receipts".into(),
        ));
    }
    if &receipt.inputs != expected {
        return Err(VerificationError(
            "receipt inputs differ from independently expected immutable inputs".into(),
        ));
    }
    if receipt.checks.len() > MAX_RECEIPT_CHECKS {
        return Err(VerificationError(
            "receipt contains more than 256 checks".into(),
        ));
    }
    if receipt.checks.len() != contract.checks.len() {
        return Err(VerificationError(
            "receipt must contain every contract check exactly once".into(),
        ));
    }

    let mut all_checks_passed = true;
    for (result, expected_check) in receipt.checks.iter().zip(&contract.checks) {
        if &result.check != expected_check {
            return Err(VerificationError(format!(
                "receipt check {:?} does not exactly match its contract check",
                result.check.id
            )));
        }
        validate_sha256_ref(&result.stdout_digest, "stdout digest")?;
        validate_sha256_ref(&result.stderr_digest, "stderr digest")?;
        all_checks_passed &= result.exit_code == Some(0);
    }

    let serialized = serde_json::to_vec(receipt)
        .map_err(|error| VerificationError(format!("cannot serialize receipt: {error}")))?;
    Ok(ValidatedReceipt {
        receipt_digest: sha256_ref(&serialized),
        all_checks_passed,
        acceptance_granted: false,
    })
}

fn validate_inputs(inputs: &VerificationInputs) -> Result<(), VerificationError> {
    if inputs.contract_revision == 0 {
        return Err(VerificationError(
            "contract revision must be nonzero".into(),
        ));
    }
    validate_sha256_ref(&inputs.contract_digest, "contract digest")?;
    validate_git_sha(&inputs.source_commit_sha, "source commit SHA")?;
    validate_git_sha(&inputs.source_tree_sha, "source tree SHA")?;
    validate_sha256_ref(&inputs.source_artifact_digest, "source artifact digest")?;
    validate_sha256_ref(&inputs.protected_oracle_digest, "protected oracle digest")?;
    Ok(())
}

fn validate_git_sha(value: &str, name: &str) -> Result<(), VerificationError> {
    if (value.len() != 40 && value.len() != 64)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(VerificationError(format!(
            "{name} must be a full lowercase 40- or 64-character hex object ID"
        )));
    }
    Ok(())
}

fn validate_sha256_ref(value: &str, name: &str) -> Result<(), VerificationError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(VerificationError(format!(
            "{name} must use sha256:<64 lowercase hex>"
        )));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(VerificationError(format!(
            "{name} must use sha256:<64 lowercase hex>"
        )));
    }
    Ok(())
}

fn sha256_ref(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcceptanceCriterion, CheckSpec};

    fn project(value: &str) -> ProjectId {
        ProjectId::new(value).unwrap()
    }
    fn root(value: &str) -> RootId {
        RootId::new(value).unwrap()
    }
    fn task(value: &str) -> TaskId {
        TaskId::new(value).unwrap()
    }
    fn principal(value: &str) -> PrincipalId {
        PrincipalId::new(value).unwrap()
    }
    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }
    fn contract() -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: project("p1"),
            root_id: root("r1"),
            task_id: task("t1"),
            revision: 3,
            title: "verify candidate".into(),
            objective: "run exact checks".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "C1".into(),
                text: "checks are recorded".into(),
                check_ids: vec!["unit".into(), "lint".into()],
            }],
            checks: vec![
                CheckSpec {
                    id: "unit".into(),
                    program: "cargo".into(),
                    args: vec!["test".into(), "-p".into(), "x".into()],
                    cwd: ".".into(),
                    timeout_secs: 60,
                },
                CheckSpec {
                    id: "lint".into(),
                    program: "cargo".into(),
                    args: vec!["clippy".into(), "--".into(), "-Dwarnings".into()],
                    cwd: "subdir".into(),
                    timeout_secs: 90,
                },
            ],
            authority_ref: "grant:verify".into(),
            max_attempts: 1,
            simulated: false,
        }
    }
    fn inputs() -> VerificationInputs {
        VerificationInputs {
            project_id: project("p1"),
            root_id: root("r1"),
            task_id: task("t1"),
            contract_revision: 3,
            contract_digest: crate::content_digest(&contract()).unwrap(),
            source_commit_sha: "1".repeat(40),
            source_tree_sha: "2".repeat(40),
            source_artifact_digest: digest('b'),
            protected_oracle_digest: digest('c'),
        }
    }
    fn receipt() -> VerificationReceipt {
        VerificationReceipt {
            schema_version: 1,
            receipt_id: "receipt-1".into(),
            producer: ReceiptProducer::IndependentVerifier,
            verifier: principal("verifier-1"),
            inputs: inputs(),
            checks: contract()
                .checks
                .into_iter()
                .map(|check| CheckResult {
                    check,
                    exit_code: Some(0),
                    stdout_digest: digest('d'),
                    stderr_digest: digest('e'),
                })
                .collect(),
        }
    }

    #[test]
    fn full_receipt_is_deterministic_and_never_grants_acceptance() {
        let contract = contract();
        let expected = inputs();
        let receipt = receipt();
        let first = validate_receipt(&contract, &expected, &receipt).unwrap();
        let replay = validate_receipt(&contract, &expected, &receipt).unwrap();
        assert_eq!(first, replay);
        assert!(first.all_checks_passed);
        assert!(!first.acceptance_granted);
    }

    #[test]
    fn provider_claim_is_not_independent_verification() {
        let mut receipt = receipt();
        receipt.producer = ReceiptProducer::ProviderClaim;
        assert!(validate_receipt(&contract(), &inputs(), &receipt).is_err());
    }

    #[test]
    fn every_expected_input_is_bound_and_contract_scope_must_match() {
        let original_contract = contract();
        let original_inputs = inputs();
        let original_receipt = receipt();
        for mutate in 0..9 {
            let mut changed = original_inputs.clone();
            match mutate {
                0 => changed.project_id = project("other-project"),
                1 => changed.root_id = root("other-root"),
                2 => changed.task_id = task("other-task"),
                3 => changed.contract_revision += 1,
                4 => changed.contract_digest = digest('f'),
                5 => changed.source_commit_sha = "3".repeat(40),
                6 => changed.source_tree_sha = "4".repeat(40),
                7 => changed.source_artifact_digest = digest('f'),
                _ => changed.protected_oracle_digest = digest('f'),
            }
            assert!(validate_receipt(&original_contract, &changed, &original_receipt).is_err());
        }
        let mut changed_contract = original_contract.clone();
        changed_contract.revision += 1;
        assert!(validate_receipt(&changed_contract, &original_inputs, &original_receipt).is_err());
        changed_contract = original_contract;
        changed_contract.task_id = task("different-task");
        assert!(validate_receipt(&changed_contract, &original_inputs, &original_receipt).is_err());
        changed_contract = contract();
        changed_contract.owned_paths = vec!["different-scope".into()];
        assert!(validate_receipt(&changed_contract, &original_inputs, &original_receipt).is_err());
    }

    #[test]
    fn parsing_rejects_wrong_typed_ids_unknown_fields_and_oversize() {
        let mut value = serde_json::to_value(receipt()).unwrap();
        value["inputs"]["project_id"] = serde_json::json!("../escape");
        assert!(parse_receipt(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::to_value(receipt()).unwrap();
        value["inputs"]["root_id"] = serde_json::json!("bad/id");
        assert!(parse_receipt(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::to_value(receipt()).unwrap();
        value["inputs"]["task_id"] = serde_json::json!(" ");
        assert!(parse_receipt(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::to_value(receipt()).unwrap();
        value["verifier"] = serde_json::json!("../forged");
        assert!(parse_receipt(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::to_value(receipt()).unwrap();
        value["unrecognized"] = serde_json::json!(true);
        assert!(parse_receipt(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(parse_receipt(&vec![b' '; MAX_RECEIPT_BYTES + 1]).is_err());
        value = serde_json::to_value(receipt()).unwrap();
        value["checks"] = serde_json::Value::Array(
            (0..=MAX_RECEIPT_CHECKS)
                .map(|_| serde_json::to_value(receipt().checks[0].clone()).unwrap())
                .collect(),
        );
        assert!(parse_receipt(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn check_set_must_be_complete_unique_ordered_and_exact_argv() {
        let contract = contract();
        let expected = inputs();
        let mut changed = receipt();
        changed.checks.pop();
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks[1] = changed.checks[0].clone();
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks.swap(0, 1);
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks[0].check.args[0] = "build".into();
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks[0].check.id = "renamed".into();
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks[0].check.program = "sh".into();
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks[0].check.cwd = "elsewhere".into();
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks[0].check.timeout_secs += 1;
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
        changed = receipt();
        changed.checks.push(changed.checks[0].clone());
        assert!(validate_receipt(&contract, &expected, &changed).is_err());
    }

    #[test]
    fn malformed_or_missing_digests_fail_instead_of_passing() {
        let contract = contract();
        let expected = inputs();
        let mut malformed = receipt();
        malformed.inputs.source_tree_sha = "deadbeef".into();
        assert!(validate_receipt(&contract, &expected, &malformed).is_err());
        malformed = receipt();
        malformed.inputs.protected_oracle_digest.clear();
        assert!(validate_receipt(&contract, &expected, &malformed).is_err());
        malformed = receipt();
        malformed.checks[0].stdout_digest = "sha256:ABC".into();
        assert!(validate_receipt(&contract, &expected, &malformed).is_err());
        malformed = receipt();
        malformed.inputs.source_commit_sha = "a".repeat(39);
        assert!(validate_receipt(&contract, &expected, &malformed).is_err());
    }

    #[test]
    fn unknown_and_nonzero_exits_do_not_pass_and_evidence_changes_digest() {
        let contract = contract();
        let expected = inputs();
        let baseline = receipt();
        let digest_before = validate_receipt(&contract, &expected, &baseline)
            .unwrap()
            .receipt_digest;
        let mut changed = baseline.clone();
        changed.checks[0].exit_code = None;
        assert!(
            !validate_receipt(&contract, &expected, &changed)
                .unwrap()
                .all_checks_passed
        );
        changed.checks[0].exit_code = Some(9);
        assert!(
            !validate_receipt(&contract, &expected, &changed)
                .unwrap()
                .all_checks_passed
        );
        changed = baseline;
        changed.checks[0].stdout_digest = digest('f');
        let digest_after = validate_receipt(&contract, &expected, &changed)
            .unwrap()
            .receipt_digest;
        assert_ne!(digest_before, digest_after);
    }
}
