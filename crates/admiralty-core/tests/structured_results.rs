use admiralty_core::coordination::envelope::{ContextEpoch, Sha256Digest};
use admiralty_core::coordination::tool_results::*;
use admiralty_core::economics::{DecimalU64, DecimalU128};
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CONTRACT_VERSION, CheckSpec, PrincipalId, ProjectId, RootId,
    TaskContract, TaskId, content_digest,
};
use serde_json::{Value, json};

const COMMIT_A: &str = "1111111111111111111111111111111111111111";
const TREE_A: &str = "2222222222222222222222222222222222222222";
const COMMIT_B: &str = "3333333333333333333333333333333333333333";
const TREE_B: &str = "4444444444444444444444444444444444444444";

fn contract_fixture() -> TaskContract {
    TaskContract {
        schema_version: CONTRACT_VERSION,
        project_id: ProjectId::new("project-alpha").expect("valid project id"),
        root_id: RootId::new("root-p06").expect("valid root id"),
        task_id: TaskId::new("TASK-P06-STRUCTURED").expect("valid task id"),
        revision: 3,
        title: "Structured tool/test-result summaries".into(),
        objective: "Do not truncate protected facts; keep latency <= 5 ms and payload <= 64 kib without zero-filling unknown values".into(),
        non_goals: vec!["Do not claim tokenizer savings from bytes alone".into()],
        owned_paths: vec![
            "crates/admiralty-core/src/coordination/tool_results.rs".into(),
            "crates/admiralty-core/tests/structured_results.rs".into(),
        ],
        forbidden_paths: vec!["crates/admiralty-store/src/lib.rs".into()],
        read_first: vec!["docs/plans/admiralty-v2/MASTER_PLAN.md".into()],
        dependencies: vec![TaskId::new("TASK-P02").expect("valid dep id")],
        criteria: vec![
            AcceptanceCriterion {
                id: "C1".into(),
                text: "Never collapse intervening passes; keep latency <= 5 ms and payload <= 64 kib without dropping negations".into(),
                check_ids: vec!["check-unit".into(), "check-integration".into()],
            },
        ],
        checks: vec![
            CheckSpec {
                id: "check-unit".into(),
                program: "cargo".into(),
                args: vec![
                    "test".into(),
                    "-p".into(),
                    "admiralty-core".into(),
                    "--test".into(),
                    "structured_results".into(),
                    "--locked".into(),
                ],
                cwd: ".".into(),
                timeout_secs: 120,
            },
            CheckSpec {
                id: "check-integration".into(),
                program: "cargo".into(),
                args: vec![
                    "test".into(),
                    "-p".into(),
                    "admiralty-testkit".into(),
                    "--test".into(),
                    "context_fidelity".into(),
                    "--locked".into(),
                ],
                cwd: ".".into(),
                timeout_secs: 180,
            },
        ],
        authority_ref: "policy:context-compiler:v3".into(),
        max_attempts: 3,
        simulated: false,
    }
}

fn sample_range(seed: &[u8], total: u64, offset: u64, length: u64) -> ArtifactByteRangeRef {
    ArtifactByteRangeRef {
        artifact_digest: artifact_bytes_digest(seed),
        artifact_size_bytes: DecimalU64(total),
        offset_bytes: DecimalU64(offset),
        length_bytes: DecimalU64(length),
    }
}

fn observation_fixture(
    seq: u64,
    transition: DiagnosticTransition,
    fingerprint: &str,
    excerpt: &str,
    offset: u64,
) -> DiagnosticObservation {
    DiagnosticObservation {
        sequence: DecimalU64(seq),
        attempt_id: AttemptId::new("attempt-3").expect("valid attempt id"),
        candidate_ref: "refs/candidates/attempt-3".into(),
        source_commit_sha: COMMIT_A.into(),
        check_id: "check-unit".into(),
        transition,
        fingerprint: fingerprint.into(),
        assertion_or_code: Some("assert_eq!(latency_ms <= 5, true)".into()),
        file_path: Some("crates/admiralty-core/src/coordination/tool_results.rs".into()),
        line_number: Some(DecimalU64(142)),
        environment: "linux-x86_64-rustc-stable".into(),
        excerpt: excerpt.into(),
        raw_range: sample_range(b"raw stderr log payload", 4096, offset, 128),
    }
}

