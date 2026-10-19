use admiralty_core::verification::{MAX_RETAINED_RECEIPTS, ReceiptRegistry, parse_receipt};

#[test]
fn receipt_parser_rejects_forged_unknown_fields_and_registry_is_bounded() {
    let forged_claim = br#"{"schema_version":1,"receipt_id":"r1","producer":"provider_claim","verifier":"verifier","inputs":{},"checks":[],"negative_checks":[],"acceptance_granted":true}"#;
    assert!(parse_receipt(forged_claim).is_err());
    assert!(
        parse_receipt(forged_claim).is_err(),
        "replayed forged claims stay rejected"
    );
    assert!(parse_receipt(br#"{"unknown":true}"#).is_err());
    assert!(
        parse_receipt(&[0xff]).is_err(),
        "invalid UTF-8 receipt bytes are rejected"
    );

    let registry = ReceiptRegistry::new(1).expect("bounded receipt history");
    assert_eq!(registry.retained_receipts(), 0);
    assert!(ReceiptRegistry::new(0).is_err());
    assert!(ReceiptRegistry::new(MAX_RETAINED_RECEIPTS).is_ok());
    assert!(ReceiptRegistry::new(MAX_RETAINED_RECEIPTS + 1).is_err());
}

#[test]
fn receipt_parser_rejects_input_over_the_size_limit() {
    use admiralty_core::verification::MAX_RECEIPT_BYTES;

    let oversized = vec![b' '; MAX_RECEIPT_BYTES + 1];
    assert_eq!(
        parse_receipt(&oversized)
            .expect_err("oversized receipt rejected before JSON decoding")
            .0,
        "receipt exceeds 1 MiB",
        "the bound must reject before the JSON parser, not merely reject invalid JSON"
    );
    assert!(parse_receipt(br#"{} trailing"#).is_err());
}

#[cfg(unix)]
mod execution {
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract,
        TaskId, content_digest,
        verification::{
            ReceiptProducer, RequiredNegativeCheck, VerificationInputs, VerificationPolicy,
            validate_receipt,
        },
    };
    use admiralty_runtime::{
        session::SessionHost,
        trusted_checks::{ExecutedChecks, TrustedCheckError, execute_checks},
    };
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::Write,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::Command,
    };

    struct Fixture {
        _root: tempfile::TempDir,
        checkout: PathBuf,
        spool: PathBuf,
        contract: TaskContract,
        policy: VerificationPolicy,
        inputs: VerificationInputs,
    }

    fn private_directory(path: &Path) {
        fs::create_dir(path).expect("create private fixture directory");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private permissions");
    }

    fn git(checkout: &Path, args: &[&str]) -> String {
        let output = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_DATE", "1970-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "1970-01-01T00:00:00Z")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(checkout)
            .output()
            .expect("run fixture Git");
        assert!(
            output.status.success(),
            "fixture Git: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("Git UTF-8 output")
            .trim()
            .to_owned()
    }

    impl Fixture {
        fn new() -> Self {
            let parent =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/exact-candidate-fixtures");
            fs::create_dir_all(&parent).expect("lane-local fixture parent");
            let root = tempfile::Builder::new()
                .prefix("candidate-")
                .tempdir_in(parent)
                .expect("lane-local private fixture");
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
                .expect("private root");
            let checkout = root.path().join("checkout");
            let spool = root.path().join("spool");
            private_directory(&checkout);
            private_directory(&spool);
            git(&checkout, &["init", "--quiet"]);
            fs::write(checkout.join("README.md"), b"exact candidate fixture\n")
                .expect("fixture source");
            git(&checkout, &["add", "README.md"]);
            git(&checkout, &["commit", "--quiet", "-m", "fixture"]);
            let checkout = checkout.canonicalize().expect("canonical checkout");
            let contract = TaskContract {
                schema_version: 1,
                project_id: ProjectId::new("exact-project").expect("project"),
                root_id: RootId::new("exact-root").expect("root"),
                task_id: TaskId::new("exact-task").expect("task"),
                revision: 1,
                title: "Execute the exact candidate".into(),
                objective: "Bind real check execution to source, policy and executable bytes"
                    .into(),
                non_goals: vec![],
                owned_paths: vec!["README.md".into()],
                forbidden_paths: vec![],
                read_first: vec![],
                dependencies: vec![],
                criteria: vec![AcceptanceCriterion {
                    id: "real-execution".into(),
                    text: "The pinned check actually executes".into(),
                    check_ids: vec!["candidate-head".into()],
                }],
                checks: vec![CheckSpec {
                    id: "candidate-head".into(),
                    program: "/usr/bin/git".into(),
                    args: vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
                    cwd: ".".into(),
                    timeout_secs: 3,
                }],
                authority_ref: "authority:exact-candidate".into(),
                max_attempts: 1,
                simulated: false,
            };
            let policy = VerificationPolicy {
                schema_version: 1,
                verifier: PrincipalId::new("independent-verifier").expect("verifier"),
                protected_oracle_digest: content_digest(&"oracle-v1").expect("oracle digest"),
                required_negative_checks: vec![RequiredNegativeCheck {
                    check: CheckSpec {
                        id: "missing-object".into(),
                        program: "/usr/bin/git".into(),
                        args: vec![
                            "cat-file".into(),
                            "-e".into(),
                            "ffffffffffffffffffffffffffffffffffffffff^{commit}".into(),
                        ],
                        cwd: ".".into(),
                        timeout_secs: 3,
                    },
                    expected_exit_code: 128,
                    fixture_digest: content_digest(&"missing-object-v1").expect("fixture digest"),
                }],
            };
            let commit = git(&checkout, &["rev-parse", "HEAD"]);
            let tree = git(&checkout, &["rev-parse", "HEAD^{tree}"]);
            let inputs = VerificationInputs {
                project_id: contract.project_id.clone(),
                root_id: contract.root_id.clone(),
                task_id: contract.task_id.clone(),
                attempt_id: AttemptId::new("exact-attempt").expect("attempt"),
                candidate_ref: "candidate:exact-fixture".into(),
                contract_revision: contract.revision,
                contract_digest: content_digest(&contract).expect("contract digest"),
                verification_policy_digest: content_digest(&policy).expect("policy digest"),
                source_artifact_digest: content_digest(&("fixture-artifact", &commit, &tree))
                    .expect("artifact digest"),
                source_commit_sha: commit,
                source_tree_sha: tree,
                protected_oracle_digest: policy.protected_oracle_digest.clone(),
            };
            Self {
                _root: root,
                checkout,
                spool,
                contract,
                policy,
                inputs,
            }
        }

        fn run(&self, run_id: &str) -> Result<ExecutedChecks, TrustedCheckError> {
            execute_checks(
                &SessionHost::new(),
                &self.inputs,
                &self.contract,
                &self.policy,
                &self.checkout,
                &self.spool,
                run_id,
            )
        }

        fn use_private_tool(&mut self) -> PathBuf {
            let tool = self._root.path().join("check-tool");
            fs::copy("/usr/bin/true", &tool).expect("copy fixture executable");
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
                .expect("tool permissions");
            self.contract.checks[0].program = tool.to_str().expect("tool UTF-8 path").into();
            self.contract.checks[0].args.clear();
            self.inputs.contract_digest = content_digest(&self.contract).expect("contract digest");
            tool
        }
    }

    #[test]
    fn real_argv_produces_reproducible_outputs_and_cache_binding_without_granting_acceptance() {
        let fixture = Fixture::new();
        let first = fixture.run("first").expect("real checks pass");
        let second = fixture.run("second").expect("same exact inputs pass again");
        assert_eq!(first.receipt().checks, second.receipt().checks);
        assert_eq!(
            first.receipt().negative_checks,
            second.receipt().negative_checks
        );
        assert_eq!(first.receipt().checks[0].exit_code, Some(0));
        assert_eq!(first.receipt().negative_checks[0].exit_code, Some(128));
        let actual_stdout = format!("{}\n", fixture.inputs.source_commit_sha);
        assert_eq!(
            first.receipt().checks[0].stdout_digest,
            format!("sha256:{:x}", Sha256::digest(actual_stdout.as_bytes()))
        );
        assert_eq!(first.environment_digest(), second.environment_digest());
        assert_eq!(first.cache_key_digest(), second.cache_key_digest());
        assert_ne!(
            first.execution_digest(),
            second.execution_digest(),
            "execution IDs remain distinct"
        );
        first
            .revalidate_environment(&fixture.contract, &fixture.policy)
            .expect("current environment");
        let result = validate_receipt(
            &fixture.contract,
            &fixture.inputs,
            &fixture.policy,
            first.receipt(),
        )
        .expect("actual receipt remains consistent");
        assert!(!result.acceptance_granted());
        assert!(!result.verifier_authenticated());
    }

    #[test]
    fn wrong_checkout_and_stale_contract_policy_oracle_are_rejected() {
        let fixture = Fixture::new();
        let other = Fixture::new();
        fs::write(other.checkout.join("README.md"), b"another checkout\n")
            .expect("different source");
        git(&other.checkout, &["add", "README.md"]);
        git(&other.checkout, &["commit", "--quiet", "-m", "changed"]);
        assert!(matches!(
            execute_checks(
                &SessionHost::new(),
                &fixture.inputs,
                &fixture.contract,
                &fixture.policy,
                &other.checkout,
                &fixture.spool,
                "wrong-checkout"
            ),
            Err(TrustedCheckError::CandidateDrift)
        ));
        let mut stale_contract = fixture.contract.clone();
        stale_contract.revision += 1;
        let mut stale_policy = fixture.policy.clone();
        stale_policy.verifier = PrincipalId::new("different-verifier").expect("verifier");
        let mut changed_oracle = fixture.policy.clone();
        changed_oracle.protected_oracle_digest =
            content_digest(&"oracle-v2").expect("oracle digest");
        for (contract, policy) in [
            (&stale_contract, &fixture.policy),
            (&fixture.contract, &stale_policy),
            (&fixture.contract, &changed_oracle),
        ] {
            assert!(
                execute_checks(
                    &SessionHost::new(),
                    &fixture.inputs,
                    contract,
                    policy,
                    &fixture.checkout,
                    &fixture.spool,
                    "stale"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn actual_receipt_rejects_provider_claim_and_forged_verifier() {
        let fixture = Fixture::new();
        let checks = fixture.run("issuer").expect("real check proof");
        let mut forged = checks.receipt().clone();
        forged.producer = ReceiptProducer::ProviderClaim;
        assert!(
            validate_receipt(&fixture.contract, &fixture.inputs, &fixture.policy, &forged).is_err()
        );
        forged = checks.receipt().clone();
        forged.verifier = PrincipalId::new("forged-verifier").expect("forged verifier");
        assert!(
            validate_receipt(&fixture.contract, &fixture.inputs, &fixture.policy, &forged).is_err()
        );
    }

    #[test]
    fn binary_change_at_same_path_invalidates_proof_and_cache_even_when_output_matches() {
        let mut fixture = Fixture::new();
        let tool = fixture.use_private_tool();
        let first = fixture.run("original-tool").expect("original executable");
        fs::OpenOptions::new()
            .append(true)
            .open(&tool)
            .expect("open private tool")
            .write_all(b"changed fixture executable bytes")
            .expect("change executable");
        assert!(matches!(
            first.revalidate_environment(&fixture.contract, &fixture.policy),
            Err(TrustedCheckError::EnvironmentDrift)
        ));
        let second = fixture
            .run("changed-tool")
            .expect("ELF trailing data preserves tool behavior");
        assert_eq!(first.receipt().checks, second.receipt().checks);
        assert_ne!(first.environment_digest(), second.environment_digest());
        assert_ne!(first.cache_key_digest(), second.cache_key_digest());
        let mut changed_policy = fixture.policy.clone();
        changed_policy.protected_oracle_digest = content_digest(&"oracle-v2").expect("oracle");
        assert!(
            second
                .revalidate_environment(&fixture.contract, &changed_policy)
                .is_err()
        );
    }

    #[test]
    fn executable_mutation_during_checks_does_not_mint_evidence() {
        let mut fixture = Fixture::new();
        let tool = fixture.use_private_tool();
        let mut unchanged_tool = fixture.contract.checks[0].clone();
        unchanged_tool.id = "tool-after-mutation".into();
        fixture.contract.checks[0].program = "/bin/sh".into();
        fixture.contract.checks[0].args = vec![
            "-c".into(),
            "printf changed >> \"$1\"".into(),
            "fixture".into(),
            tool.to_str().expect("tool path").into(),
        ];
        fixture.contract.criteria[0]
            .check_ids
            .push(unchanged_tool.id.clone());
        fixture.contract.checks.push(unchanged_tool);
        fixture.inputs.contract_digest =
            content_digest(&fixture.contract).expect("contract digest");
        assert!(matches!(
            fixture.run("mutating-tool"),
            Err(TrustedCheckError::EnvironmentDrift)
        ));
    }

    #[test]
    fn per_process_home_and_tmp_are_fresh_with_controlled_locale() {
        let mut fixture = Fixture::new();
        fixture.contract.checks[0].program = "/bin/sh".into();
        fixture.contract.checks[0].args = vec!["-c".into(),
            "test \"$LANG\" = C && test \"$LC_ALL\" = C && test -d \"$HOME\" && test -d \"$TMPDIR\" && test \"$HOME\" != \"$TMPDIR\" && test ! -e \"$HOME/marker\" && touch \"$HOME/marker\"".into()];
        let mut next = fixture.contract.checks[0].clone();
        next.id = "fresh-second-home".into();
        fixture.contract.criteria[0].check_ids.push(next.id.clone());
        fixture.contract.checks.push(next);
        fixture.inputs.contract_digest =
            content_digest(&fixture.contract).expect("contract digest");
        fixture
            .run("isolated-home")
            .expect("each command receives a fresh home and tmp");
    }

    #[test]
    fn oversized_executable_is_rejected_before_launch() {
        let mut fixture = Fixture::new();
        let tool = fixture.use_private_tool();
        fs::OpenOptions::new()
            .write(true)
            .open(tool)
            .expect("private executable")
            .set_len(256 * 1024 * 1024 + 1)
            .expect("sparse oversized executable");
        assert!(matches!(
            fixture.run("oversized-tool"),
            Err(TrustedCheckError::Invalid(
                "executable exceeds hashing bounds"
            ))
        ));
    }
}
