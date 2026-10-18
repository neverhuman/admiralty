//! Round 8 P08/P19 qualification for publishing a Codex-owned worktree candidate.
//! Git and forge effects are confined to temporary, deterministic fixtures.

use std::process::{Command, Output};

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, PublicationIntent,
        PublicationScope, PublicationSnapshot, propose_publication, publication_request_digest,
    },
};
use admiralty_testkit::forge::{
    Fault as ForgeFault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator,
};
use tempfile::tempdir;

fn git(repository: &std::path::Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git fixture command starts");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn text(output: Output) -> String {
    String::from_utf8(output.stdout)
        .expect("git output is utf-8")
        .trim()
        .to_owned()
}

fn commit(repository: &std::path::Path, message: &str, timestamp: &str) -> String {
    git(
        repository,
        &[
            "-c",
            "user.name=Qualification Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "add",
            "--",
            ".",
        ],
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args([
            "-c",
            "user.name=Qualification Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            message,
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", timestamp)
        .env("GIT_COMMITTER_DATE", timestamp)
        .output()
        .expect("git commit fixture command starts");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    text(git(repository, &["rev-parse", "HEAD"]))
}

fn eligible_intent(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    candidate_head: &str,
) -> PublicationIntent {
    let generation = DecimalU64::new(8);
    let digest = publication_request_digest(scope, snapshot, candidate_head, &generation)
        .expect("fixture request digest");
    let result = propose_publication(scope, snapshot, candidate_head, &digest, &generation);
    assert!(result.eligible, "proposal rejected: {:?}", result.reasons);
    assert!(!result.execution_granted());
    result.intent.expect("eligible proposal returns an intent")
}

#[test]
fn p19_round8_p08_worktree_publication_qualification() {
    let fixture = tempdir().expect("temporary repository root");
    let repository = fixture.path().join("repository");
    let worktree = fixture.path().join("codex-worktree");
    std::fs::create_dir(&repository).expect("repository directory");
    git(&repository, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::create_dir(repository.join("src")).expect("initial source directory");
    std::fs::write(
        repository.join("src/lib.rs"),
        b"pub fn value() -> u8 { 1 }\n",
    )
    .expect("baseline source");
    let base = commit(&repository, "baseline", "2000-01-01T00:00:00Z");

    git(
        &repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "codex/round8-item2",
            worktree.to_str().expect("worktree path is utf-8"),
            &base,
        ],
    );
    std::fs::write(worktree.join("src/lib.rs"), b"pub fn value() -> u8 { 2 }\n")
        .expect("Codex candidate source");
    let candidate = commit(&worktree, "candidate", "2000-01-01T00:00:01Z");

    assert_eq!(text(git(&repository, &["rev-parse", "main"])), base);
    assert_eq!(text(git(&worktree, &["rev-parse", "HEAD"])), candidate);
    assert_eq!(
        text(git(&worktree, &["rev-parse", "HEAD^"])),
        base,
        "candidate is a linear child of the observed baseline"
    );

    let generation = DecimalU64::new(8);
    let scope = PublicationScope {
        project_id: ProjectId::new("round8-p08-p19").expect("project id"),
        root_id: RootId::new("temporary-git-root").expect("root id"),
        task_id: TaskId::new("worktree-publication").expect("task id"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(base.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(base.clone()),
        expected_base: Some(base.clone()),
        required_reviewers: vec!["maintainer".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "maintainer".into(),
            head: base.clone(),
            dismissed: false,
            provenance_ref: "fixture:round8:approval".into(),
        }],
        required_checks: vec!["worktree-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "worktree-integrity".into(),
            head: base.clone(),
            state: CheckState::Passed,
            provenance_ref: "fixture:round8:check".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let intent = eligible_intent(&scope, &snapshot, &candidate);
    assert_eq!(intent.expected_head, base);

    let key = ForgeKey {
        repository: "fixture/round8".into(),
        target: scope.target_ref,
    };
    let request = ForgeRequest {
        id: intent.intent_key,
        key: key.clone(),
        expected_head: base.clone(),
        new_head: candidate.clone(),
    };
    let mut forge = ForgeSimulator::new(1, 1).expect("bounded forge simulator");
    forge
        .seed_ref(key.clone(), base.clone())
        .expect("seed target");
    assert_eq!(
        forge.submit(request, ForgeFault::None),
        Ok(ForgeReply::Observed(ForgeOutcome::Applied {
            previous_head: base.clone(),
            new_head: candidate.clone(),
        }))
    );
    assert_eq!(forge.head(&key), Some(candidate.as_str()));
    assert_eq!(forge.applied_mutations(), 1);
    assert_eq!(text(git(&repository, &["rev-parse", "main"])), base);
}