fn facts_fixture() -> StructuredResultFacts {
    let contract = contract_fixture();
    let contract_digest =
        Sha256Digest::new(content_digest(&contract).expect("contract digest")).expect("sha256");
    let attempt_id = AttemptId::new("attempt-3").expect("valid attempt id");
    let candidate_ref = "refs/candidates/attempt-3";

    let observations = vec![
        observation_fixture(
            1,
            DiagnosticTransition::TestAssertionFailure,
            "fp:latency-bound",
            "assertion failed: latency 9 ms exceeds <= 5 ms threshold; do not ignore",
            0,
        ),
        observation_fixture(
            2,
            DiagnosticTransition::TestAssertionFailure,
            "fp:latency-bound",
            "assertion failed: latency 9 ms exceeds <= 5 ms threshold; do not ignore",
            128,
        ),
    ];
    let grouped = group_diagnostic_observations(
        &attempt_id,
        candidate_ref,
        COMMIT_A,
        "check-unit",
        &observations,
    )
    .expect("valid diagnostic grouping");

    StructuredResultFacts {
        identity: StructuredResultIdentity {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            contract_revision: contract.revision,
            contract_digest,
            authority_ref: contract.authority_ref.clone(),
            attempt_id: attempt_id.clone(),
            candidate_ref: candidate_ref.into(),
            source_commit_sha: COMMIT_A.into(),
            source_tree_sha: TREE_A.into(),
            environment_fingerprint: "linux-x86_64-cargo-locked-j1".into(),
        },
        must_carry_constraints: vec![ProtectedConstraintSummary {
            criterion_id: "C1".into(),
            summary_text: "Do not truncate protected facts; never collapse intervening passes; keep latency <= 5 ms and payload <= 64 kib without zero-filling unknown values".into(),
            required_negations: vec!["not".into(), "never".into(), "without".into(), "<=".into()],
            required_units: vec!["ms".into(), "kib".into()],
        }],
        checks: vec![
            CheckResultSummary {
                check: contract.checks[0].clone(),
                provenance: ResultProvenance::IndependentVerifier {
                    receipt_id: "receipt-check-unit-3".into(),
                    receipt_digest: artifact_bytes_digest(b"verifier-receipt-bytes"),
                    verifier: PrincipalId::new("verifier-node-1").expect("valid verifier"),
                    trusted_execution_proof: false,
                },
                status: CheckOutcomeStatus::Failed {
                    exit_code: 101,
                    failed_criterion_id: "C1".into(),
                    duration: MeasurementObservation::Measured {
                        metric: "wall_clock_duration".into(),
                        value: DecimalU128(412),
                        unit: "ms".into(),
                    },
                },
                stdout_log: sample_range(b"stdout bytes", 2048, 0, 512),
                stderr_log: sample_range(b"raw stderr log payload", 4096, 0, 256),
                diagnostics: grouped,
                measured_counters: vec![
                    MeasurementObservation::Measured {
                        metric: "executed_assertions".into(),
                        value: DecimalU128(9_007_199_254_740_993),
                        unit: "tokens".into(),
                    },
                    MeasurementObservation::Absent {
                        metric: "provider_hidden_context_floor".into(),
                        reason: "opaque CLI does not expose internal prompt floor".into(),
                    },
                ],
                uncertainty: vec![
                    "Verifier observation is unauthenticated until Store custody check".into(),
                ],
            },
            CheckResultSummary {
                check: contract.checks[1].clone(),
                provenance: ResultProvenance::UntrustedCallerClaim {
                    claimant: PrincipalId::new("worker-2").expect("valid claimant"),
                    claim_ref: "claim:worker-2:attempt-3".into(),
                    trusted_execution_proof: false,
                },
                status: CheckOutcomeStatus::Unknown {
                    reason: "Caller claim only; independent verifier has not executed check-integration".into(),
                },
                stdout_log: sample_range(b"claimed stdout", 1024, 0, 256),
                stderr_log: sample_range(b"claimed stderr", 1024, 256, 256),
                diagnostics: Vec::new(),
                measured_counters: vec![MeasurementObservation::Absent {
                    metric: "wall_clock_duration".into(),
                    reason: "not independently measured".into(),
                }],
                uncertainty: vec!["Caller claim never becomes trusted execution evidence".into()],
            },
        ],
        failed_approaches: vec![FailedApproachRecord {
            approach_id: "approach-1".into(),
            attempt_id,
            candidate_ref: candidate_ref.into(),
            source_commit_sha: COMMIT_A.into(),
            source_tree_sha: TREE_A.into(),
            hypothesis: "Coalescing all matching fingerprints across the whole file might be safe".into(),
            hypothesis_status: HypothesisStatus::TentativeUnverified,
            action_taken: "Grouped by fingerprint globally without checking intervening pass transitions".into(),
            check_id: "check-unit".into(),
            outcome: CheckOutcomeStatus::Failed {
                exit_code: 101,
                failed_criterion_id: "C1".into(),
                duration: MeasurementObservation::Measured {
                    metric: "wall_clock_duration".into(),
                    value: DecimalU128(412),
                    unit: "ms".into(),
                },
            },
            raw_evidence_refs: vec![sample_range(b"raw stderr log payload", 4096, 0, 256)],
            negative_evidence: vec![
                "Global fingerprint grouping hid an intervening pass between sequence 2 and sequence 4".into(),
            ],
        }],
        uncertainty: vec![
            "Raw log byte ranges require a separately authorized custody read".into(),
        ],
        next_action: "Preserve intervening state transitions and resubmit candidate within write scope; do not escalate model or claim acceptance".into(),
        contract,
    }
}

