//! Portable verification receipt parsing and consistency checks.
//!
//! This module validates receipt contents and digest shape. It does not run
//! checks, authenticate the named verifier, or grant acceptance.

use crate::{AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract, TaskId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
pub const MAX_RECEIPT_CHECKS: usize = 256;
pub const MAX_NEGATIVE_CHECKS: usize = 64;
pub const MAX_RETAINED_RECEIPTS: usize = 4_096;
pub const MAX_CHECK_ARGS: usize = 128;
pub const MAX_CHECK_ARG_BYTES: usize = 4_096;
pub const MAX_CHECK_TIMEOUT_SECS: u32 = 86_400;
const MAX_RECEIPT_ID_BYTES: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationInputs {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub verification_policy_digest: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub source_artifact_digest: String,
    pub protected_oracle_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptProducer {
    IndependentVerifier,
    ProviderClaim,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub check: CheckSpec,
    #[schemars(range(min = 0, max = 255))]
    pub exit_code: Option<i32>,
    pub stdout_digest: String,
    pub stderr_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredNegativeCheck {
    pub check: CheckSpec,
    /// A rejecting command can expect a nonzero exit; an assertion test that
    /// proves rejection normally expects zero. The receipt cannot choose it.
    #[schemars(range(min = 0, max = 255))]
    pub expected_exit_code: i32,
    pub fixture_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationPolicy {
    pub schema_version: u32,
    /// Expected identity remains an unauthenticated claim until the caller
    /// verifies custody and actor identity outside this pure component.
    pub verifier: PrincipalId,
    pub protected_oracle_digest: String,
    #[schemars(length(min = 1, max = MAX_NEGATIVE_CHECKS))]
    pub required_negative_checks: Vec<RequiredNegativeCheck>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationReceipt {
    pub schema_version: u32,
    pub receipt_id: String,
    pub producer: ReceiptProducer,
    pub verifier: PrincipalId,
    pub inputs: VerificationInputs,
    #[schemars(length(max = MAX_RECEIPT_CHECKS))]
    pub checks: Vec<CheckResult>,
    #[schemars(length(max = MAX_NEGATIVE_CHECKS))]
    pub negative_checks: Vec<CheckResult>,
}

/// Serialize-only result with no public constructor or mutable trust flags.
/// It must never be reconstructed from an untrusted receipt/JSON claim.
///
/// ```compile_fail
/// use admiralty_core::verification::ValidatedReceipt;
/// let _: ValidatedReceipt = serde_json::from_str("{}")
///     .expect("untrusted JSON must not reconstruct validator results");
/// ```
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ValidatedReceipt {
    receipt_digest: String,
    /// Whether the claimed exits match policy, not proof of execution.
    all_checks_passed: bool,
    /// Exact-check consistency is not task acceptance or integration approval.
    acceptance_granted: bool,
    verifier_authenticated: bool,
    execution_qualified: bool,
    evidence_custody_verified: bool,
}

impl ValidatedReceipt {
    pub fn receipt_digest(&self) -> &str {
        &self.receipt_digest
    }
    pub fn all_checks_passed(&self) -> bool {
        self.all_checks_passed
    }
    pub fn acceptance_granted(&self) -> bool {
        self.acceptance_granted
    }
    pub fn verifier_authenticated(&self) -> bool {
        self.verifier_authenticated
    }
    pub fn execution_qualified(&self) -> bool {
        self.execution_qualified
    }
    pub fn evidence_custody_verified(&self) -> bool {
        self.evidence_custody_verified
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptObservation {
    pub result: ValidatedReceipt,
    pub replayed: bool,
}

/// Pure bounded identity history; the caller must durably journal operations
/// and persist observation/outbox atomically. No silent history retirement.
#[derive(Clone, Debug)]
pub struct ReceiptRegistry {
    max_receipts: usize,
    receipts: BTreeMap<String, String>,
}

impl ReceiptRegistry {
    pub fn new(max_receipts: usize) -> Result<Self, VerificationError> {
        if !(1..=MAX_RETAINED_RECEIPTS).contains(&max_receipts) {
            return Err(VerificationError("invalid bounded receipt history".into()));
        }
        Ok(Self {
            max_receipts,
            receipts: BTreeMap::new(),
        })
    }
    pub fn retained_receipts(&self) -> usize {
        self.receipts.len()
    }
    pub fn validate_and_record(
        &mut self,
        contract: &TaskContract,
        expected: &VerificationInputs,
        policy: &VerificationPolicy,
        receipt: &VerificationReceipt,
    ) -> Result<ReceiptObservation, VerificationError> {
        let result = validate_receipt(contract, expected, policy, receipt)?;
        if let Some(previous) = self.receipts.get(&receipt.receipt_id) {
            if previous != result.receipt_digest() {
                return Err(VerificationError(
                    "receipt ID reused with different content".into(),
                ));
            }
            return Ok(ReceiptObservation {
                result,
                replayed: true,
            });
        }
        if self.receipts.len() >= self.max_receipts {
            return Err(VerificationError(
                "receipt history full; retain input upstream".into(),
            ));
        }
        self.receipts
            .insert(receipt.receipt_id.clone(), result.receipt_digest.clone());
        Ok(ReceiptObservation {
            result,
            replayed: false,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid verification receipt: {0}")]
pub struct VerificationError(pub String);

/// Parse bounded JSON and reject unknown fields through the receipt types.
pub fn parse_receipt(bytes: &[u8]) -> Result<VerificationReceipt, VerificationError> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(VerificationError("receipt exceeds 1 MiB".into()));
    }
    let receipt: VerificationReceipt = serde_json::from_slice(bytes).map_err(|error| {
        // Do not echo arbitrary field names or purported output into logs.
        VerificationError(format!(
            "invalid receipt JSON at line {}, column {}",
            error.line(),
            error.column()
        ))
    })?;
    validate_receipt_shape(&receipt)?;
    Ok(receipt)
}

/// Validate immutable inputs and exact argv checks against a task contract.
/// A passing result means only that the receipt is internally consistent.
pub fn validate_receipt(
    contract: &TaskContract,
    expected: &VerificationInputs,
    policy: &VerificationPolicy,
    receipt: &VerificationReceipt,
) -> Result<ValidatedReceipt, VerificationError> {
    validate_contract_bounds(contract)?;
    contract
        .validate()
        .map_err(|error| VerificationError(format!("invalid task contract: {error}")))?;
    validate_inputs(expected)?;
    validate_policy(policy)?;
    validate_receipt_shape(receipt)?;

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
    if expected.protected_oracle_digest != policy.protected_oracle_digest
        || expected.verification_policy_digest
            != crate::content_digest(policy)
                .map_err(|_| VerificationError("cannot digest verification policy".into()))?
    {
        return Err(VerificationError(
            "expected inputs do not pin the independent verification policy".into(),
        ));
    }
    if receipt.verifier != policy.verifier {
        return Err(VerificationError(
            "receipt verifier differs from pinned policy identity".into(),
        ));
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
    if receipt.checks.len() != contract.checks.len() {
        return Err(VerificationError(
            "receipt must contain every contract check exactly once".into(),
        ));
    }
    if receipt.negative_checks.len() != policy.required_negative_checks.len() {
        return Err(VerificationError(
            "receipt must contain every required negative check exactly once".into(),
        ));
    }
    let mut check_ids: BTreeSet<_> = contract
        .checks
        .iter()
        .map(|check| check.id.as_str())
        .collect();
    if policy
        .required_negative_checks
        .iter()
        .any(|negative| !check_ids.insert(negative.check.id.as_str()))
    {
        return Err(VerificationError(
            "contract and negative checks require distinct IDs".into(),
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
        all_checks_passed &= result.exit_code == Some(0);
    }
    for (result, required) in receipt
        .negative_checks
        .iter()
        .zip(&policy.required_negative_checks)
    {
        if result.check != required.check {
            return Err(VerificationError(
                "negative check differs from exact protected policy argv".into(),
            ));
        }
        all_checks_passed &= result.exit_code == Some(required.expected_exit_code);
    }
    Ok(ValidatedReceipt {
        receipt_digest: crate::content_digest(receipt)
            .map_err(|_| VerificationError("cannot digest bounded receipt".into()))?,
        all_checks_passed,
        acceptance_granted: false,
        verifier_authenticated: false,
        execution_qualified: false,
        evidence_custody_verified: false,
    })
}

fn validate_inputs(inputs: &VerificationInputs) -> Result<(), VerificationError> {
    if inputs.contract_revision == 0 {
        return Err(VerificationError(
            "contract revision must be nonzero".into(),
        ));
    }
    validate_sha256_ref(&inputs.contract_digest, "contract digest")?;
    validate_sha256_ref(
        &inputs.verification_policy_digest,
        "verification policy digest",
    )?;
    validate_git_sha(&inputs.source_commit_sha, "source commit SHA")?;
    validate_git_sha(&inputs.source_tree_sha, "source tree SHA")?;
    validate_sha256_ref(&inputs.source_artifact_digest, "source artifact digest")?;
    validate_sha256_ref(&inputs.protected_oracle_digest, "protected oracle digest")?;
    if inputs.source_commit_sha.len() != inputs.source_tree_sha.len()
        || inputs.source_commit_sha == inputs.source_tree_sha
    {
        return Err(VerificationError(
            "commit and tree must be distinct full IDs in the same Git hash format".into(),
        ));
    }
    if !valid_ref(&inputs.candidate_ref) {
        return Err(VerificationError(
            "invalid bounded candidate reference".into(),
        ));
    }
    Ok(())
}

fn validate_git_sha(value: &str, name: &str) -> Result<(), VerificationError> {
    if (value.len() != 40 && value.len() != 64)
        || value.bytes().all(|byte| byte == b'0')
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

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
}

fn bounded_json<T: Serialize>(value: &T) -> Result<(), VerificationError> {
    serde_json::to_writer(ReceiptSize(MAX_RECEIPT_BYTES), value)
        .map_err(|_| VerificationError("serialized verification input exceeds 1 MiB".into()))
}

struct ReceiptSize(usize);
impl std::io::Write for ReceiptSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("bounded verification JSON"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_check(check: &CheckSpec) -> Result<(), VerificationError> {
    if PrincipalId::new(check.id.as_str()).is_err()
        || check.program.trim().is_empty()
        || check.program.len() > 256
        || check.program.chars().any(char::is_control)
        || check.cwd.len() > 1_024
        || !crate::contracts::safe_relative_path(&check.cwd)
        || check.cwd.chars().any(char::is_control)
        || check.timeout_secs == 0
        || check.timeout_secs > MAX_CHECK_TIMEOUT_SECS
        || check.args.len() > MAX_CHECK_ARGS
        || check
            .args
            .iter()
            .any(|arg| arg.len() > MAX_CHECK_ARG_BYTES || arg.contains('\0'))
    {
        return Err(VerificationError(
            "check ID, program, argv, cwd or timeout exceeds validated bounds".into(),
        ));
    }
    Ok(())
}

fn validate_policy(policy: &VerificationPolicy) -> Result<(), VerificationError> {
    if policy.schema_version != RECEIPT_SCHEMA_VERSION
        || policy.required_negative_checks.is_empty()
        || policy.required_negative_checks.len() > MAX_NEGATIVE_CHECKS
    {
        return Err(VerificationError(
            "versioned policy requires 1..64 protected negative checks".into(),
        ));
    }
    validate_sha256_ref(
        &policy.protected_oracle_digest,
        "policy protected oracle digest",
    )?;
    let mut ids = BTreeSet::new();
    for negative in &policy.required_negative_checks {
        validate_check(&negative.check)?;
        validate_sha256_ref(&negative.fixture_digest, "negative fixture digest")?;
        if !(0..=255).contains(&negative.expected_exit_code) || !ids.insert(&negative.check.id) {
            return Err(VerificationError(
                "negative checks need unique IDs and portable expected exits".into(),
            ));
        }
    }
    bounded_json(policy)
}

fn validate_receipt_shape(receipt: &VerificationReceipt) -> Result<(), VerificationError> {
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
        return Err(VerificationError(
            "unsupported receipt schema version".into(),
        ));
    }
    if receipt.receipt_id.len() > MAX_RECEIPT_ID_BYTES
        || PrincipalId::new(receipt.receipt_id.as_str()).is_err()
    {
        return Err(VerificationError(
            "receipt ID requires 1..128 safe ASCII identity bytes".into(),
        ));
    }
    validate_inputs(&receipt.inputs)?;
    if receipt.checks.is_empty()
        || receipt.negative_checks.is_empty()
        || receipt.checks.len() > MAX_RECEIPT_CHECKS
        || receipt.negative_checks.len() > MAX_NEGATIVE_CHECKS
        || receipt
            .checks
            .len()
            .checked_add(receipt.negative_checks.len())
            .is_none_or(|count| count > MAX_RECEIPT_CHECKS)
    {
        return Err(VerificationError(
            "receipt needs positive and negative checks, at most 256 total".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for result in receipt.checks.iter().chain(&receipt.negative_checks) {
        validate_check(&result.check)?;
        validate_sha256_ref(&result.stdout_digest, "stdout digest")?;
        validate_sha256_ref(&result.stderr_digest, "stderr digest")?;
        if result
            .exit_code
            .is_some_and(|code| !(0..=255).contains(&code))
            || !ids.insert(&result.check.id)
        {
            return Err(VerificationError(
                "check identities must be unique and exits portable or unknown".into(),
            ));
        }
    }
    bounded_json(receipt)
}

fn validate_contract_bounds(contract: &TaskContract) -> Result<(), VerificationError> {
    let bounded_text = |value: &str, max| !value.trim().is_empty() && value.len() <= max;
    if !bounded_text(&contract.title, 512)
        || !bounded_text(&contract.objective, 16_384)
        || !valid_ref(&contract.authority_ref)
        || contract.non_goals.len() > 64
        || contract
            .non_goals
            .iter()
            .any(|value| !bounded_text(value, 4_096))
        || contract.dependencies.len() > 256
        || contract.criteria.is_empty()
        || contract.criteria.len() > 256
        || contract.checks.is_empty()
        || contract.checks.len() > MAX_RECEIPT_CHECKS
    {
        return Err(VerificationError(
            "task contract exceeds verification text or count bounds".into(),
        ));
    }
    for paths in [
        &contract.owned_paths,
        &contract.forbidden_paths,
        &contract.read_first,
    ] {
        if paths.len() > 256 || paths.iter().any(|path| !bounded_text(path, 1_024)) {
            return Err(VerificationError(
                "task path collection exceeds verification bounds".into(),
            ));
        }
    }
    for criterion in &contract.criteria {
        if PrincipalId::new(criterion.id.as_str()).is_err()
            || !bounded_text(&criterion.text, 4_096)
            || criterion.check_ids.is_empty()
            || criterion.check_ids.len() > MAX_RECEIPT_CHECKS
            || criterion
                .check_ids
                .iter()
                .any(|id| PrincipalId::new(id.as_str()).is_err())
        {
            return Err(VerificationError(
                "acceptance criterion exceeds verification bounds".into(),
            ));
        }
    }
    for check in &contract.checks {
        validate_check(check)?;
    }
    bounded_json(contract)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcceptanceCriterion, CheckSpec};

    fn project(value: &str) -> ProjectId {
        ProjectId::new(value).expect("valid static verification fixture")
    }
    fn root(value: &str) -> RootId {
        RootId::new(value).expect("valid static verification fixture")
    }
    fn task(value: &str) -> TaskId {
        TaskId::new(value).expect("valid static verification fixture")
    }
    fn principal(value: &str) -> PrincipalId {
        PrincipalId::new(value).expect("valid static verification fixture")
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
    fn policy() -> VerificationPolicy {
        VerificationPolicy {
            schema_version: 1,
            verifier: principal("verifier-1"),
            protected_oracle_digest: digest('c'),
            required_negative_checks: vec![RequiredNegativeCheck {
                check: CheckSpec {
                    id: "negative-scope".into(),
                    program: "fixture-verifier".into(),
                    args: vec!["--reject-foreign-scope".into()],
                    cwd: ".".into(),
                    timeout_secs: 5,
                },
                expected_exit_code: 1,
                fixture_digest: digest('a'),
            }],
        }
    }
    fn inputs() -> VerificationInputs {
        VerificationInputs {
            project_id: project("p1"),
            root_id: root("r1"),
            task_id: task("t1"),
            attempt_id: AttemptId::new("attempt-1").expect("fixture attempt"),
            candidate_ref: "candidate:immutable-one".into(),
            contract_revision: 3,
            verification_policy_digest: crate::content_digest(&policy())
                .expect("fixture policy digest"),
            contract_digest: crate::content_digest(&contract())
                .expect("valid static verification fixture"),
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
            negative_checks: policy()
                .required_negative_checks
                .into_iter()
                .map(|negative| CheckResult {
                    check: negative.check,
                    exit_code: Some(negative.expected_exit_code),
                    stdout_digest: digest('d'),
                    stderr_digest: digest('e'),
                })
                .collect(),
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
        let first = validate_receipt(&contract, &expected, &policy(), &receipt)
            .expect("valid static verification fixture");
        let replay = validate_receipt(&contract, &expected, &policy(), &receipt)
            .expect("valid static verification fixture");
        assert_eq!(first, replay);
        assert!(first.all_checks_passed);
        assert!(!first.acceptance_granted);
        assert!(!first.acceptance_granted());
        assert!(!first.verifier_authenticated());
        assert!(!first.execution_qualified());
        assert!(!first.evidence_custody_verified());
    }

    #[test]
    fn provider_claim_is_not_independent_verification() {
        let mut receipt = receipt();
        receipt.producer = ReceiptProducer::ProviderClaim;
        assert!(validate_receipt(&contract(), &inputs(), &policy(), &receipt).is_err());
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
            assert!(
                validate_receipt(&original_contract, &changed, &policy(), &original_receipt)
                    .is_err()
            );
        }
        let mut changed_contract = original_contract.clone();
        changed_contract.revision += 1;
        assert!(
            validate_receipt(
                &changed_contract,
                &original_inputs,
                &policy(),
                &original_receipt
            )
            .is_err()
        );
        changed_contract = original_contract;
        changed_contract.task_id = task("different-task");
        assert!(
            validate_receipt(
                &changed_contract,
                &original_inputs,
                &policy(),
                &original_receipt
            )
            .is_err()
        );
        changed_contract = contract();
        changed_contract.owned_paths = vec!["different-scope".into()];
        assert!(
            validate_receipt(
                &changed_contract,
                &original_inputs,
                &policy(),
                &original_receipt
            )
            .is_err()
        );
    }

    #[test]
    fn parsing_rejects_wrong_typed_ids_unknown_fields_and_oversize() {
        let mut value = serde_json::to_value(receipt()).expect("valid static verification fixture");
        value["inputs"]["project_id"] = serde_json::json!("../escape");
        assert!(
            parse_receipt(&serde_json::to_vec(&value).expect("valid static verification fixture"))
                .is_err()
        );
        value = serde_json::to_value(receipt()).expect("valid static verification fixture");
        value["inputs"]["root_id"] = serde_json::json!("bad/id");
        assert!(
            parse_receipt(&serde_json::to_vec(&value).expect("valid static verification fixture"))
                .is_err()
        );
        value = serde_json::to_value(receipt()).expect("valid static verification fixture");
        value["inputs"]["task_id"] = serde_json::json!(" ");
        assert!(
            parse_receipt(&serde_json::to_vec(&value).expect("valid static verification fixture"))
                .is_err()
        );
        value = serde_json::to_value(receipt()).expect("valid static verification fixture");
        value["verifier"] = serde_json::json!("../forged");
        assert!(
            parse_receipt(&serde_json::to_vec(&value).expect("valid static verification fixture"))
                .is_err()
        );
        value = serde_json::to_value(receipt()).expect("valid static verification fixture");
        value["unrecognized"] = serde_json::json!(true);
        assert!(
            parse_receipt(&serde_json::to_vec(&value).expect("valid static verification fixture"))
                .is_err()
        );
        assert!(parse_receipt(&vec![b' '; MAX_RECEIPT_BYTES + 1]).is_err());
        value = serde_json::to_value(receipt()).expect("valid static verification fixture");
        value["checks"] = serde_json::Value::Array(
            (0..=MAX_RECEIPT_CHECKS)
                .map(|_| {
                    serde_json::to_value(receipt().checks[0].clone())
                        .expect("valid static verification fixture")
                })
                .collect(),
        );
        assert!(
            parse_receipt(&serde_json::to_vec(&value).expect("valid static verification fixture"))
                .is_err()
        );
    }

    #[test]
    fn check_set_must_be_complete_unique_ordered_and_exact_argv() {
        let contract = contract();
        let expected = inputs();
        let mut changed = receipt();
        changed.checks.pop();
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks[1] = changed.checks[0].clone();
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks.swap(0, 1);
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks[0].check.args[0] = "build".into();
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks[0].check.id = "renamed".into();
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks[0].check.program = "sh".into();
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks[0].check.cwd = "elsewhere".into();
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks[0].check.timeout_secs += 1;
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
        changed = receipt();
        changed.checks.push(changed.checks[0].clone());
        assert!(validate_receipt(&contract, &expected, &policy(), &changed).is_err());
    }

    #[test]
    fn malformed_or_missing_digests_fail_instead_of_passing() {
        let contract = contract();
        let expected = inputs();
        let mut malformed = receipt();
        malformed.inputs.source_tree_sha = "deadbeef".into();
        assert!(validate_receipt(&contract, &expected, &policy(), &malformed).is_err());
        malformed = receipt();
        malformed.inputs.protected_oracle_digest.clear();
        assert!(validate_receipt(&contract, &expected, &policy(), &malformed).is_err());
        malformed = receipt();
        malformed.checks[0].stdout_digest = "sha256:ABC".into();
        assert!(validate_receipt(&contract, &expected, &policy(), &malformed).is_err());
        malformed = receipt();
        malformed.inputs.source_commit_sha = "a".repeat(39);
        assert!(validate_receipt(&contract, &expected, &policy(), &malformed).is_err());
    }

    #[test]
    fn unknown_and_nonzero_exits_do_not_pass_and_evidence_changes_digest() {
        let contract = contract();
        let expected = inputs();
        let baseline = receipt();
        let digest_before = validate_receipt(&contract, &expected, &policy(), &baseline)
            .expect("valid static verification fixture")
            .receipt_digest;
        let mut changed = baseline.clone();
        changed.checks[0].exit_code = None;
        assert!(
            !validate_receipt(&contract, &expected, &policy(), &changed)
                .expect("valid static verification fixture")
                .all_checks_passed
        );
        changed.checks[0].exit_code = Some(9);
        assert!(
            !validate_receipt(&contract, &expected, &policy(), &changed)
                .expect("valid static verification fixture")
                .all_checks_passed
        );
        changed = baseline;
        changed.checks[0].stdout_digest = digest('f');
        let digest_after = validate_receipt(&contract, &expected, &policy(), &changed)
            .expect("valid static verification fixture")
            .receipt_digest;
        assert_ne!(digest_before, digest_after);
    }

    #[test]
    fn attempts_candidates_policy_and_verifier_cannot_be_substituted() {
        for field in ["attempt", "candidate", "policy"] {
            let mut changed = receipt();
            match field {
                "attempt" => {
                    changed.inputs.attempt_id =
                        AttemptId::new("other-attempt").expect("fixture attempt")
                }
                "candidate" => changed.inputs.candidate_ref = "candidate:replacement".into(),
                _ => changed.inputs.verification_policy_digest = digest('f'),
            }
            assert!(validate_receipt(&contract(), &inputs(), &policy(), &changed).is_err());
        }
        let mut changed = receipt();
        changed.verifier = principal("unapproved-verifier");
        assert!(validate_receipt(&contract(), &inputs(), &policy(), &changed).is_err());
        let mut changed_policy = policy();
        changed_policy.required_negative_checks[0].fixture_digest = digest('f');
        assert!(validate_receipt(&contract(), &inputs(), &changed_policy, &receipt()).is_err());
        let mut changed_inputs = inputs();
        changed_inputs.source_commit_sha = "0".repeat(40);
        assert!(validate_receipt(&contract(), &changed_inputs, &policy(), &receipt()).is_err());
        changed_inputs = inputs();
        changed_inputs.source_tree_sha = "3".repeat(64);
        assert!(validate_receipt(&contract(), &changed_inputs, &policy(), &receipt()).is_err());
    }

    #[test]
    fn negative_checks_are_required_exact_and_use_pinned_expected_exit_not_zero_heuristics() {
        let base = receipt();
        assert!(
            validate_receipt(&contract(), &inputs(), &policy(), &base)
                .expect("all exits match pinned policy")
                .all_checks_passed()
        );
        for exit in [None, Some(0), Some(2)] {
            let mut wrong = base.clone();
            wrong.negative_checks[0].exit_code = exit;
            assert!(
                !validate_receipt(&contract(), &inputs(), &policy(), &wrong)
                    .expect("failure is retained as a consistent non-passing claim")
                    .all_checks_passed()
            );
        }
        let mut missing = base.clone();
        missing.negative_checks.clear();
        assert!(validate_receipt(&contract(), &inputs(), &policy(), &missing).is_err());
        let mut wrong_argv = base.clone();
        wrong_argv.negative_checks[0]
            .check
            .args
            .push("--skip".into());
        assert!(validate_receipt(&contract(), &inputs(), &policy(), &wrong_argv).is_err());
        let mut invalid_policy = policy();
        invalid_policy.required_negative_checks.clear();
        assert!(validate_receipt(&contract(), &inputs(), &invalid_policy, &base).is_err());
        invalid_policy = policy();
        invalid_policy
            .required_negative_checks
            .push(invalid_policy.required_negative_checks[0].clone());
        assert!(validate_receipt(&contract(), &inputs(), &invalid_policy, &base).is_err());
        let mut value = serde_json::to_value(base).expect("fixture JSON");
        value
            .as_object_mut()
            .expect("fixture object")
            .remove("negative_checks");
        assert!(parse_receipt(&serde_json::to_vec(&value).expect("fixture bytes")).is_err());
    }

    #[test]
    fn negative_and_contract_check_ids_cannot_overlap_and_output_identity_cannot_change() {
        let mut overlapping_policy = policy();
        overlapping_policy.required_negative_checks[0].check.id = "unit".into();
        let mut expected = inputs();
        expected.verification_policy_digest =
            crate::content_digest(&overlapping_policy).expect("fixture policy digest");
        let mut changed = receipt();
        changed.inputs = expected.clone();
        changed.negative_checks[0].check.id = "unit".into();
        assert!(validate_receipt(&contract(), &expected, &overlapping_policy, &changed).is_err());
        let expected = inputs();
        let first = validate_receipt(&contract(), &expected, &policy(), &receipt())
            .expect("fixture receipt");
        let mut changed = receipt();
        changed.negative_checks[0].stderr_digest = digest('f');
        let second = validate_receipt(&contract(), &expected, &policy(), &changed)
            .expect("changed output evidence");
        assert_ne!(first.receipt_digest(), second.receipt_digest());
    }

    #[test]
    fn typed_receipts_and_policy_cannot_bypass_byte_argument_count_or_timeout_limits() {
        let mut oversized = receipt();
        let mut matching_contract = contract();
        for result in &mut oversized.checks {
            result.check.args = vec!["x".repeat(MAX_CHECK_ARG_BYTES); MAX_CHECK_ARGS];
        }
        matching_contract.checks = oversized
            .checks
            .iter()
            .map(|result| result.check.clone())
            .collect();
        let mut matching_inputs = inputs();
        matching_inputs.contract_digest =
            crate::content_digest(&matching_contract).expect("large fixture digest");
        oversized.inputs = matching_inputs.clone();
        assert!(
            validate_receipt(&matching_contract, &matching_inputs, &policy(), &oversized)
                .expect_err("matching typed data still has a byte bound")
                .0
                .contains("exceeds 1 MiB")
        );
        for mutation in ["args", "argument", "timeout", "identity", "exit"] {
            let mut changed = receipt();
            match mutation {
                "args" => changed.checks[0].check.args = vec!["x".into(); MAX_CHECK_ARGS + 1],
                "argument" => {
                    changed.checks[0].check.args = vec!["x".repeat(MAX_CHECK_ARG_BYTES + 1)]
                }
                "timeout" => changed.checks[0].check.timeout_secs = MAX_CHECK_TIMEOUT_SECS + 1,
                "identity" => changed.receipt_id = "receipt\nforged".into(),
                _ => changed.checks[0].exit_code = Some(-9),
            }
            if matches!(mutation, "args" | "argument" | "timeout") {
                let mut matching_contract = contract();
                matching_contract.checks = changed
                    .checks
                    .iter()
                    .map(|result| result.check.clone())
                    .collect();
                let mut matching_inputs = inputs();
                matching_inputs.contract_digest =
                    crate::content_digest(&matching_contract).expect("bounded fixture digest");
                changed.inputs = matching_inputs.clone();
                assert!(
                    validate_receipt(&matching_contract, &matching_inputs, &policy(), &changed)
                        .expect_err("matching checks must respect shape bounds")
                        .0
                        .contains("validated bounds")
                );
            } else {
                assert!(validate_receipt(&contract(), &inputs(), &policy(), &changed).is_err());
            }
        }
        let mut oversized_policy = policy();
        let one = oversized_policy.required_negative_checks[0].clone();
        oversized_policy.required_negative_checks = (0..3)
            .map(|index| {
                let mut negative = one.clone();
                negative.check.id = format!("negative-{index}");
                negative.check.args = vec!["x".repeat(MAX_CHECK_ARG_BYTES); MAX_CHECK_ARGS];
                negative
            })
            .collect();
        let mut matching_inputs = inputs();
        matching_inputs.verification_policy_digest =
            crate::content_digest(&oversized_policy).expect("large policy digest");
        let mut matching_receipt = receipt();
        matching_receipt.inputs = matching_inputs.clone();
        matching_receipt.negative_checks = oversized_policy
            .required_negative_checks
            .iter()
            .map(|required| CheckResult {
                check: required.check.clone(),
                exit_code: Some(required.expected_exit_code),
                stdout_digest: digest('d'),
                stderr_digest: digest('e'),
            })
            .collect();
        assert!(
            validate_receipt(
                &contract(),
                &matching_inputs,
                &oversized_policy,
                &matching_receipt
            )
            .expect_err("matching policy is still bounded")
            .0
            .contains("exceeds 1 MiB")
        );
        let mut oversized_contract = contract();
        oversized_contract.objective = "x".repeat(16_385);
        assert!(validate_receipt(&oversized_contract, &inputs(), &policy(), &receipt()).is_err());
    }

    #[test]
    fn bounded_registry_replays_exact_receipts_rejects_collisions_and_never_evicts() {
        assert!(ReceiptRegistry::new(0).is_err());
        assert!(ReceiptRegistry::new(MAX_RETAINED_RECEIPTS + 1).is_err());
        let mut registry = ReceiptRegistry::new(1).expect("bounded fixture registry");
        let first = registry
            .validate_and_record(&contract(), &inputs(), &policy(), &receipt())
            .expect("initial receipt admitted");
        assert!(!first.replayed);
        assert_eq!(registry.retained_receipts(), 1);
        let replay = registry
            .validate_and_record(&contract(), &inputs(), &policy(), &receipt())
            .expect("exact duplicate remains replayable at capacity");
        assert!(replay.replayed);
        assert_eq!(replay.result, first.result);
        let mut changed = receipt();
        changed.checks[0].stdout_digest = digest('f');
        assert!(
            registry
                .validate_and_record(&contract(), &inputs(), &policy(), &changed)
                .expect_err("same identity cannot change output")
                .0
                .contains("different content")
        );
        changed = receipt();
        changed.receipt_id = "receipt-2".into();
        assert!(
            registry
                .validate_and_record(&contract(), &inputs(), &policy(), &changed)
                .expect_err("new input is retained upstream at capacity")
                .0
                .contains("history full")
        );
        assert_eq!(registry.retained_receipts(), 1);
        assert!(
            registry
                .validate_and_record(&contract(), &inputs(), &policy(), &receipt())
                .expect("old identity was never evicted")
                .replayed
        );
    }

    #[test]
    fn untrusted_trust_flags_are_rejected_and_parse_errors_do_not_echo_source_text() {
        for field in [
            "acceptance_granted",
            "verifier_authenticated",
            "execution_qualified",
        ] {
            let mut value = serde_json::to_value(receipt()).expect("fixture JSON");
            value[field] = serde_json::json!(true);
            let error = parse_receipt(&serde_json::to_vec(&value).expect("fixture bytes"))
                .expect_err("untrusted claims cannot add validator flags");
            assert!(!error.0.contains(field));
        }
        let private_field = "private-source-text-do-not-echo";
        let mut value = serde_json::to_value(receipt()).expect("fixture JSON");
        value[private_field] = serde_json::json!(true);
        let error = parse_receipt(&serde_json::to_vec(&value).expect("fixture bytes"))
            .expect_err("unknown field rejected");
        assert!(!error.0.contains(private_field));
        assert!(error.0.len() < 128);
    }
}
