//! Offline invariant and process fixtures; no paid provider calls.
pub mod fake_provider;

use admiralty_core::*;

pub fn task_fixture(id: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("demo-harbor").expect("static fixture ID"),
        root_id: RootId::new("root-demo").expect("static fixture ID"),
        task_id: TaskId::new(id).expect("test provides valid ID"),
        revision: 1,
        title: "Demonstrate scoped work".into(),
        objective: "Implement range check; do not change the public API".into(),
        non_goals: vec!["No publication or external effects".into()],
        owned_paths: vec!["src/range.rs".into(), "tests/range.rs".into()],
        forbidden_paths: vec!["policy/**".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "Reject lengths below 5 ms; do not accept unknown as zero".into(),
            check_ids: vec!["range".into()],
        }],
        checks: vec![CheckSpec {
            id: "range".into(),
            program: "cargo".into(),
            args: vec!["test".into(), "range".into()],
            cwd: ".".into(),
            timeout_secs: 60,
        }],
        authority_ref: "demo-candidate-only-v1".into(),
        max_attempts: 2,
        simulated: true,
    }
}