#[test]
fn round_trip_render_and_decode_preserves_exact_protected_facts_and_false_authority_flags() {
    let facts = facts_fixture();
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Retained {
        baseline_digest: artifact_bytes_digest(b"retained-baseline-v1"),
        context_epoch: epoch.clone(),
    };

    let first = compile_structured_results(&facts, &epoch, &baseline, false, 16_384)
        .expect("compile succeeds");
    let second = compile_structured_results(&facts, &epoch, &baseline, false, 16_384)
        .expect("repeat compile succeeds");
    assert_eq!(first, second);
    assert!(!first.fresh_capsule_requested);
    assert!(!first.authority_granted);
    assert!(!first.execution_evidence_trusted);
    assert!(!first.artifact_custody_verified);
    assert!(!first.retry_or_escalation_authorized);
    assert!(!first.token_savings_claimed);
    assert_eq!(
        first.tokenizer_basis,
        "utf8_bytes_div4_estimate_no_savings_claim"
    );

    let decoded = decode_structured_results(
        first.text.as_bytes(),
        &ExpectedStructuredResultBindings {
            identity: &facts.identity,
            context_epoch: &epoch,
            baseline: &baseline,
            facts_digest: &first.facts_digest,
        },
    )
    .expect("decode with exact retained bindings succeeds");
    assert_eq!(decoded, facts);
}

#[test]
fn omitted_negation_unit_authority_or_revision_fails_closed() {
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    // 1. Omitted negation in constraint summary text
    let mut missing_negation = facts_fixture();
    missing_negation.must_carry_constraints[0].summary_text =
        "Truncate protected facts; collapse intervening passes; keep latency 5 ms and payload 64 kib"
            .into();
    assert_eq!(
        compile_structured_results(&missing_negation, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::ProtectedFactOmitted(
            "required negation from contract objective or criterion was omitted"
        ))
    );

    // 2. Omitted unit in constraint summary text
    let mut missing_unit = facts_fixture();
    missing_unit.must_carry_constraints[0].summary_text =
        "Do not truncate protected facts; never collapse intervening passes; keep latency <= 5 and payload <= 64 without zero-filling unknown values".into();
    assert_eq!(
        compile_structured_results(&missing_unit, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::ProtectedFactOmitted(
            "required unit from contract objective or criterion was omitted"
        ))
    );

    // 3. Mismatched or zero contract revision
    let mut wrong_revision = facts_fixture();
    wrong_revision.identity.contract_revision = 0;
    assert_eq!(
        compile_structured_results(&wrong_revision, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::ProtectedFactOmitted(
            "contract_revision must be nonzero"
        ))
    );
    wrong_revision.identity.contract_revision = 4;
    assert_eq!(
        compile_structured_results(&wrong_revision, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::ProtectedFactOmitted(
            "identity contract_revision differs from TaskContract revision"
        ))
    );

    // 4. Altered or empty authority_ref
    let mut wrong_authority = facts_fixture();
    wrong_authority.identity.authority_ref = "forged:authority".into();
    assert_eq!(
        compile_structured_results(&wrong_authority, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::ProtectedFactOmitted(
            "identity authority_ref differs from TaskContract authority_ref"
        ))
    );
}

