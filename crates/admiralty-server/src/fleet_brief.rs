//! Bounded operational projection of a producer feed, never a task authority.
//! Only typed counters and fixed diagnostic codes enter agent context. Original
//! prose, screen tails, commands, paths and Git commit text stay in the source.
use admiralty_core::git_health::{GitHealthReport, GitHealthState};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BRIEF_BYTES: usize = 16 * 1024;
pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROJECTS: usize = 2000;
const MAX_ROWS: usize = 64;
pub const STALE_AFTER_SECONDS: i64 = 180;

#[derive(Clone, Debug, Serialize)]
pub struct Clock {
    pub observed_utc: Option<String>,
    pub age_seconds: Option<i64>,
    pub state: &'static str,
}

fn clock(value: Option<&str>, now: DateTime<Utc>) -> Clock {
    let observed = value.and_then(|s| DateTime::parse_from_rfc3339(s).ok());
    let age = observed.map(|s| now.signed_duration_since(s).num_seconds());
    Clock {
        observed_utc: observed.map(|s| s.to_rfc3339()),
        age_seconds: age,
        state: match age {
            None => "unknown",
            Some(_) if observed.is_some_and(|observed| observed > now) => "future",
            Some(n) if n > STALE_AFTER_SECONDS => "stale",
            _ => "fresh",
        },
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Totals {
    pub projects: usize,
    pub git_review_projects: usize,
    pub git_attention: usize,
    pub git_unknown: usize,
    pub git_disabled: usize,
    pub git_missing: usize,
    pub git_healthy: usize,
    pub git_not_fresh: usize,
    pub dirty_checkouts: usize,
    pub conflicted_checkouts: usize,
    pub unknown_checkouts: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Project {
    pub id: String,
    // Source task counters are decimal strings so JS and agents retain u64s.
    pub total: Option<String>,
    pub done: Option<String>,
    pub blocked: Option<String>,
    pub git_state: &'static str,
    pub git_clock: Clock,
    pub observed_branches: Option<usize>,
    pub diverged_branches: Option<usize>,
    pub dirty_checkouts: Option<usize>,
    pub conflicted_checkouts: Option<usize>,
    pub unknown_checkouts: Option<usize>,
    pub issue_codes: BTreeMap<&'static str, usize>,
}

#[derive(Debug, Serialize)]
pub struct Brief {
    pub schema_version: u32,
    pub read_only: bool,
    pub source_sha256: String,
    pub source_bytes: usize,
    pub stale_after_seconds: i64,
    pub source_clock: Clock,
    pub collector_duration_ms: Option<u64>,
    pub diagnostics: Vec<&'static str>,
    pub totals: Totals,
    pub projects: Vec<Project>,
    pub omitted_projects: usize,
    pub detail_path: &'static str,
}

// Diagnostic vocabulary is controlled here, never copied from imported prose.
fn issue_code(code: &str) -> &'static str {
    match code {
        "merge_conflicts" => "merge_conflicts",
        "integration_worktree_dirty" => "integration_worktree_dirty",
        "detached_work" => "detached_work",
        "diverged_branch" => "diverged_branch",
        "unrelated_branch" => "unrelated_branch",
        "working_state_unknown" => "working_state_unknown",
        "checkout_ancestry_unknown" => "checkout_ancestry_unknown",
        "missing_ancestry" => "missing_ancestry",
        "missing_parents" => "missing_parents",
        "missing_integration_ref" => "missing_integration_ref",
        "invalid_graph" => "invalid_graph",
        "shallow_history" => "shallow_history",
        "truncated_observation" => "truncated_observation",
        "observation_error" => "observation_error",
        _ => "unrecognized_issue",
    }
}

fn counter(value: &Value) -> Option<String> {
    value.as_u64().map(|n| n.to_string())
}

fn identifier(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
    {
        value.to_owned()
    } else {
        format!("source-id-{:x}", Sha256::digest(value.as_bytes()))
    }
}

pub fn compile(bytes: &[u8], now: DateTime<Utc>, budget: usize) -> Result<Vec<u8>, &'static str> {
    if bytes.len() > MAX_SOURCE_BYTES || !(2048..=MAX_BRIEF_BYTES).contains(&budget) {
        return Err("brief input or output budget exceeds its bounds");
    }
    let source: Value =
        serde_json::from_slice(bytes).map_err(|_| "brief source is not valid JSON")?;
    compile_value(&source, bytes, now, budget)
}

pub(crate) fn compile_value(
    source: &Value,
    bytes: &[u8],
    now: DateTime<Utc>,
    budget: usize,
) -> Result<Vec<u8>, &'static str> {
    let projects = source["projects"]
        .as_object()
        .ok_or("brief source projects are missing")?;
    if !source["order"].is_array() || !source["generated_utc"].is_string() {
        return Err("brief source fields are missing");
    }
    let reports = source.get("git_health").and_then(Value::as_object);
    let repositories = source.get("git").and_then(Value::as_object);
    let ids: BTreeSet<&str> = projects
        .keys()
        .map(String::as_str)
        .chain(
            reports
                .into_iter()
                .flat_map(|r| r.keys().map(String::as_str)),
        )
        .chain(
            repositories
                .into_iter()
                .flat_map(|r| r.keys().map(String::as_str)),
        )
        .collect();
    if ids.len() > MAX_PROJECTS {
        return Err("brief source project scope exceeds its bound");
    }
    let mut totals = Totals {
        projects: ids.len(),
        ..Totals::default()
    };
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        let project = &source["projects"][id];
        let raw = reports.and_then(|reports| reports.get(id));
        let parsed =
            raw.and_then(|value| serde_json::from_value::<GitHealthReport>(value.clone()).ok());
        let report = parsed.as_ref().filter(|r| r.schema_version == 1);
        if report.is_some_and(|r| {
            r.issues
                .iter()
                .any(|i| i.severity == GitHealthState::Attention)
        }) {
            totals.git_review_projects += 1;
        }
        let state = match report.map(|r| &r.state) {
            Some(GitHealthState::Healthy) => {
                totals.git_healthy += 1;
                "healthy"
            }
            Some(GitHealthState::Attention) => {
                totals.git_attention += 1;
                "attention"
            }
            Some(GitHealthState::Disabled) => {
                totals.git_disabled += 1;
                "disabled"
            }
            Some(GitHealthState::Unknown) => {
                totals.git_unknown += 1;
                "unknown"
            }
            None if raw.is_some() => {
                totals.git_unknown += 1;
                "invalid"
            }
            None => {
                totals.git_missing += 1;
                "missing"
            }
        };
        let git_clock = clock(report.map(|r| r.observed_utc.as_str()), now);
        if report.is_some() && git_clock.state != "fresh" {
            totals.git_not_fresh += 1;
        }
        let dirty = report.map(|r| r.checkouts.iter().filter(|c| c.dirty == Some(true)).count());
        let conflicted = report.map(|r| {
            r.checkouts
                .iter()
                .filter(|c| c.conflicts.is_some_and(|n| n > 0))
                .count()
        });
        let unknown = report.map(|r| r.checkouts.iter().filter(|c| c.dirty.is_none()).count());
        totals.dirty_checkouts += dirty.unwrap_or(0);
        totals.conflicted_checkouts += conflicted.unwrap_or(0);
        totals.unknown_checkouts += unknown.unwrap_or(0);
        let mut codes = BTreeMap::new();
        for issue in report.into_iter().flat_map(|r| &r.issues) {
            *codes.entry(issue_code(&issue.code)).or_default() += 1;
        }
        rows.push(Project {
            id: identifier(id),
            total: counter(&project["total"]),
            done: counter(&project["done"]),
            blocked: counter(
                project
                    .get("blocked")
                    .unwrap_or(&project["counts"]["blocked"]),
            ),
            git_state: state,
            git_clock,
            observed_branches: report.map(|r| r.branches.len()),
            diverged_branches: report.map(|r| {
                r.branches
                    .iter()
                    .filter(|b| {
                        matches!(
                            b.relation,
                            admiralty_core::git_health::GitHealthRelation::Diverged
                        )
                    })
                    .count()
            }),
            dirty_checkouts: dirty,
            conflicted_checkouts: conflicted,
            unknown_checkouts: unknown,
            issue_codes: codes,
        });
    }
    // Keep problems first when a context budget cannot contain every project.
    rows.sort_by_key(|r| {
        (
            if r.conflicted_checkouts.is_some_and(|n| n > 0) {
                0
            } else if r.git_state == "attention" {
                1
            } else if r.git_clock.state != "fresh"
                || matches!(r.git_state, "unknown" | "invalid" | "missing")
            {
                2
            } else {
                3
            },
            r.id.clone(),
        )
    });
    let mut brief = Brief {
        schema_version: 1,
        read_only: true,
        source_sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
        source_bytes: bytes.len(),
        stale_after_seconds: STALE_AFTER_SECONDS,
        source_clock: clock(source["generated_utc"].as_str(), now),
        collector_duration_ms: source["collector"]["duration_ms"]
            .as_u64()
            .filter(|n| *n <= 9_007_199_254_740_991),
        diagnostics: ["usage_error", "history_error"]
            .into_iter()
            .filter(|key| !source[*key].is_null())
            .collect(),
        omitted_projects: totals.projects,
        totals,
        projects: vec![],
        detail_path: "/v1/legacy",
    };
    // Count exact UTF-8 JSON bytes. The initial omitted count reserves at least
    // as many decimal digits as its eventual value; no partial JSON truncation.
    let mut size = serde_json::to_vec(&brief)
        .map_err(|_| "brief encoding failed")?
        .len();
    for row in rows.into_iter().take(MAX_ROWS) {
        let addition = serde_json::to_vec(&row)
            .map_err(|_| "brief encoding failed")?
            .len()
            + usize::from(!brief.projects.is_empty());
        if size + addition > budget {
            break;
        }
        size += addition;
        brief.projects.push(row);
    }
    brief.omitted_projects -= brief.projects.len();
    let bytes = serde_json::to_vec(&brief).map_err(|_| "brief encoding failed")?;
    if bytes.len() > budget {
        return Err("brief metadata exceeds output budget");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        "2026-10-21T12:00:00Z".parse().expect("clock")
    }
    fn source() -> Value {
        json!({"generated_utc":"2026-10-21T11:00:00Z","order":["a"],
            "projects":{"a":{"total":9007199254740993_u64,"done":2,"captain":{"tail":"SECRET command"}}},
            "git":{"a":{}},"usage_error":"SECRET error","collector":{"duration_ms":42}})
    }

    #[test]
    fn preserves_missing_data_staleness_exact_counters_and_excludes_prose() {
        let bytes =
            compile(&serde_json::to_vec(&source()).expect("json"), now(), 2048).expect("brief");
        let brief: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(brief["source_clock"]["state"], "stale");
        assert_eq!(brief["projects"][0]["total"], "9007199254740993");
        assert!(brief["projects"][0]["blocked"].is_null());
        assert_eq!(brief["totals"]["git_missing"], 1);
        assert_eq!(brief["projects"][0]["git_state"], "missing");
        assert!(!String::from_utf8(bytes).expect("utf8").contains("SECRET"));
    }

    #[test]
    fn exact_byte_budget_retains_full_totals_and_declares_omissions() {
        let mut source = source();
        for n in 0..200 {
            source["projects"][format!("p-{n:03}")] = json!({"done":0});
        }
        let input = serde_json::to_vec(&source).expect("json");
        let bytes = compile(&input, now(), 2048).expect("brief");
        assert!(bytes.len() <= 2048);
        let brief: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(brief["totals"]["projects"], 201);
        assert_eq!(brief["totals"]["git_missing"], 201);
        assert_eq!(
            brief["omitted_projects"].as_u64().expect("count") as usize
                + brief["projects"].as_array().expect("rows").len(),
            201
        );
        assert_eq!(bytes, compile(&input, now(), 2048).expect("deterministic"));
    }
    #[test]
    fn invalid_clocks_and_reports_are_unknown_and_never_healthy() {
        let mut source = source();
        source["generated_utc"] = json!("broken");
        source["git_health"] = json!({"a":{"state":"healthy"}});
        let bytes =
            compile(&serde_json::to_vec(&source).expect("json"), now(), 2048).expect("brief");
        let brief: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(brief["source_clock"]["state"], "unknown");
        assert_eq!(brief["totals"]["git_unknown"], 1);
        assert_eq!(brief["projects"][0]["git_state"], "invalid");
        assert_eq!(clock(Some("2026-10-22T00:00:00Z"), now()).state, "future");
    }

    #[test]
    fn known_conflicts_remain_visible_with_unknown_ancestry_and_limited_rows() {
        use admiralty_core::views::{GitCheckout, GitRepository, GitWorkingState};
        let repository = GitRepository {
            instance_id: "synthetic:repo".into(),
            node: "fixture".into(),
            observed_utc: "2026-10-21T11:59:00Z".into(),
            error: None,
            shallow: false,
            truncated: true,
            commits: vec![],
            refs: vec![],
            checkouts: vec![GitCheckout {
                id: "checkout".into(),
                name: "worker".into(),
                kind: "linked".into(),
                head: None,
                branch: Some("worker".into()),
                detached: false,
                locked: false,
                prunable: false,
                status: Some(GitWorkingState {
                    conflicts: 2,
                    ..Default::default()
                }),
            }],
        };
        let report = admiralty_core::git_health::evaluate(&repository, Some("refs/heads/main"), 20);
        let mut source = source();
        for n in 0..100 {
            source["projects"][format!("a-{n:03}")] = json!({});
        }
        source["git_health"]["z-conflicted"] = serde_json::to_value(report).expect("report");
        let bytes =
            compile(&serde_json::to_vec(&source).expect("json"), now(), 2048).expect("brief");
        let brief: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(brief["totals"]["git_unknown"], 1);
        assert_eq!(brief["totals"]["git_review_projects"], 1);
        assert_eq!(brief["totals"]["conflicted_checkouts"], 1);
        assert_eq!(brief["projects"][0]["id"], "z-conflicted");
        assert_eq!(brief["projects"][0]["issue_codes"]["merge_conflicts"], 1);
    }

    #[test]
    fn refuses_unbounded_scope_input_and_invalid_budget() {
        assert!(compile(&vec![b' '; MAX_SOURCE_BYTES + 1], now(), 2048).is_err());
        assert!(compile(b"{}", now(), 1024).is_err());
        let mut source = source();
        for n in 0..MAX_PROJECTS {
            source["projects"][format!("p-{n}")] = json!({});
        }
        assert!(
            compile(
                &serde_json::to_vec(&source).expect("json"),
                now(),
                MAX_BRIEF_BYTES
            )
            .is_err()
        );
    }
}
