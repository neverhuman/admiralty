//! Pure fleet placement proposals. This module never grants execution authority.
use crate::{ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_OFFERS: usize = 256;
pub const MAX_COLLECTION: usize = 1024;
pub const MAX_TEXT: usize = 4096;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct NodeId(String);
impl NodeId {
    pub fn new(value: impl Into<String>) -> Result<Self, PlacementError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value == "."
            || value == ".."
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(PlacementError::Invalid("node_id"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for NodeId {
    type Error = PlacementError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        Self::new(v)
    }
}
impl From<NodeId> for String {
    fn from(v: NodeId) -> Self {
        v.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct DecimalU64(String);
impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value.to_string())
    }
    pub fn parse(&self) -> Result<u64, PlacementError> {
        self.0
            .parse::<u64>()
            .ok()
            .filter(|n| n.to_string() == self.0)
            .ok_or(PlacementError::Invalid("canonical decimal u64"))
    }
}
impl TryFrom<String> for DecimalU64 {
    type Error = PlacementError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        let x = Self(v);
        x.parse()?;
        Ok(x)
    }
}
impl From<DecimalU64> for String {
    fn from(v: DecimalU64) -> Self {
        v.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementRequest {
    pub project: ProjectId,
    pub root: RootId,
    pub task: TaskId,
    pub contract_digest: String,
    pub authority_ref: String,
    pub required_capabilities: Vec<String>,
    pub locality_refs: Vec<String>,
    pub budget_requirement: BudgetRequirement,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BudgetRequirement {
    pub account_pool: String,
    pub amount: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Observation<T> {
    pub value: T,
    pub observed_at: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Known<T> {
    Known { value: T },
    Unknown { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
    pub slots: DecimalU64,
    pub budget_remaining: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActiveRoot {
    pub root: RootId,
    pub ordinary: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IsolatedPermit {
    pub permit_id: String,
    pub root: RootId,
    pub project: ProjectId,
    pub task: TaskId,
    pub node_id: NodeId,
    pub authority_ref: String,
    pub expires_at: DecimalU64,
    pub immutable_digest: String,
    pub authority_generation: DecimalU64,
    pub reboot_id: String,
    pub remaining_budget: DecimalU64,
    pub spool_reserve: DecimalU64,
    pub minimum_spool_reserve: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NodeOffer {
    pub node_id: NodeId,
    pub boot_id: String,
    pub authority_generation: DecimalU64,
    pub visible_scopes: Observation<Vec<String>>,
    pub capabilities: Observation<Vec<String>>,
    pub account_pool: Observation<String>,
    pub capacity: Observation<Known<Capacity>>,
    pub current_pressure: Observation<Known<DecimalU64>>,
    pub spool_free: Observation<Known<DecimalU64>>,
    pub existing_active_roots: Vec<ActiveRoot>,
    pub controller_connected: bool,
    pub locality_refs: Vec<String>,
    pub continuation_permit: Option<IsolatedPermit>,
    pub reconciliation_required: bool,
    #[serde(default)]
    pub ownership_observed_at: Option<DecimalU64>,
    #[serde(default)]
    pub ownership_revision: Option<DecimalU64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementPolicy {
    pub max_observation_age: DecimalU64,
    pub maximum_pressure: DecimalU64,
    pub minimum_spool_reserve: DecimalU64,
    pub disconnected_continuation_only: bool,
    /// Expected canonical ownership snapshot revision, not a node-supplied grant.
    #[serde(default)]
    pub ownership_revision: Option<DecimalU64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementProposal {
    pub project: ProjectId,
    pub root: RootId,
    pub task: TaskId,
    pub contract_digest: String,
    pub authority_ref: String,
    pub source_digest: String,
    pub candidates: Vec<PlacementCandidate>,
    pub rejected: Vec<Rejection>,
    pub execution_granted: bool,
    pub publication_granted: bool,
    pub authority_granted: bool,
    pub decided_at: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementCandidate {
    pub node_id: NodeId,
    pub continuation_permit_id: Option<String>,
    pub locality_score: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rejection {
    pub node_id: NodeId,
    pub reason: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlacementError {
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("input exceeds {0}")]
    Oversize(&'static str),
    #[error("duplicate node offer")]
    DuplicateNode,
    #[error("duplicate root offer; active-root evidence must be represented on its owning node")]
    DuplicateRootEvidence,
    #[error("serialized placement input exceeds 1 MiB")]
    JsonOversize,
    #[error("decision clock unknown or rolled back")]
    Clock,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementInput {
    pub request: PlacementRequest,
    pub offers: Vec<NodeOffer>,
    pub policy: PlacementPolicy,
    pub now: Option<DecimalU64>,
    pub previous_tick: Option<DecimalU64>,
}

pub fn decode_input(bytes: &[u8]) -> Result<PlacementInput, PlacementError> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(PlacementError::JsonOversize);
    }
    let input: PlacementInput =
        serde_json::from_slice(bytes).map_err(|_| PlacementError::Invalid("placement JSON"))?;
    propose_placement_recorded(
        &input.request,
        &input.offers,
        &input.policy,
        input.now.as_ref().map(DecimalU64::parse).transpose()?,
        input
            .previous_tick
            .as_ref()
            .map(DecimalU64::parse)
            .transpose()?,
    )?;
    Ok(input)
}

pub fn propose_placement(
    request: &PlacementRequest,
    offers: &[NodeOffer],
    policy: &PlacementPolicy,
    explicit_now: u64,
) -> Result<PlacementProposal, PlacementError> {
    propose_placement_recorded(request, offers, policy, Some(explicit_now), None)
}

pub fn propose_placement_recorded(
    request: &PlacementRequest,
    offers: &[NodeOffer],
    policy: &PlacementPolicy,
    explicit_now: Option<u64>,
    previous_tick: Option<u64>,
) -> Result<PlacementProposal, PlacementError> {
    if offers.len() > MAX_OFFERS {
        return Err(PlacementError::Oversize("offers"));
    }
    bounded_json_size(
        &(request, offers, policy, explicit_now, previous_tick),
        MAX_JSON_BYTES,
    )
    .map_err(|_| PlacementError::JsonOversize)?;
    let now = explicit_now.ok_or(PlacementError::Clock)?;
    if previous_tick.is_some_and(|previous| now < previous) {
        return Err(PlacementError::Clock);
    }
    if !valid_digest(&request.contract_digest) {
        return Err(PlacementError::Invalid("contract_digest"));
    }
    validate_text(&request.authority_ref)?;
    validate_text(&request.budget_requirement.account_pool)?;
    if request.required_capabilities.len() > MAX_COLLECTION
        || request.locality_refs.len() > MAX_COLLECTION
    {
        return Err(PlacementError::Oversize("request collections"));
    }
    validate_unique(&request.required_capabilities)?;
    validate_unique(&request.locality_refs)?;
    let max_age = policy.max_observation_age.parse()?;
    let max_pressure = policy.maximum_pressure.parse()?;
    let budget = request.budget_requirement.amount.parse()?;
    let ownership_revision = policy
        .ownership_revision
        .as_ref()
        .map(DecimalU64::parse)
        .transpose()?;
    if ownership_revision == Some(0) {
        return Err(PlacementError::Invalid("ownership revision"));
    }
    let mut nodes = HashSet::new();
    let mut roots = HashSet::new();
    for o in offers {
        if !nodes.insert(o.node_id.clone()) {
            return Err(PlacementError::DuplicateNode);
        }
        if o.existing_active_roots.len() > MAX_COLLECTION {
            return Err(PlacementError::Oversize("active roots"));
        }
        if o.visible_scopes.value.len() > MAX_COLLECTION
            || o.capabilities.value.len() > MAX_COLLECTION
            || o.locality_refs.len() > MAX_COLLECTION
        {
            return Err(PlacementError::Oversize("offer collections"));
        }
        for active in &o.existing_active_roots {
            if !roots.insert(active.root.clone()) {
                return Err(PlacementError::DuplicateRootEvidence);
            }
        }
        validate_text(&o.boot_id)?;
        validate_text(&o.account_pool.value)?;
        if o.authority_generation.parse()? == 0 {
            return Err(PlacementError::Invalid("authority generation"));
        }
        validate_unique(&o.visible_scopes.value)?;
        validate_unique(&o.capabilities.value)?;
        validate_unique(&o.locality_refs)?;
        for reason in [
            unknown_reason(&o.capacity.value),
            unknown_reason(&o.current_pressure.value),
            unknown_reason(&o.spool_free.value),
        ]
        .into_iter()
        .flatten()
        {
            validate_text(reason)?;
        }
        if let Some(permit) = &o.continuation_permit {
            for value in [
                &permit.permit_id,
                &permit.immutable_digest,
                &permit.reboot_id,
                &permit.authority_ref,
            ] {
                validate_text(value)?;
            }
            if !valid_digest(&permit.immutable_digest) {
                return Err(PlacementError::Invalid("permit contract digest"));
            }
            permit.expires_at.parse()?;
        }
        for s in o
            .visible_scopes
            .value
            .iter()
            .chain(&o.capabilities.value)
            .chain(&o.locality_refs)
        {
            validate_text(s)?;
        }
    }
    if request.required_capabilities.is_empty() && request.project.as_str().is_empty() {
        return Err(PlacementError::Invalid("request"));
    }
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    let ownership_reconciled = ownership_revision.is_some()
        && offers.iter().all(|offer| {
            !offer.reconciliation_required
                && offer
                    .ownership_revision
                    .as_ref()
                    .and_then(|revision| revision.parse().ok())
                    == ownership_revision
                && offer
                    .ownership_observed_at
                    .as_ref()
                    .and_then(|tick| tick.parse().ok())
                    .is_some_and(|at| at <= now && now - at <= max_age)
        });
    for o in offers {
        let mut reject = |reason: &str| {
            rejected.push(Rejection {
                node_id: o.node_id.clone(),
                reason: reason.to_owned(),
            })
        };
        if !ownership_reconciled {
            reject("topology ownership unknown, stale, or unreconciled");
            continue;
        }
        let has_local_root = o
            .existing_active_roots
            .iter()
            .any(|r| r.root == request.root);
        let valid_continuation_binding = !o.controller_connected
            && o.continuation_permit.as_ref().is_some_and(|p| {
                p.root == request.root
                    && p.project == request.project
                    && p.task == request.task
                    && p.node_id == o.node_id
                    && p.authority_ref == request.authority_ref
                    && p.immutable_digest == request.contract_digest
            });
        // A known active root blocks new placement topology-wide. Only its own disconnected
        // node can be considered for bounded continuation under the matching permit.
        if roots.contains(&request.root) && !(has_local_root && valid_continuation_binding) {
            reject("root already active in topology");
            continue;
        }
        if has_local_root && !valid_continuation_binding {
            reject("root already active on this node");
            continue;
        }
        if o.visible_scopes.value.iter().any(|s| s.len() > MAX_TEXT)
            || !fresh(&o.visible_scopes, now, max_age)?
        {
            reject("scope observation stale");
            continue;
        }
        if !o
            .visible_scopes
            .value
            .contains(&request.project.to_string())
        {
            reject("foreign or missing project scope");
            continue;
        }
        if !fresh(&o.capabilities, now, max_age)?
            || !request
                .required_capabilities
                .iter()
                .all(|c| o.capabilities.value.contains(c))
        {
            reject("capability missing, unknown, or stale");
            continue;
        }
        if !fresh(&o.account_pool, now, max_age)?
            || o.account_pool.value != request.budget_requirement.account_pool
        {
            reject("account pool mismatch or stale");
            continue;
        }
        if !fresh(&o.current_pressure, now, max_age)? {
            reject("pressure observation stale");
            continue;
        }
        let pressure = match &o.current_pressure.value {
            Known::Known { value } => value.parse()?,
            Known::Unknown { .. } => {
                reject("pressure unknown");
                continue;
            }
        };
        if pressure > max_pressure {
            reject("pressure exceeds policy");
            continue;
        }
        if !fresh(&o.capacity, now, max_age)? {
            reject("capacity observation stale");
            continue;
        }
        let cap = match &o.capacity.value {
            Known::Known { value } => value,
            Known::Unknown { .. } => {
                reject("capacity unknown");
                continue;
            }
        };
        if cap.slots.parse()? == 0 || cap.budget_remaining.parse()? < budget {
            reject("capacity or budget insufficient");
            continue;
        }
        if !fresh(&o.spool_free, now, max_age)? {
            reject("spool observation stale");
            continue;
        }
        match &o.spool_free.value {
            Known::Known { value } if value.parse()? >= policy.minimum_spool_reserve.parse()? => {}
            Known::Known { .. } => {
                reject("spool reserve insufficient");
                continue;
            }
            Known::Unknown { .. } => {
                reject("spool unknown");
                continue;
            }
        }
        let permit_id = if o.controller_connected {
            None
        } else {
            if !policy.disconnected_continuation_only {
                reject("controller disconnected");
                continue;
            }
            let Some(p) = &o.continuation_permit else {
                reject("no prior isolated permit");
                continue;
            };
            if !has_local_root {
                reject("disconnected node has no existing active root to continue");
                continue;
            }
            if !valid_continuation_binding {
                reject("permit does not bind exact project/root/task/node/authority/contract");
                continue;
            }
            if now >= p.expires_at.parse()? {
                reject("continuation permit expired");
                continue;
            }
            if p.reboot_id != o.boot_id
                || p.authority_generation != o.authority_generation
                || o.reconciliation_required
            {
                reject("permit stale after reboot or authority generation change");
                continue;
            }
            if p.remaining_budget.parse()? < budget {
                reject("continuation budget exhausted");
                continue;
            }
            if p.spool_reserve.parse()? < p.minimum_spool_reserve.parse()?
                || p.spool_reserve.parse()? < policy.minimum_spool_reserve.parse()?
            {
                reject("continuation spool reserve insufficient");
                continue;
            }
            Some(p.permit_id.clone())
        };
        let locality_score = request
            .locality_refs
            .iter()
            .filter(|r| o.locality_refs.contains(r))
            .count();
        candidates.push(PlacementCandidate {
            node_id: o.node_id.clone(),
            continuation_permit_id: permit_id,
            locality_score,
        });
    }
    candidates.sort_by(|a, b| {
        b.locality_score
            .cmp(&a.locality_score)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    rejected.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let mut sorted_offers: Vec<_> = offers.iter().collect();
    sorted_offers.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let source_digest =
        content_digest(&(request, sorted_offers, policy, explicit_now, previous_tick))
            .map_err(|_| PlacementError::Invalid("placement input digest"))?;
    Ok(PlacementProposal {
        project: request.project.clone(),
        root: request.root.clone(),
        task: request.task.clone(),
        contract_digest: request.contract_digest.clone(),
        authority_ref: request.authority_ref.clone(),
        source_digest,
        candidates,
        rejected,
        execution_granted: false,
        publication_granted: false,
        authority_granted: false,
        decided_at: DecimalU64::new(now),
    })
}

fn validate_text(s: &str) -> Result<(), PlacementError> {
    if s.len() > MAX_TEXT {
        Err(PlacementError::Oversize("string"))
    } else if s.trim().is_empty() || s.chars().any(char::is_control) {
        Err(PlacementError::Invalid("empty or control string"))
    } else {
        Ok(())
    }
}
fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
fn validate_unique(values: &[String]) -> Result<(), PlacementError> {
    let mut seen = HashSet::new();
    for value in values {
        validate_text(value)?;
        if !seen.insert(value) {
            return Err(PlacementError::Invalid("duplicate collection entry"));
        }
    }
    Ok(())
}
fn unknown_reason<T>(known: &Known<T>) -> Option<&str> {
    match known {
        Known::Unknown { reason } => Some(reason),
        Known::Known { .. } => None,
    }
}
fn fresh<T>(o: &Observation<T>, now: u64, age: u64) -> Result<bool, PlacementError> {
    let at = o.observed_at.parse()?;
    Ok(at <= now
        && now
            .checked_sub(at)
            .ok_or(PlacementError::Invalid("observation time"))?
            <= age)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(n: u64) -> DecimalU64 {
        DecimalU64::new(n)
    }
    fn request(root: &str) -> PlacementRequest {
        PlacementRequest {
            project: ProjectId::new("p").expect("valid project fixture"),
            root: RootId::new(root).expect("valid root fixture"),
            task: TaskId::new("t").expect("valid task fixture"),
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            authority_ref: "grant-1".into(),
            required_capabilities: vec!["rust".into()],
            locality_refs: vec!["src".into()],
            budget_requirement: BudgetRequirement {
                account_pool: "pool".into(),
                amount: d(5),
            },
        }
    }
    fn offer(id: &str) -> NodeOffer {
        NodeOffer {
            node_id: NodeId::new(id).expect("valid node fixture"),
            boot_id: "boot".into(),
            authority_generation: d(1),
            visible_scopes: Observation {
                value: vec!["p".into()],
                observed_at: d(10),
            },
            capabilities: Observation {
                value: vec!["rust".into()],
                observed_at: d(10),
            },
            account_pool: Observation {
                value: "pool".into(),
                observed_at: d(10),
            },
            capacity: Observation {
                value: Known::Known {
                    value: Capacity {
                        slots: d(1),
                        budget_remaining: d(10),
                    },
                },
                observed_at: d(10),
            },
            current_pressure: Observation {
                value: Known::Known { value: d(1) },
                observed_at: d(10),
            },
            spool_free: Observation {
                value: Known::Known { value: d(10) },
                observed_at: d(10),
            },
            existing_active_roots: vec![],
            controller_connected: true,
            locality_refs: vec!["src".into()],
            continuation_permit: None,
            reconciliation_required: false,
            ownership_observed_at: Some(d(10)),
            ownership_revision: Some(d(1)),
        }
    }
    fn policy() -> PlacementPolicy {
        PlacementPolicy {
            max_observation_age: d(10),
            maximum_pressure: d(5),
            minimum_spool_reserve: d(2),
            disconnected_continuation_only: true,
            ownership_revision: Some(d(1)),
        }
    }
    #[test]
    fn orders_locality_then_node_and_is_input_order_independent() {
        let a = offer("a");
        let mut b = offer("b");
        b.locality_refs.clear();
        let x = propose_placement(&request("r"), &[b.clone(), a.clone()], &policy(), 10)
            .expect("proposal");
        let y = propose_placement(&request("r"), &[a, b], &policy(), 10).expect("proposal");
        assert_eq!(x, y);
        assert_eq!(x.candidates[0].node_id.as_str(), "a");
        assert!(!x.execution_granted);
    }
    #[test]
    fn rejects_unknown_stale_foreign_and_same_root_anywhere() {
        let mut a = offer("a");
        a.existing_active_roots.push(ActiveRoot {
            root: RootId::new("r").expect("root"),
            ordinary: true,
        });
        let mut b = offer("b");
        b.controller_connected = false;
        let p = propose_placement(&request("r"), &[a, b], &policy(), 10).expect("safe rejection");
        assert!(p.candidates.is_empty());
    }
    #[test]
    fn disconnected_only_continues_bound_permit_with_budget_and_spool() {
        let mut a = offer("a");
        a.controller_connected = false;
        a.existing_active_roots.push(ActiveRoot {
            root: request("r").root.clone(),
            ordinary: true,
        });
        a.continuation_permit = Some(IsolatedPermit {
            permit_id: "permit".into(),
            root: request("r").root,
            immutable_digest: request("r").contract_digest,
            project: request("r").project,
            task: request("r").task,
            node_id: a.node_id.clone(),
            authority_ref: "grant-1".into(),
            expires_at: d(20),
            authority_generation: d(1),
            reboot_id: "boot".into(),
            remaining_budget: d(5),
            spool_reserve: d(3),
            minimum_spool_reserve: d(2),
        });
        let p = propose_placement(&request("r"), &[a.clone()], &policy(), 10).expect("proposal");
        assert_eq!(
            p.candidates[0].continuation_permit_id.as_deref(),
            Some("permit")
        );
        a.boot_id = "new-boot".into();
        assert!(
            propose_placement(&request("r"), &[a], &policy(), 10)
                .expect("proposal")
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn arbitrary_topology_and_unknown_capacity_do_not_assume_host_names() {
        let ids = [
            "550e8400-e29b-41d4-a716-446655440000",
            "7f3a1c2d-9876-4abc-8123-abcdef012345",
            "node.alpha-2",
            "z_9",
        ];
        let mut offers: Vec<_> = ids.iter().map(|id| offer(id)).collect();
        offers[2].capacity.value = Known::Unknown {
            reason: "not reported".into(),
        };
        let proposal = propose_placement(&request("new-root"), &offers, &policy(), 10)
            .expect("synthetic topology proposal");
        assert_eq!(proposal.candidates.len(), 3);
        assert_eq!(proposal.rejected[0].reason, "capacity unknown");
    }

    #[test]
    fn continuation_rejects_authority_generation_budget_and_spool_mismatch() {
        let mut a = offer("node");
        a.controller_connected = false;
        a.existing_active_roots.push(ActiveRoot {
            root: request("r").root.clone(),
            ordinary: true,
        });
        a.continuation_permit = Some(IsolatedPermit {
            permit_id: "permit".into(),
            root: request("r").root,
            immutable_digest: request("r").contract_digest,
            project: request("r").project,
            task: request("r").task,
            node_id: a.node_id.clone(),
            authority_ref: "grant-1".into(),
            expires_at: d(20),
            authority_generation: d(2),
            reboot_id: "boot".into(),
            remaining_budget: d(4),
            spool_reserve: d(1),
            minimum_spool_reserve: d(2),
        });
        let p = propose_placement(&request("r"), &[a.clone()], &policy(), 10).expect("proposal");
        assert!(p.candidates.is_empty());
        assert!(p.rejected[0].reason.contains("authority generation"));
        a.authority_generation = d(2);
        let p = propose_placement(&request("r"), &[a], &policy(), 10).expect("proposal");
        assert!(p.candidates.is_empty());
        assert!(p.rejected[0].reason.contains("budget"));
    }

    #[test]
    fn input_bounds_and_unknown_fields_are_explicit() {
        let too_many = vec![offer("n"); MAX_OFFERS + 1];
        assert_eq!(
            propose_placement(&request("r"), &too_many, &policy(), 10),
            Err(PlacementError::Oversize("offers"))
        );
        let malformed = serde_json::json!({"project":"p","extra":true});
        assert!(serde_json::from_value::<PlacementRequest>(malformed).is_err());
    }
    #[test]
    fn canonical_counters_and_bounds_fail_closed() {
        assert!(DecimalU64("01".into()).parse().is_err());
        assert!(DecimalU64("18446744073709551616".into()).parse().is_err());
        let mut a = offer("a");
        a.visible_scopes.observed_at = d(0);
        let p = propose_placement(&request("r"), &[a], &policy(), 100).expect("proposal");
        assert!(p.candidates.is_empty());
    }

    fn continuing_offer() -> NodeOffer {
        let mut node = offer("continuing-node");
        let request = request("r");
        node.controller_connected = false;
        node.existing_active_roots = vec![ActiveRoot {
            root: request.root.clone(),
            ordinary: true,
        }];
        node.continuation_permit = Some(IsolatedPermit {
            permit_id: "permit".into(),
            root: request.root,
            project: request.project,
            task: request.task,
            node_id: node.node_id.clone(),
            authority_ref: request.authority_ref,
            immutable_digest: request.contract_digest,
            expires_at: d(20),
            authority_generation: d(1),
            reboot_id: node.boot_id.clone(),
            remaining_budget: d(10),
            spool_reserve: d(5),
            minimum_spool_reserve: d(2),
        });
        node
    }

    #[test]
    fn unknown_or_unreconciled_ownership_blocks_the_entire_topology_not_just_one_node() {
        for variant in ["unknown_time", "future", "stale", "revision", "reconcile"] {
            let mut unknown = offer("unknown");
            match variant {
                "unknown_time" => unknown.ownership_observed_at = None,
                "future" => unknown.ownership_observed_at = Some(d(11)),
                "stale" => unknown.ownership_observed_at = Some(d(0)),
                "revision" => unknown.ownership_revision = Some(d(2)),
                _ => unknown.reconciliation_required = true,
            }
            let now = if variant == "stale" { 20 } else { 10 };
            let proposal = propose_placement(
                &request("r"),
                &[offer("otherwise-ready"), unknown],
                &policy(),
                now,
            )
            .expect("explicit ownership rejection");
            assert!(proposal.candidates.is_empty(), "accepted {variant}");
            assert_eq!(proposal.rejected.len(), 2);
            assert!(
                proposal
                    .rejected
                    .iter()
                    .all(|row| row.reason.contains("ownership"))
            );
        }
        let mut unknown = policy();
        unknown.ownership_revision = None;
        assert!(
            propose_placement(&request("r"), &[offer("ready")], &unknown, 10)
                .expect("unknown topology revision")
                .candidates
                .is_empty()
        );
        let mut owner = offer("owner");
        owner.existing_active_roots = vec![ActiveRoot {
            root: request("r").root,
            ordinary: false,
        }];
        assert!(
            propose_placement(&request("r"), &[offer("ready"), owner], &policy(), 10)
                .expect("nonordinary ownership is not free")
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn offline_continuation_cannot_be_rebound_extended_or_used_to_start_a_new_root() {
        let original = continuing_offer();
        assert_eq!(
            propose_placement(
                &request("r"),
                std::slice::from_ref(&original),
                &policy(),
                10
            )
            .expect("bound continuation proposal")
            .candidates
            .len(),
            1
        );
        for variant in [
            "project",
            "root",
            "task",
            "node",
            "authority",
            "contract",
            "expired",
            "new_root",
            "spool_unknown",
        ] {
            let mut node = original.clone();
            let permit = node.continuation_permit.as_mut().expect("permit");
            match variant {
                "project" => permit.project = ProjectId::new("foreign").expect("project"),
                "root" => permit.root = RootId::new("foreign").expect("root"),
                "task" => permit.task = TaskId::new("foreign").expect("task"),
                "node" => permit.node_id = NodeId::new("foreign").expect("node"),
                "authority" => permit.authority_ref = "foreign-grant".into(),
                "contract" => permit.immutable_digest = format!("sha256:{}", "b".repeat(64)),
                "expired" => permit.expires_at = d(10),
                "new_root" => node.existing_active_roots.clear(),
                _ => {
                    node.spool_free.value = Known::Unknown {
                        reason: "unobserved".into(),
                    }
                }
            }
            let proposal = propose_placement(&request("r"), &[node], &policy(), 10)
                .expect("continuation rejection");
            assert!(proposal.candidates.is_empty(), "accepted {variant}");
            assert!(
                !proposal.execution_granted
                    && !proposal.publication_granted
                    && !proposal.authority_granted
            );
        }
        assert_eq!(original, continuing_offer());
    }

    #[test]
    fn recorded_clocks_parser_bounds_and_source_bindings_are_explicit() {
        let input = PlacementInput {
            request: request("r"),
            offers: vec![offer("b"), offer("a")],
            policy: policy(),
            now: Some(d(10)),
            previous_tick: Some(d(9)),
        };
        let bytes = serde_json::to_vec(&input).expect("JSON");
        assert_eq!(decode_input(&bytes).expect("bounded input"), input);
        assert_eq!(
            propose_placement_recorded(&input.request, &input.offers, &input.policy, None, None),
            Err(PlacementError::Clock)
        );
        assert_eq!(
            propose_placement_recorded(
                &input.request,
                &input.offers,
                &input.policy,
                Some(10),
                Some(11)
            ),
            Err(PlacementError::Clock)
        );
        let proposal = propose_placement_recorded(
            &input.request,
            &input.offers,
            &input.policy,
            Some(10),
            Some(9),
        )
        .expect("recorded proposal");
        let mut reversed = input.offers.clone();
        reversed.reverse();
        assert_eq!(
            proposal,
            propose_placement_recorded(&input.request, &reversed, &input.policy, Some(10), Some(9))
                .expect("stable offer delivery")
        );
        assert_ne!(
            proposal.source_digest,
            propose_placement_recorded(
                &input.request,
                &input.offers,
                &input.policy,
                Some(10),
                Some(10)
            )
            .expect("changed clock receipt")
            .source_digest
        );
        assert_eq!(proposal.contract_digest, input.request.contract_digest);
        assert_eq!(proposal.authority_ref, input.request.authority_ref);
        let mut foreign = input.request.clone();
        foreign.project = ProjectId::new("foreign-project").expect("project");
        assert!(
            propose_placement(&foreign, &input.offers, &input.policy, 10)
                .expect("foreign visibility is not a candidate")
                .candidates
                .is_empty()
        );
        let mut invalid = input.request.clone();
        invalid.contract_digest = "sha256:abc".into();
        assert_eq!(
            propose_placement(&invalid, &input.offers, &input.policy, 10),
            Err(PlacementError::Invalid("contract_digest"))
        );
        invalid = input.request.clone();
        invalid.locality_refs.push(invalid.locality_refs[0].clone());
        assert_eq!(
            propose_placement(&invalid, &input.offers, &input.policy, 10),
            Err(PlacementError::Invalid("duplicate collection entry"))
        );
        let mut extra = serde_json::to_value(&input).expect("value");
        extra["execution_granted"] = true.into();
        assert!(decode_input(&serde_json::to_vec(&extra).expect("JSON")).is_err());
        assert_eq!(
            decode_input(&vec![b' '; MAX_JSON_BYTES + 1]),
            Err(PlacementError::JsonOversize)
        );
        let mut oversized = input.request.clone();
        oversized.authority_ref = "x".repeat(MAX_JSON_BYTES);
        assert_eq!(
            propose_placement(&oversized, &input.offers, &input.policy, 10),
            Err(PlacementError::JsonOversize)
        );
        let mut zero = offer("zero");
        zero.visible_scopes.observed_at = d(0);
        zero.capabilities.observed_at = d(0);
        zero.account_pool.observed_at = d(0);
        zero.capacity.observed_at = d(0);
        zero.current_pressure.observed_at = d(0);
        zero.spool_free.observed_at = d(0);
        zero.ownership_observed_at = Some(d(0));
        assert_eq!(
            propose_placement_recorded(&request("r"), &[zero], &policy(), Some(0), Some(0))
                .expect("logical clock zero is known")
                .candidates
                .len(),
            1
        );
    }
}