#[test]
fn different_source_commit_tree_attempt_candidate_or_contract_digest_is_rejected() {
    let facts = facts_fixture();
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;
    let capsule =
        compile_structured_results(&facts, &epoch, &baseline, false, 16_384).expect("capsule");

    for mutation in 0..6 {
        let mut wrong_identity = facts.identity.clone();
        match mutation {
            0 => wrong_identity.project_id = ProjectId::new("foreign-project").expect("id"),
            1 => wrong_identity.task_id = TaskId::new("FOREIGN-TASK").expect("id"),
            2 => wrong_identity.attempt_id = AttemptId::new("attempt-99").expect("id"),
            3 => wrong_identity.candidate_ref = "refs/candidates/attempt-99".into(),
            4 => wrong_identity.source_commit_sha = COMMIT_B.into(),
            _ => wrong_identity.source_tree_sha = TREE_B.into(),
        }
        assert_eq!(
            decode_structured_results(
                capsule.text.as_bytes(),
                &ExpectedStructuredResultBindings {
                    identity: &wrong_identity,
                    context_epoch: &epoch,
                    baseline: &baseline,
                    facts_digest: &capsule.facts_digest,
                },
            ),
            Err(StructuredResultError::IncompatibleSourceIdentity(
                "decoded facts identity differs from expected identity"
            )),
            "identity mutation {mutation} must fail closed"
        );
    }

    let mut wrong_contract_digest = facts;
    wrong_contract_digest.identity.contract_digest = artifact_bytes_digest(b"different-contract");
    assert_eq!(
        compile_structured_results(&wrong_contract_digest, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::IncompatibleSourceIdentity(
            "identity contract_digest differs from TaskContract digest"
        ))
    );
}

