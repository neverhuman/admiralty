//! Explicit GitHub adapter. No global gh login, worker environment or shell.
use super::publisher::{
    EffectObservation, Forge, ForgeObservation, PublicationRequest, private_directory,
    valid_repository,
};
use admiralty_core::publication::{ApprovalEvidence, CheckEvidence, CheckState};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

/// Run this capability in the publisher service account. The credential home
/// must be inaccessible to workers/verifiers; do not use their global gh config.
pub struct GitHubForge {
    executable: PathBuf,
    credential_home: PathBuf,
}

impl GitHubForge {
    pub fn new(executable: PathBuf, credential_home: PathBuf) -> Result<Self, String> {
        if !executable.is_absolute() || !credential_home.is_absolute() {
            return Err("absolute publisher executable/configuration paths required".into());
        }
        private_directory(&credential_home).map_err(|e| e.to_string())?;
        Ok(Self {
            executable,
            credential_home,
        })
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        private_directory(&self.credential_home).map_err(|e| e.to_string())?;
        let output = Command::new(&self.executable)
            .env_clear()
            .env("HOME", &self.credential_home)
            .env("GH_CONFIG_DIR", &self.credential_home)
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1")
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.credential_home)
            .args(args)
            .output()
            .map_err(|_| "publisher forge process failed")?;
        // Do not include stderr, credential configuration or environment values
        // in returned errors. Any failed mutation remains outcome-unknown.
        if !output.status.success() || output.stdout.len() > 1024 * 1024 {
            return Err("publisher forge command failed or exceeded output limit".into());
        }
        Ok(output.stdout)
    }

    fn api(&self, endpoint: &str) -> Result<Value, String> {
        serde_json::from_slice(&self.run(&["api", "--hostname", "github.com", endpoint])?)
            .map_err(|_| "malformed forge JSON".into())
    }

    fn pull(&self, repository: &str, number: u64) -> Result<Value, String> {
        if !valid_repository(repository) || number == 0 {
            return Err("invalid forge target".into());
        }
        let value = self.api(&format!("repos/{repository}/pulls/{number}"))?;
        if value["number"].as_u64() != Some(number)
            || text(&value["base"]["repo"]["full_name"])? != repository
        {
            return Err("forge response target mismatch".into());
        }
        Ok(value)
    }
}

fn text(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .ok_or_else(|| "missing or invalid forge text".into())
}
fn oid(value: &Value) -> Result<&str, String> {
    let s = text(value)?;
    if matches!(s.len(), 40 | 64)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(s)
    } else {
        Err("invalid forge revision".into())
    }
}
fn bounded_array(value: &Value) -> Result<&[Value], String> {
    value
        .as_array()
        .filter(|a| a.len() < 100)
        .map(Vec::as_slice)
        .ok_or_else(|| "forge evidence page incomplete or malformed".into())
}

