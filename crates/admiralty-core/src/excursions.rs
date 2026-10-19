//! Bounded, pure excursion episode reduction and local proposal construction.
//! Callers authenticate evidence and persist these projections; this module has no effects.
use crate::{ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_INPUT_BYTES: usize = 1_048_576;
pub const MAX_ITEMS: usize = 1024;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_CAUSAL_DEPTH: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExcursionScope {
    pub project: ProjectId,
    pub root: RootId,
    pub task: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub protected_oracle_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub digest: String,
    pub media_type: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IncidentEvent {
    pub event_id: String,
    pub family_signature: String,
    pub cause_parent: Option<String>,
    pub kind: String,
    pub evidence_digests: Vec<String>,
    pub observed_tick: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Episode {
    pub scope: ExcursionScope,
    pub episode_id: String,
    pub lineage: Vec<String>,
    pub family: String,
    pub events: Vec<IncidentEvent>,
    pub unknown_attribution: bool,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExcursionState {
    pub scope: ExcursionScope,
    pub episodes: Vec<Episode>,
    receipts: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReproductionContract {
    pub fields: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalProposal {
    pub scope: ExcursionScope,
    pub episode_digest: String,
    pub title: String,
    pub reproduction_contract: ReproductionContract,
    pub evidence_digests: Vec<String>,
    pub uncertainty: String,
    pub diagnosis: String,
    pub external_publication_allowed: bool,
    pub execution_granted: bool,
    pub proposed_investigator_max_steps: u16,
    pub proposed_investigator_max_model_turns: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ExcursionError(pub &'static str);

/// Input records bind each event to its exact task scope. Parent links are checked iteratively.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopedIncident {
    pub scope: ExcursionScope,
    pub event: IncidentEvent,
}

pub fn reduce_excursions(
    current: &Option<ExcursionState>,
    scope: &ExcursionScope,
    input: &[ScopedIncident],
) -> Result<ExcursionState, ExcursionError> {
    validate_scope(scope)?;
    bounded(input)?;
    if input.len() > MAX_ITEMS {
        return Err(ExcursionError("incident count exceeds bound"));
    }
    if let Some(previous) = current {
        bounded(previous)?;
        if previous.scope != *scope {
            return Err(ExcursionError("excursion scope mismatch"));
        }
        let mut events = Vec::new();
        for episode in &previous.episodes {
            if episode.events.is_empty()
                || episode.events.len() > MAX_ITEMS
                || events
                    .len()
                    .checked_add(episode.events.len())
                    .is_none_or(|count| count > MAX_ITEMS)
            {
                return Err(ExcursionError("state count exceeds bound"));
            }
            for event in &episode.events {
                events.push(ScopedIncident {
                    scope: scope.clone(),
                    event: event.clone(),
                });
            }
        }
        let canonical = reduce_inner(&None, scope, &events)?;
        if &canonical != previous {
            return Err(ExcursionError(
                "stored episode/receipt projection is not canonical",
            ));
        }
        let existing: BTreeSet<_> = events
            .iter()
            .map(|row| row.event.event_id.as_str())
            .collect();
        let new: BTreeSet<_> = input
            .iter()
            .map(|row| row.event.event_id.as_str())
            .filter(|id| !existing.contains(id))
            .collect();
        if existing
            .len()
            .checked_add(new.len())
            .is_none_or(|count| count > MAX_ITEMS)
        {
            return Err(ExcursionError("cumulative incident count exceeds bound"));
        }
    }
    let result = reduce_inner(current, scope, input)?;
    bounded(&result)?;
    Ok(result)
}
fn bounded<T: Serialize + ?Sized>(value: &T) -> Result<(), ExcursionError> {
    bounded_json_size(value, MAX_INPUT_BYTES)
        .map(|_| ())
        .map_err(|_| ExcursionError("invalid or oversized serialized excursion input"))
}
fn validate_scope(scope: &ExcursionScope) -> Result<(), ExcursionError> {
    if scope.contract_revision == 0 {
        return Err(ExcursionError("contract revision required"));
    }
    validate_digest(&scope.contract_digest)?;
    validate_digest(&scope.protected_oracle_digest)
}
fn reduce_inner(
    current: &Option<ExcursionState>,
    scope: &ExcursionScope,
    input: &[ScopedIncident],
) -> Result<ExcursionState, ExcursionError> {
    if input.len() > MAX_ITEMS {
        return Err(ExcursionError("incident input exceeds bound"));
    }
    let mut state = current.clone().unwrap_or_else(|| ExcursionState {
        scope: scope.clone(),
        episodes: Vec::new(),
        receipts: BTreeMap::new(),
    });
    if &state.scope != scope {
        return Err(ExcursionError("excursion scope mismatch"));
    }
    let mut all: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
    for (index, ep) in state.episodes.iter().enumerate() {
        for event in &ep.events {
            let digest =
                content_digest(event).map_err(|_| ExcursionError("cannot fingerprint event"))?;
            all.insert(
                event.event_id.clone(),
                (digest, ep.episode_id.clone(), index),
            );
        }
    }
    for row in input {
        if &row.scope != scope {
            return Err(ExcursionError("cross-scope parent or incident"));
        }
        validate_event(&row.event)?;
        let event = &row.event;
        let digest =
            content_digest(event).map_err(|_| ExcursionError("cannot fingerprint event"))?;
        if let Some(previous) = state.receipts.get(&event.event_id) {
            if previous != &digest {
                return Err(ExcursionError("event ID reused with different bytes"));
            }
            continue;
        }
        if let Some((previous, _, _)) = all.get(&event.event_id) {
            if previous != &digest {
                return Err(ExcursionError("duplicate event ID changed bytes"));
            }
            state.receipts.insert(event.event_id.clone(), digest);
            continue;
        }
        let mut lineage = BTreeSet::from([event.event_id.clone()]);
        let mut cursor = event.cause_parent.as_deref();
        let mut steps = 0;
        while let Some(parent) = cursor {
            steps += 1;
            if steps > MAX_CAUSAL_DEPTH {
                return Err(ExcursionError("causal graph depth bound exceeded"));
            }
            if !lineage.insert(parent.to_owned()) {
                return Err(ExcursionError("causal cycle detected"));
            }
            let Some((_, parent_episode, _)) = all.get(parent) else {
                return Err(ExcursionError("unknown or foreign-scope causal parent"));
            };
            if let Some(parent_event) = state
                .episodes
                .iter()
                .find(|episode| &episode.episode_id == parent_episode)
                .and_then(|episode| episode.events.iter().find(|value| value.event_id == parent))
                && let (Some(child), Some(parent_tick)) =
                    (&event.observed_tick, &parent_event.observed_tick)
                && canonical_decimal(child)? < canonical_decimal(parent_tick)?
            {
                return Err(ExcursionError("causal child predates its recorded parent"));
            }
            cursor = state
                .episodes
                .iter()
                .find(|ep| &ep.episode_id == parent_episode)
                .and_then(|ep| ep.events.iter().find(|e| e.event_id == parent))
                .and_then(|e| e.cause_parent.as_deref());
        }
        let correlated_episode = event
            .cause_parent
            .as_ref()
            .and_then(|id| all.get(id))
            .map(|(_, ep, _)| ep.clone());
        if let Some(ep_id) = correlated_episode {
            let ep = state
                .episodes
                .iter_mut()
                .find(|ep| ep.episode_id == ep_id)
                .ok_or(ExcursionError("causal episode missing"))?;
            if ep.family != event.family_signature {
                return Err(ExcursionError("causal lineage family mismatch"));
            }
            ep.lineage.push(event.event_id.clone());
            ep.events.push(event.clone());
            ep.unknown_attribution |= event.cause_parent.is_none();
        } else {
            let identity = content_digest(&(scope, &event.event_id))
                .map_err(|_| ExcursionError("cannot bind episode scope"))?;
            let id = format!("episode-{}", &identity[7..]);
            state.episodes.push(Episode {
                scope: scope.clone(),
                episode_id: id.clone(),
                lineage: vec![event.event_id.clone()],
                family: event.family_signature.clone(),
                events: vec![event.clone()],
                unknown_attribution: event.cause_parent.is_none(),
                evidence: Vec::new(),
            });
        }
        let owning = state
            .episodes
            .iter()
            .find(|ep| ep.events.iter().any(|e| e.event_id == event.event_id))
            .map(|ep| ep.episode_id.clone())
            .ok_or(ExcursionError("episode identity missing"))?;
        let index = state
            .episodes
            .iter()
            .position(|ep| ep.episode_id == owning)
            .ok_or(ExcursionError("episode projection missing"))?;
        all.insert(event.event_id.clone(), (digest.clone(), owning, index));
        state.receipts.insert(event.event_id.clone(), digest);
    }
    Ok(state)
}

fn validate_event(event: &IncidentEvent) -> Result<(), ExcursionError> {
    for id in [&event.event_id, &event.family_signature, &event.kind] {
        if !safe_identifier(id) {
            return Err(ExcursionError("missing or oversized incident identifier"));
        }
    }
    if event
        .cause_parent
        .as_ref()
        .is_some_and(|parent| !safe_identifier(parent))
    {
        return Err(ExcursionError("invalid causal parent identity"));
    }
    if event.evidence_digests.len() > MAX_ITEMS {
        return Err(ExcursionError("evidence collection exceeds bound"));
    }
    for digest in &event.evidence_digests {
        validate_digest(digest)?;
    }
    if let Some(tick) = &event.observed_tick {
        canonical_decimal(tick)?;
    }
    Ok(())
}
fn safe_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
fn canonical_decimal(value: &str) -> Result<u64, ExcursionError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == value)
        .ok_or(ExcursionError("tick must be a canonical decimal u64"))
}

/// Build a sanitized local proposal. This recognizes only narrow signatures; unknown cases
/// remain investigator proposals. It never launches an inquiry or grants publication.
pub fn build_local_proposal(
    episode: &Episode,
    reproduction: &ReproductionContract,
    evidence: &[EvidenceRef],
) -> Result<LocalProposal, ExcursionError> {
    bounded(&(episode, reproduction, evidence))?;
    validate_scope(&episode.scope)?;
    if episode.events.is_empty()
        || episode.events.len() > MAX_ITEMS
        || reproduction.fields.len() > 32
        || evidence.len() > MAX_ITEMS
    {
        return Err(ExcursionError("proposal input exceeds bound"));
    }
    let rows: Vec<_> = episode
        .events
        .iter()
        .map(|event| ScopedIncident {
            scope: episode.scope.clone(),
            event: event.clone(),
        })
        .collect();
    let rebuilt = reduce_inner(&None, &episode.scope, &rows)?;
    if rebuilt.episodes.as_slice() != std::slice::from_ref(episode) {
        return Err(ExcursionError(
            "proposal episode is not canonical scoped evidence",
        ));
    }
    let mut clean = BTreeMap::new();
    for (key, value) in &reproduction.fields {
        if key.is_empty()
            || key.len() > 128
            || value.len() > MAX_TEXT_BYTES
            || value.trim().is_empty()
            || value
                .chars()
                .any(|character| character.is_control() && character != '\n' && character != '\t')
            || !permitted_field(key)
            || unsafe_value(value)
        {
            return Err(ExcursionError(
                "reproduction field is not permitted or contains unsafe content",
            ));
        }
        clean.insert(key.clone(), value.clone());
    }
    for required in ["scope", "steps", "expected", "actual"] {
        if !clean.contains_key(required) {
            return Err(ExcursionError("required reproduction field missing"));
        }
    }
    let mut digests = BTreeSet::new();
    for r in evidence {
        validate_digest(&r.digest)?;
        if r.media_type.is_empty()
            || r.media_type.len() > 128
            || !r
                .media_type
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/.-+".contains(&byte))
        {
            return Err(ExcursionError("invalid evidence reference"));
        }
        digests.insert(r.digest.clone());
    }
    for e in &episode.events {
        for d in &e.evidence_digests {
            validate_digest(d)?;
            digests.insert(d.clone());
        }
    }
    let known = match episode.family.as_str() {
        "duplicate-replay" => "Rule-based diagnosis: repeated event identity was replayed.",
        "fail-escalate" => {
            "Rule-based diagnosis: escalation shares recorded causal lineage with failure."
        }
        _ => "Unfamiliar signature: bounded investigator proposal; no inquiry was launched.",
    };
    Ok(LocalProposal {
        scope: episode.scope.clone(),
        episode_digest: content_digest(episode)
            .map_err(|_| ExcursionError("cannot bind proposal episode"))?,
        title: "Investigate recorded excursion".into(),
        reproduction_contract: ReproductionContract { fields: clean },
        evidence_digests: digests.into_iter().collect(),
        uncertainty: if episode.unknown_attribution {
            "Attribution is unknown; temporal proximity does not establish cause.".into()
        } else {
            "Causal lineage is recorded; diagnosis remains rule-based and limited.".into()
        },
        diagnosis: known.into(),
        external_publication_allowed: false,
        execution_granted: false,
        proposed_investigator_max_steps: 8,
        proposed_investigator_max_model_turns: 1,
    })
}
fn permitted_field(key: &str) -> bool {
    matches!(
        key,
        "scope"
            | "steps"
            | "expected"
            | "actual"
            | "negation"
            | "failed_criteria"
            | "units"
            | "uncertainty"
            | "tried_approaches"
            | "candidate_fix"
            | "measurable_outcome"
    )
}
fn unsafe_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("://")
        || lower.contains("-----begin ")
        || [
            "password=",
            "token=",
            "api_key=",
            "authorization:",
            "secret=",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}
fn validate_digest(value: &str) -> Result<(), ExcursionError> {
    if let Some(hex) = value.strip_prefix("sha256:")
        && hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Ok(());
    }
    Err(ExcursionError(
        "evidence must be an immutable SHA-256 reference",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> ExcursionScope {
        ExcursionScope {
            project: ProjectId::new("p").expect("fixture project"),
            root: RootId::new("r").expect("fixture root"),
            task: TaskId::new("t").expect("fixture task"),
            contract_revision: 1,
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
        }
    }
    fn event(id: &str, parent: Option<&str>, kind: &str) -> IncidentEvent {
        IncidentEvent {
            event_id: id.into(),
            family_signature: "failure".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![],
            observed_tick: Some("1".into()),
        }
    }
    #[test]
    fn exact_replay_and_correlated_escalation_are_stable() {
        let s = scope();
        let a = ScopedIncident {
            scope: s.clone(),
            event: event("a", None, "failure"),
        };
        let one = reduce_excursions(&None, &s, std::slice::from_ref(&a)).expect("first incident");
        let replay =
            reduce_excursions(&Some(one.clone()), &s, std::slice::from_ref(&a)).expect("replay");
        assert_eq!(one, replay);
        let b = ScopedIncident {
            scope: s.clone(),
            event: event("b", Some("a"), "escalate"),
        };
        let both = reduce_excursions(&Some(one), &s, &[b]).expect("correlated escalation");
        assert_eq!(both.episodes.len(), 1);
        assert_eq!(both.episodes[0].events.len(), 2);
    }
    #[test]
    fn recurrence_unknown_cause_and_invalid_graph_are_bounded() {
        let s = scope();
        let a = ScopedIncident {
            scope: s.clone(),
            event: event("a", None, "failure"),
        };
        let one = reduce_excursions(&None, &s, &[a]).expect("first");
        let b = ScopedIncident {
            scope: s.clone(),
            event: event("b", None, "failure"),
        };
        let two = reduce_excursions(&Some(one), &s, &[b]).expect("independent recurrence");
        assert_eq!(two.episodes.len(), 2);
        assert!(two.episodes[0].unknown_attribution);
        let c = ScopedIncident {
            scope: s.clone(),
            event: event("c", Some("c"), "failure"),
        };
        assert!(reduce_excursions(&None, &s, &[c]).is_err());
        let foreign = ScopedIncident {
            scope: ExcursionScope {
                task: TaskId::new("other").expect("fixture task"),
                ..s.clone()
            },
            event: event("x", None, "failure"),
        };
        assert!(reduce_excursions(&None, &s, &[foreign]).is_err());
    }
    #[test]
    fn proposal_rejects_secrets_and_never_grants_publication() {
        let ep = reduce_excursions(
            &None,
            &scope(),
            &[ScopedIncident {
                scope: scope(),
                event: event("e", None, "failure"),
            }],
        )
        .expect("canonical episode")
        .episodes
        .remove(0);
        let good = ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "task t".into()),
                ("steps".into(), "run fixture".into()),
                ("expected".into(), "failed".into()),
                ("actual".into(), "still failed".into()),
                ("negation".into(), "must not publish".into()),
            ]),
        };
        let proposal = build_local_proposal(&ep, &good, &[]).expect("local proposal");
        assert!(!proposal.external_publication_allowed);
        assert!(proposal.uncertainty.contains("unknown"));
        let mut bad = good;
        bad.fields.insert("steps".into(), "token=secret".into());
        assert!(build_local_proposal(&ep, &bad, &[]).is_err());
    }
    #[test]
    fn stored_receipt_or_projection_forgery_cannot_hide_an_incident() {
        let s = scope();
        let row = ScopedIncident {
            scope: s.clone(),
            event: event("a", None, "failure"),
        };
        let good =
            reduce_excursions(&None, &s, std::slice::from_ref(&row)).expect("canonical state");
        let mut forged = good.clone();
        forged
            .receipts
            .insert("phantom".into(), format!("sha256:{}", "c".repeat(64)));
        assert!(reduce_excursions(&Some(forged), &s, &[]).is_err());
        let mut forged = good.clone();
        forged.episodes[0].events.clear();
        assert!(reduce_excursions(&Some(forged), &s, std::slice::from_ref(&row)).is_err());
        let mut forged = good;
        forged.episodes[0].scope.contract_revision = 2;
        assert!(reduce_excursions(&Some(forged), &s, &[]).is_err());
    }
    #[test]
    fn original_scope_digest_and_causal_clocks_are_exact() {
        let s = scope();
        let mut first = event("a", None, "failure");
        first.observed_tick = Some("10".into());
        let one = reduce_excursions(
            &None,
            &s,
            &[ScopedIncident {
                scope: s.clone(),
                event: first,
            }],
        )
        .expect("parent");
        let child = ScopedIncident {
            scope: s.clone(),
            event: event("b", Some("a"), "escalate"),
        };
        assert!(reduce_excursions(&Some(one.clone()), &s, &[child]).is_err());
        let mut other = s.clone();
        other.contract_revision = 2;
        assert!(reduce_excursions(&Some(one.clone()), &other, &[]).is_err());
        let mut bad = event("bad", None, "failure");
        bad.evidence_digests.push("sha256:not-immutable".into());
        assert!(
            reduce_excursions(
                &None,
                &s,
                &[ScopedIncident {
                    scope: s.clone(),
                    event: bad
                }]
            )
            .is_err()
        );
        let foreign = reduce_excursions(
            &None,
            &other,
            &[ScopedIncident {
                scope: other.clone(),
                event: event("a", None, "failure"),
            }],
        )
        .expect("separate scope");
        assert_ne!(one.episodes[0].episode_id, foreign.episodes[0].episode_id);
    }
    #[test]
    fn cumulative_state_limit_preserves_replay_without_unbounded_growth() {
        let s = scope();
        let rows: Vec<_> = (0..MAX_ITEMS)
            .map(|index| ScopedIncident {
                scope: s.clone(),
                event: event(&format!("event-{index}"), None, "failure"),
            })
            .collect();
        let full = reduce_excursions(&None, &s, &rows).expect("bounded complete state");
        assert_eq!(
            reduce_excursions(&Some(full.clone()), &s, &rows[..1]).expect("replay at capacity"),
            full
        );
        let more = ScopedIncident {
            scope: s.clone(),
            event: event("new", None, "failure"),
        };
        assert!(reduce_excursions(&Some(full), &s, &[more]).is_err());
    }
}