#[test]
fn repeated_diagnostics_coalesce_losslessly_while_intervening_transitions_never_collapse() {
    let attempt_id = AttemptId::new("attempt-3").expect("valid attempt id");
    let candidate_ref = "refs/candidates/attempt-3";
    let observations = vec![
        observation_fixture(
            1,
            DiagnosticTransition::CompilerError,
            "fp:E0308",
            "mismatched types: expected u64, found Option<u64>",
            0,
        ),
        observation_fixture(
            2,
            DiagnosticTransition::CompilerError,
            "fp:E0308",
            "mismatched types: expected u64, found Option<u64>",
            100,
        ),
        observation_fixture(
            3,
            DiagnosticTransition::CompilerError,
            "fp:E0308",
            "mismatched types: expected u64, found Option<u64>",
            200,
        ),
        // Intervening pass must NOT be collapsed or allow surrounding E0308 groups to merge!
        observation_fixture(
            4,
            DiagnosticTransition::InterveningPass,
            "fp:subcheck-pass",
            "unit subcheck passed before next failure",
            300,
        ),
        observation_fixture(
            5,
            DiagnosticTransition::CompilerError,
            "fp:E0308",
            "mismatched types: expected u64, found Option<u64>",
            400,
        ),
        observation_fixture(
            6,
            DiagnosticTransition::CompilerError,
            "fp:E0308",
            "mismatched types: expected u64, found Option<u64>",
            500,
        ),
        // Unknown format transition with explicit parser_gap must stay distinct!
        observation_fixture(
            7,
            DiagnosticTransition::UnknownFormat {
                parser_gap: "unrecognized custom runner frame v9".into(),
            },
            "fp:unknown-frame",
            "raw runner frame: [UNPARSED_FRAME_HEADER]",
            600,
        ),
        observation_fixture(
            8,
            DiagnosticTransition::CompilerError,
            "fp:E0308",
            "mismatched types: expected u64, found Option<u64>",
            700,
        ),
    ];

    let groups = group_diagnostic_observations(
        &attempt_id,
        candidate_ref,
        COMMIT_A,
        "check-unit",
        &observations,
    )
    .expect("lossless grouping succeeds");

    assert_eq!(
        groups.len(),
        5,
        "intervening transitions must prevent collapsing"
    );
    assert_eq!(groups[0].repeat_count, DecimalU128(3));
    assert_eq!(groups[0].first_sequence, DecimalU64(1));
    assert_eq!(groups[0].last_sequence, DecimalU64(3));
    assert_eq!(groups[0].occurrences.len(), 3);
    assert_eq!(groups[0].first_occurrence.offset_bytes, DecimalU64(0));
    assert_eq!(groups[0].last_occurrence.offset_bytes, DecimalU64(200));

    assert_eq!(groups[1].transition, DiagnosticTransition::InterveningPass);
    assert_eq!(groups[1].repeat_count, DecimalU128(1));
    assert_eq!(groups[1].first_sequence, DecimalU64(4));

    assert_eq!(groups[2].repeat_count, DecimalU128(2));
    assert_eq!(groups[2].first_sequence, DecimalU64(5));
    assert_eq!(groups[2].last_sequence, DecimalU64(6));
    assert_eq!(groups[2].occurrences.len(), 2);

    assert_eq!(
        groups[3].transition,
        DiagnosticTransition::UnknownFormat {
            parser_gap: "unrecognized custom runner frame v9".into()
        }
    );
    assert_eq!(groups[3].repeat_count, DecimalU128(1));

    assert_eq!(groups[4].repeat_count, DecimalU128(1));
    assert_eq!(groups[4].first_sequence, DecimalU64(8));
}

#[test]
fn grouping_rejects_cross_attempt_cross_candidate_cross_commit_or_non_monotonic_sequences() {
    let attempt_id = AttemptId::new("attempt-3").expect("valid attempt id");
    let candidate_ref = "refs/candidates/attempt-3";

    let mut wrong_attempt =
        observation_fixture(1, DiagnosticTransition::CompilerError, "fp:1", "error 1", 0);
    wrong_attempt.attempt_id = AttemptId::new("attempt-4").expect("valid attempt id");
    assert_eq!(
        group_diagnostic_observations(
            &attempt_id,
            candidate_ref,
            COMMIT_A,
            "check-unit",
            &[wrong_attempt]
        ),
        Err(StructuredResultError::IncompatibleSourceIdentity(
            "cannot group diagnostics across distinct attempts, candidates, commits, or checks"
        ))
    );

    let mut wrong_commit =
        observation_fixture(1, DiagnosticTransition::CompilerError, "fp:1", "error 1", 0);
    wrong_commit.source_commit_sha = COMMIT_B.into();
    assert_eq!(
        group_diagnostic_observations(
            &attempt_id,
            candidate_ref,
            COMMIT_A,
            "check-unit",
            &[wrong_commit]
        ),
        Err(StructuredResultError::IncompatibleSourceIdentity(
            "cannot group diagnostics across distinct attempts, candidates, commits, or checks"
        ))
    );

    let non_monotonic = vec![
        observation_fixture(2, DiagnosticTransition::CompilerError, "fp:1", "error", 0),
        observation_fixture(2, DiagnosticTransition::CompilerError, "fp:1", "error", 128),
    ];
    assert_eq!(
        group_diagnostic_observations(
            &attempt_id,
            candidate_ref,
            COMMIT_A,
            "check-unit",
            &non_monotonic
        ),
        Err(StructuredResultError::InvalidField(
            "diagnostic observations must have strictly increasing sequences"
        ))
    );
}