impl Forge for GitHubForge {
    fn observe(&mut self, repository: &str, number: u64) -> Result<ForgeObservation, String> {
        let pull = self.pull(repository, number)?;
        let head = oid(&pull["head"]["sha"])?;
        let base = oid(&pull["base"]["sha"])?;
        let reviews = self.api(&format!(
            "repos/{repository}/pulls/{number}/reviews?per_page=100"
        ))?;
        let mut latest = BTreeMap::new();
        for review in bounded_array(&reviews)? {
            let state = text(&review["state"])?;
            // A comment does not dismiss an existing approval. Review IDs are
            // ordered explicitly; never trust a response's incidental ordering.
            if !matches!(state, "APPROVED" | "DISMISSED" | "CHANGES_REQUESTED") {
                continue;
            }
            let login = text(&review["user"]["login"])?;
            let id = review["id"].as_u64().ok_or("missing review id")?;
            if latest.get(login).is_none_or(|(old, _)| id > *old) {
                latest.insert(login, (id, review));
            }
        }
        let mut approvals = Vec::new();
        for (reviewer, (_, review)) in latest {
            approvals.push(ApprovalEvidence {
                reviewer: reviewer.into(),
                head: oid(&review["commit_id"])?.into(),
                dismissed: review["state"] != "APPROVED",
                provenance_ref: format!("github:{repository}:pr:{number}:review:{}", review["id"]),
            });
        }
        let runs = self.api(&format!(
            "repos/{repository}/commits/{head}/check-runs?per_page=100&filter=latest"
        ))?;
        let runs_list = bounded_array(&runs["check_runs"])?;
        if runs["total_count"].as_u64() != Some(runs_list.len() as u64) {
            return Err("incomplete check-run evidence".into());
        }
        let mut checks = Vec::new();
        for run in runs_list {
            let app = run["app"]["id"]
                .as_u64()
                .ok_or("missing check app identity")?;
            let id = run["id"].as_u64().ok_or("missing check run identity")?;
            checks.push(CheckEvidence {
                check_id: format!("check-run:{app}:{}", text(&run["name"])?),
                head: oid(&run["head_sha"])?.into(),
                state: if run["status"] != "completed" {
                    CheckState::Pending
                } else if run["conclusion"] == "success" {
                    CheckState::Passed
                } else {
                    CheckState::Failed
                },
                provenance_ref: format!("github:{repository}:check-run:{id}"),
            });
        }
        let statuses = self.api(&format!(
            "repos/{repository}/commits/{head}/status?per_page=100"
        ))?;
        let statuses_list = bounded_array(&statuses["statuses"])?;
        if statuses["total_count"].as_u64() != Some(statuses_list.len() as u64)
            || oid(&statuses["sha"])? != head
        {
            return Err("incomplete commit-status evidence".into());
        }
        for status in statuses_list {
            let creator = status["creator"]["id"]
                .as_u64()
                .ok_or("missing status issuer")?;
            let id = status["id"].as_u64().ok_or("missing status id")?;
            checks.push(CheckEvidence {
                check_id: format!("status:{creator}:{}", text(&status["context"])?),
                head: head.into(),
                state: match text(&status["state"])? {
                    "success" => CheckState::Passed,
                    "pending" => CheckState::Pending,
                    _ => CheckState::Failed,
                },
                provenance_ref: format!("github:{repository}:status:{id}"),
            });
        }
        Ok(ForgeObservation {
            repository: repository.into(),
            target_ref: format!("refs/heads/{}", text(&pull["base"]["ref"])?),
            pull_request: number,
            head: head.into(),
            base: base.into(),
            open: pull["state"] == "open" && pull["merged"] == false && pull["draft"] == false,
            approvals,
            checks,
        })
    }

    fn merge_rebase(
        &mut self,
        repository: &str,
        request: &PublicationRequest,
    ) -> Result<(), String> {
        // A third fresh read closes the gap introduced by persisting the intent.
        // GitHub also compares the expected PR head atomically with the merge.
        let current = self.pull(repository, request.pull_request)?;
        if oid(&current["head"]["sha"])? != request.expected_head
            || oid(&current["base"]["sha"])? != request.expected_base
            || format!("refs/heads/{}", text(&current["base"]["ref"])?) != request.scope.target_ref
            || current["state"] != "open"
            || current["merged"] != false
            || current["draft"] != false
        {
            return Err("forge head/base/state changed before dispatch".into());
        }
        self.run(&[
            "pr",
            "merge",
            &request.pull_request.to_string(),
            "--repo",
            repository,
            "--rebase",
            "--match-head-commit",
            &request.expected_head,
        ])?;
        Ok(())
    }

    fn reconcile(
        &mut self,
        repository: &str,
        request: &PublicationRequest,
    ) -> Result<EffectObservation, String> {
        let pull = self.pull(repository, request.pull_request)?;
        if pull["merged"] != true {
            return Ok(EffectObservation::Unknown);
        }
        if oid(&pull["head"]["sha"])? != request.expected_head
            || format!("refs/heads/{}", text(&pull["base"]["ref"])?) != request.scope.target_ref
        {
            return Err("merged PR identity differs from intent".into());
        }
        Ok(EffectObservation::Applied {
            head: request.expected_head.clone(),
            commit: oid(&pull["merge_commit_sha"])?.into(),
            provenance: format!(
                "github:{repository}:pr:{}:merged:{}",
                request.pull_request,
                text(&pull["merged_at"])?
            ),
        })
    }
}
