//! Pure fleet placement proposals. This module never grants execution authority.
use crate::{ProjectId, RootId, TaskId};
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
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementPolicy {
    pub max_observation_age: DecimalU64,
    pub maximum_pressure: DecimalU64,
    pub minimum_spool_reserve: DecimalU64,
    pub disconnected_continuation_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlacementProposal {
    pub project: ProjectId,
    pub root: RootId,
    pub task: TaskId,
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
}

pub fn propose_placement(
    request: &PlacementRequest,
    offers: &[NodeOffer],
    policy: &PlacementPolicy,
    explicit_now: u64,
) -> Result<PlacementProposal, PlacementError> {
    let encoded_size = serde_json::to_vec(&(request, offers, policy, explicit_now))
        .map_err(|_| PlacementError::Invalid("placement input serialization"))?
        .len();
    if encoded_size > MAX_JSON_BYTES {
        return Err(PlacementError::JsonOversize);
    }
    if offers.len() > MAX_OFFERS {
        return Err(PlacementError::Oversize("offers"));
    }
    if request.contract_digest.len() > 128 || !request.contract_digest.starts_with("sha256:") {
        return Err(PlacementError::Invalid("contract_digest"));
    }
    if request.required_capabilities.len() > MAX_COLLECTION
        || request.locality_refs.len() > MAX_COLLECTION
    {
        return Err(PlacementError::Oversize("request collections"));
    }
    if request
        .required_capabilities
        .iter()
        .chain(&request.locality_refs)
        .any(|s| s.len() > MAX_TEXT)
    {
        return Err(PlacementError::Oversize("request string"));
    }
    let now = explicit_now;
    let max_age = policy.max_observation_age.parse()?;
    let max_pressure = policy.maximum_pressure.parse()?;
    let budget = request.budget_requirement.amount.parse()?;
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
            if active.ordinary && !roots.insert(active.root.clone()) {
                return Err(PlacementError::DuplicateRootEvidence);
            }
        }
        validate_text(&o.boot_id)?;
        validate_text(&o.account_pool.value)?;
        if let Some(permit) = &o.continuation_permit {
            for value in [
                &permit.permit_id,
                &permit.immutable_digest,
                &permit.reboot_id,
            ] {
                validate_text(value)?;
            }
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
    for o in offers {
        let mut reject = |reason: &str| {
            rejected.push(Rejection {
                node_id: o.node_id.clone(),
                reason: reason.to_owned(),
            })
        };
        if o.reconciliation_required {
            reject("reconciliation required");
            continue;
        }
        let has_local_root = o
            .existing_active_roots
            .iter()
            .any(|r| r.ordinary && r.root == request.root);
        let valid_continuation_binding = !o.controller_connected
            && o.continuation_permit.as_ref().is_some_and(|p| {
                p.root == request.root && p.immutable_digest == request.contract_digest
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
        let permit_id = if o.controller_connected {
            if !fresh(&o.spool_free, now, max_age)? {
                reject("spool observation stale");
                continue;
            }
            match &o.spool_free.value {
                Known::Known { value }
                    if value.parse()? >= policy.minimum_spool_reserve.parse()? => {}
                Known::Known { .. } => {
                    reject("spool reserve insufficient");
                    continue;
                }
                Known::Unknown { .. } => {
                    reject("spool unknown");
                    continue;
                }
            }
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
            if p.root != request.root || p.immutable_digest != request.contract_digest {
                reject("permit does not bind immutable root contract");
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
    Ok(PlacementProposal {
        project: request.project.clone(),
        root: request.root.clone(),
        task: request.task.clone(),
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
    } else {
        Ok(())
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
            contract_digest: "sha256:abc".into(),
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
        }
    }
    fn policy() -> PlacementPolicy {
        PlacementPolicy {
            max_observation_age: d(10),
            maximum_pressure: d(5),
            minimum_spool_reserve: d(2),
            disconnected_continuation_only: true,
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
            immutable_digest: "sha256:abc".into(),
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
        a.continuation_permit = Some(IsolatedPermit {
            permit_id: "permit".into(),
            root: request("r").root,
            immutable_digest: "sha256:abc".into(),
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
}