#[test]
fn timeout_partial_and_unknown_check_outcomes_preserve_absent_measures_without_zero_fill() {
    let mut facts = facts_fixture();
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    facts.checks[0].status = CheckOutcomeStatus::TimedOut {
        timeout_secs: 120,
        observed_duration: MeasurementObservation::Absent {
            metric: "wall_clock_duration".into(),
            reason: "runner killed by kernel deadline before final timer readback".into(),
        },
        partial_output_retained: true,
    };
    facts.checks[1].status = CheckOutcomeStatus::Partial {
        reason: "stderr stream truncated at runner pipe EOF".into(),
        observed_duration: MeasurementObservation::Measured {
            metric: "wall_clock_duration".into(),
            value: DecimalU128(119_950),
            unit: "ms".into(),
        },
    };

    let capsule = compile_structured_results(&facts, &epoch, &baseline, false, 16_384)
        .expect("timeout and partial outcomes compile");
    assert!(capsule.text.contains("\"state\":\"timed_out\""));
    assert!(capsule.text.contains("\"state\":\"partial\""));
    assert!(capsule.text.contains("\"observation\":\"absent\""));

    let decoded = decode_structured_results(
        capsule.text.as_bytes(),
        &ExpectedStructuredResultBindings {
            identity: &facts.identity,
            context_epoch: &epoch,
            baseline: &baseline,
            facts_digest: &capsule.facts_digest,
        },
    )
    .expect("decode timeout/partial");
    assert_eq!(decoded, facts);

    // Zero timeout_secs or empty unit/reason must fail
    facts.checks[0].status = CheckOutcomeStatus::TimedOut {
        timeout_secs: 0,
        observed_duration: MeasurementObservation::Absent {
            metric: "wall_clock_duration".into(),
            reason: "absent".into(),
        },
        partial_output_retained: false,
    };
    assert_eq!(
        compile_structured_results(&facts, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::InvalidField(
            "timeout_secs must be nonzero"
        ))
    );
}

#[test]
fn huge_counters_preserve_exact_u64_and_u128_decimal_precision_and_reject_overflow() {
    let mut facts = facts_fixture();
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    facts.checks[0].diagnostics[0].repeat_count = DecimalU128(u128::MAX);
    facts.checks[0].diagnostics[0].first_sequence = DecimalU64(u64::MAX - 1);
    facts.checks[0].diagnostics[0].last_sequence = DecimalU64(u64::MAX);
    facts.checks[0].measured_counters[0] = MeasurementObservation::Measured {
        metric: "exact_huge_counter".into(),
        value: DecimalU128(u128::MAX),
        unit: "tokens".into(),
    };

    let capsule = compile_structured_results(&facts, &epoch, &baseline, false, 16_384)
        .expect("huge counters compile");
    assert!(capsule.text.contains(&u128::MAX.to_string()));
    assert!(capsule.text.contains(&u64::MAX.to_string()));

    let decoded = decode_structured_results(
        capsule.text.as_bytes(),
        &ExpectedStructuredResultBindings {
            identity: &facts.identity,
            context_epoch: &epoch,
            baseline: &baseline,
            facts_digest: &capsule.facts_digest,
        },
    )
    .expect("huge counters round-trip");
    assert_eq!(
        decoded.checks[0].diagnostics[0].repeat_count,
        DecimalU128(u128::MAX)
    );

    let mut raw: Value = serde_json::from_str(&capsule.text).expect("valid JSON");
    raw["facts"]["checks"][0]["diagnostics"][0]["repeat_count"] =
        json!("340282366920938463463374607431768211456");
    let raw_bytes = serde_json::to_vec(&raw).expect("serialize mutated JSON");
    assert_eq!(
        decode_structured_results(
            &raw_bytes,
            &ExpectedStructuredResultBindings {
                identity: &facts.identity,
                context_epoch: &epoch,
                baseline: &baseline,
                facts_digest: &capsule.facts_digest,
            },
        ),
        Err(StructuredResultError::Json)
    );
}

