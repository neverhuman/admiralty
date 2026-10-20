//! Git health publication reuses the single Git observation for this cycle.
use admiralty_core::git_health::{GitHealthReport, GitHealthState, evaluate};
use admiralty_core::views::GitRepository;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHealthAlert {
    pub project_id: String,
    pub fingerprint: String,
    pub observed_utc: String,
    pub state: GitHealthState,
    pub issue_codes: Vec<String>,
}

/// Compare persisted evidence, not wall time. Recurring identical problems
/// remain in the reports without waking a captain on every observation tick.
pub fn collect(
    config: &crate::config::ObserveConfig,
    repositories: &BTreeMap<String, GitRepository>,
    previous: Option<&Value>,
) -> (BTreeMap<String, GitHealthReport>, Vec<GitHealthAlert>) {
    let mut reports = BTreeMap::new();
    let mut alerts = Vec::new();
    for project in &config.projects {
        let Some(repository) = repositories.get(&project.id) else {
            // git::collect retains a marked error for every configured project.
            // Do not invent repository identity or healthy empty facts here.
            continue;
        };
        let policy = project.git_health.as_ref();
        let report = evaluate(
            repository,
            policy.map(|p| p.integration_ref.as_str()),
            policy.map_or(20, |p| p.divergence_threshold),
        );
        let prior = previous.and_then(|v| v["git_health"][&project.id]["fingerprint"].as_str());
        if report.actionable && prior != Some(report.fingerprint.as_str()) {
            let mut issue_codes = report
                .issues
                .iter()
                .map(|i| i.code.clone())
                .collect::<Vec<_>>();
            issue_codes.sort();
            issue_codes.dedup();
            alerts.push(GitHealthAlert {
                project_id: project.id.clone(),
                fingerprint: report.fingerprint.clone(),
                observed_utc: report.observed_utc.clone(),
                state: report.state.clone(),
                issue_codes,
            });
        }
        reports.insert(project.id.clone(), report);
    }
    alerts.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    (reports, alerts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn changed_actionable_report_alerts_once_and_dedup_survives_restart() {
        let config = serde_json::from_value(json!({
            "schema_version":1,"node_id":"fixture","output_dir":"/owned",
            "projects":[{"id":"project","name":"Project","repository":"/repo","tracker":{},
                "git_health":{"integration_ref":"refs/heads/main","divergence_threshold":0}}]
        }))
        .expect("config");
        let mut repositories = BTreeMap::from([(
            "project".into(),
            GitRepository {
                instance_id: "fixture:repository".into(),
                node: "fixture".into(),
                observed_utc: "2026-10-20T00:00:00Z".into(),
                error: Some("unavailable".into()),
                shallow: false,
                truncated: false,
                commits: vec![],
                refs: vec![],
                checkouts: vec![],
            },
        )]);
        let (reports, alerts) = collect(&config, &repositories, None);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].project_id, "project");
        assert_eq!(
            alerts[0].issue_codes,
            vec!["missing_integration_ref", "observation_error"]
        );
        let previous = json!({"git_health":reports});
        repositories.get_mut("project").expect("repo").observed_utc = "2026-10-20T00:01:00Z".into();
        let (reports, alerts) = collect(&config, &repositories, Some(&previous));
        assert!(alerts.is_empty());
        assert!(reports["project"].actionable);
        repositories.get_mut("project").expect("repo").truncated = true;
        assert_eq!(collect(&config, &repositories, Some(&previous)).1.len(), 1);
    }

    #[test]
    fn unconfigured_policy_is_disabled_without_alerts_or_git_access() {
        let config = serde_json::from_value(json!({
            "schema_version":1,"node_id":"fixture","output_dir":"/owned",
            "projects":[{"id":"project","name":"Project","repository":"/does/not/exist","tracker":{}}]
        })).expect("config");
        let repositories = BTreeMap::from([(
            "project".into(),
            GitRepository {
                instance_id: "fixture:repository".into(),
                node: "fixture".into(),
                observed_utc: "2026-10-20T00:00:00Z".into(),
                error: None,
                shallow: false,
                truncated: false,
                commits: vec![],
                refs: vec![],
                checkouts: vec![],
            },
        )]);
        let (reports, alerts) = collect(&config, &repositories, None);
        assert_eq!(reports["project"].state, GitHealthState::Disabled);
        assert!(alerts.is_empty());
    }
}
