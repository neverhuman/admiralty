//! Read-only visual projections. Presence, assignment and acceptance are separate facts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewsSnapshot {
    pub schema_version: u32,
    pub revision: String,
    pub simulated: bool,
    pub observed_utc: String,
    pub projects: Vec<ViewProject>,
    pub agents: Vec<ViewAgent>,
    pub sources: Vec<ViewSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewProject {
    pub id: String,
    pub name: String,
    pub git: Option<GitRepository>,
    pub task_count: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewAgent {
    pub id: String,
    pub name: String,
    pub role: ViewRole,
    pub project_id: Option<String>,
    pub parent_id: Option<String>,
    pub node: Option<String>,
    pub presence: ViewPresence,
    pub state: String,
    pub model: Option<String>,
    pub requested_model: Option<String>,
    pub model_source: String,
    pub model_observed_utc: Option<String>,
    pub effort: Option<String>,
    pub task: Option<String>,
    pub branch: Option<String>,
    pub observed_utc: Option<String>,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewRole {
    Owner,
    Watcher,
    Admiral,
    Commodore,
    Worker,
    Assignment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewPresence {
    Observed,
    Unobserved,
    Ended,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewSource {
    pub id: String,
    pub label: String,
    pub observed_utc: Option<String>,
    pub state: String,
    pub coverage: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitRepository {
    pub instance_id: String,
    pub node: String,
    pub observed_utc: String,
    pub error: Option<String>,
    pub shallow: bool,
    pub truncated: bool,
    pub commits: Vec<GitCommit>,
    pub refs: Vec<GitRef>,
    pub checkouts: Vec<GitCheckout>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCommit {
    pub oid: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub committed_utc: String,
    /// `Agent-*` trailers exactly as the commit message states them. This is
    /// the commit's own claim, not proof of which process or model authored it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_trailers: Option<GitAgentTrailers>,
}

/// First value of each fleet attribution trailer in a commit's final trailer
/// paragraph (`Agent-Session`, `Agent-Model`, `Agent-Task`). Absent trailers
/// stay absent; nothing is inferred from branch, checkout or screen names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitAgentTrailers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitRef {
    pub name: String,
    pub oid: String,
    pub commit_oid: Option<String>,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCheckout {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
    pub status: Option<GitWorkingState>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitWorkingState {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicts: u32,
}