#[test]
fn malformed_artifact_byte_ranges_git_shas_and_unsafe_paths_are_rejected() {
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    // 1. Byte range offset + length > artifact_size_bytes
    let mut bad_range = facts_fixture();
    bad_range.checks[0].stdout_log = sample_range(b"stdout", 1024, 900, 200);
    assert_eq!(
        compile_structured_results(&bad_range, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::MalformedReference(
            "artifact byte range exceeds artifact_size_bytes"
        ))
    );

    // 2. Byte range u64 addition overflow
    bad_range.checks[0].stdout_log = sample_range(b"stdout", 1024, u64::MAX, 10);
    assert_eq!(
        compile_structured_results(&bad_range, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::MalformedReference(
            "artifact byte range offset + length overflow"
        ))
    );

    // 3. Zero length or oversized artifact
    bad_range.checks[0].stdout_log = sample_range(b"stdout", 1024, 0, 0);
    assert_eq!(
        compile_structured_results(&bad_range, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::MalformedReference(
            "length_bytes must be nonzero"
        ))
    );
    bad_range.checks[0].stdout_log = sample_range(b"stdout", MAX_ARTIFACT_SIZE_BYTES + 1, 0, 10);
    assert_eq!(
        compile_structured_results(&bad_range, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::MalformedReference(
            "artifact_size_bytes must be within 1..=MAX_ARTIFACT_SIZE_BYTES"
        ))
    );

    // 4. Commit SHA == Tree SHA or malformed Git SHA
    let mut bad_git = facts_fixture();
    bad_git.identity.source_tree_sha = COMMIT_A.into();
    assert_eq!(
        compile_structured_results(&bad_git, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::MalformedReference(
            "source_commit_sha and source_tree_sha must be distinct full hex IDs of equal length"
        ))
    );

    // 5. Path traversal in diagnostic file_path
    let mut bad_path = facts_fixture();
    bad_path.checks[0].diagnostics[0].file_path = Some("../secret/key.pem".into());
    assert_eq!(
        compile_structured_results(&bad_path, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::MalformedReference(
            "diagnostic file_path must be a bounded safe relative path"
        ))
    );
}

#[test]
fn total_byte_token_budget_item_occurrence_and_depth_overflow_fail_without_truncating() {
    let facts = facts_fixture();
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    let full =
        compile_structured_results(&facts, &epoch, &baseline, false, 16_384).expect("full capsule");
    let exact_tokens = full.estimated_tokens_div4.as_u64() as usize;
    assert_eq!(
        compile_structured_results(&facts, &epoch, &baseline, false, exact_tokens)
            .expect("exact token budget"),
        full
    );
    assert_eq!(
        compile_structured_results(&facts, &epoch, &baseline, false, exact_tokens - 1),
        Err(StructuredResultError::BoundExceeded)
    );
    assert_eq!(
        compile_structured_results(&facts, &epoch, &baseline, false, 0),
        Err(StructuredResultError::InvalidBudget)
    );
    assert_eq!(
        compile_structured_results(&facts, &epoch, &baseline, false, usize::MAX),
        Err(StructuredResultError::InvalidBudget)
    );

    // Occurrence reference overflow (> MAX_OCCURRENCE_REFS_PER_GROUP)
    let attempt_id = AttemptId::new("attempt-3").expect("id");
    let too_many_occurrences: Vec<_> = (1..=(MAX_OCCURRENCE_REFS_PER_GROUP as u64 + 1))
        .map(|seq| {
            observation_fixture(
                seq,
                DiagnosticTransition::CompilerError,
                "fp:repeat",
                "repeated compiler error",
                seq * 64,
            )
        })
        .collect();
    assert_eq!(
        group_diagnostic_observations(
            &attempt_id,
            "refs/candidates/attempt-3",
            COMMIT_A,
            "check-unit",
            &too_many_occurrences,
        ),
        Err(StructuredResultError::BoundExceeded)
    );

    // Nesting depth overflow before serde allocation
    let deep_json = format!(
        "{}{}{}",
        "[".repeat(MAX_STRUCTURED_RESULT_DEPTH + 1),
        "0",
        "]".repeat(MAX_STRUCTURED_RESULT_DEPTH + 1)
    );
    assert_eq!(
        decode_structured_results(
            deep_json.as_bytes(),
            &ExpectedStructuredResultBindings {
                identity: &facts.identity,
                context_epoch: &epoch,
                baseline: &baseline,
                facts_digest: &full.facts_digest,
            },
        ),
        Err(StructuredResultError::BoundExceeded)
    );
}

#[test]
fn caller_claims_and_wire_flags_never_upgrade_to_trusted_execution_or_savings_authority() {
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    let mut false_claim = facts_fixture();
    false_claim.checks[1].provenance = ResultProvenance::UntrustedCallerClaim {
        claimant: PrincipalId::new("worker-2").expect("id"),
        claim_ref: "claim:worker-2:attempt-3".into(),
        trusted_execution_proof: true,
    };
    assert_eq!(
        compile_structured_results(&false_claim, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::FalseAuthorityClaim(
            "caller claim never becomes trusted execution evidence"
        ))
    );

    let facts = facts_fixture();
    let capsule =
        compile_structured_results(&facts, &epoch, &baseline, false, 16_384).expect("capsule");
    for flag in [
        "authority_granted",
        "execution_evidence_trusted",
        "artifact_custody_verified",
        "retry_or_escalation_authorized",
        "token_savings_claimed",
    ] {
        let mut doc: Value = serde_json::from_str(&capsule.text).expect("valid JSON");
        doc[flag] = json!(true);
        let bytes = serde_json::to_vec(&doc).expect("serialize JSON");
        assert_eq!(
            decode_structured_results(
                &bytes,
                &ExpectedStructuredResultBindings {
                    identity: &facts.identity,
                    context_epoch: &epoch,
                    baseline: &baseline,
                    facts_digest: &capsule.facts_digest,
                },
            ),
            Err(StructuredResultError::FalseAuthorityClaim(
                "wire document asserted a forbidden authority, custody, or token-savings flag"
            )),
            "flag {flag} must be rejected"
        );
    }
}

#[test]
fn compacted_or_lost_baseline_requests_fresh_capsule_and_rejects_silent_delta_continuation() {
    let facts = facts_fixture();
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let compacted = BaselineObservation::CompactedOrLost {
        reason: "provider compacted conversation context after turn 14".into(),
    };

    assert_eq!(
        compile_structured_results(&facts, &epoch, &compacted, false, 16_384),
        Err(StructuredResultError::FreshCapsuleRequired {
            reason: "provider compacted conversation context after turn 14".into(),
        })
    );

    let fresh_capsule = compile_structured_results(&facts, &epoch, &compacted, true, 16_384)
        .expect("explicit fresh capsule after compaction succeeds");
    assert!(fresh_capsule.fresh_capsule_requested);

    let wrong_epoch_baseline = BaselineObservation::Retained {
        baseline_digest: artifact_bytes_digest(b"baseline"),
        context_epoch: ContextEpoch::new("older-epoch").expect("id"),
    };
    assert_eq!(
        compile_structured_results(&facts, &epoch, &wrong_epoch_baseline, true, 16_384),
        Err(StructuredResultError::FreshCapsuleRequired {
            reason: "retained baseline epoch differs from target context_epoch".into(),
        })
    );
}

#[test]
fn failed_approach_records_reject_passing_outcomes_missing_evidence_or_unknown_checks() {
    let epoch = ContextEpoch::new("epoch-p06-c").expect("valid epoch");
    let baseline = BaselineObservation::Fresh;

    let mut passing_approach = facts_fixture();
    passing_approach.failed_approaches[0].outcome = CheckOutcomeStatus::Passed {
        exit_code: 0,
        duration: MeasurementObservation::Measured {
            metric: "wall_clock_duration".into(),
            value: DecimalU128(10),
            unit: "ms".into(),
        },
    };
    assert_eq!(
        compile_structured_results(&passing_approach, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::InvalidField(
            "failed approach record cannot have a passing check outcome"
        ))
    );

    let mut empty_evidence = facts_fixture();
    empty_evidence.failed_approaches[0]
        .raw_evidence_refs
        .clear();
    assert_eq!(
        compile_structured_results(&empty_evidence, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::BoundExceeded)
    );

    let mut unknown_check = facts_fixture();
    unknown_check.failed_approaches[0].check_id = "undeclared-check".into();
    assert_eq!(
        compile_structured_results(&unknown_check, &epoch, &baseline, false, 16_384),
        Err(StructuredResultError::IncompatibleSourceIdentity(
            "failed approach check_id is not in TaskContract"
        ))
    );
}
