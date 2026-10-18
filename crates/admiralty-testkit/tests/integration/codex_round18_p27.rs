#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Round 18 release-bundle and serialized-schema parity qualification for P27.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        ExcursionScope, ExcursionState, IncidentEvent, LocalProposal, ReproductionContract,
        ScopedIncident, build_local_proposal, reduce_excursions,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io::{Error as IoError, ErrorKind},
};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseBundle {
    schema_version: u32,
    state: ExcursionState,
    proposal: LocalProposal,
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn make_scope(task: &str) -> Result<ExcursionScope, Box<dyn Error>> {
    Ok(ExcursionScope {
        project: ProjectId::new("round18-project")?,
        root: RootId::new("round18-root")?,
        task: TaskId::new(task)?,
        contract_revision: 18,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    })
}

fn incident(
    scope: &ExcursionScope,
    id: &str,
    parent: Option<&str>,
    kind: &str,
    tick: u64,
) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: id.into(),
            family_signature: "round18-release-bundle".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.to_string()),
        },
    }
}

fn parse_bundle(bytes: &[u8]) -> Result<ReleaseBundle, Box<dyn Error>> {
    let bundle: ReleaseBundle = serde_json::from_slice(bytes)?;
    if bundle.schema_version != 1 {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            "unsupported release bundle schema version",
        )
        .into());
    }
    Ok(bundle)
}

fn keys(value: &serde_json::Value) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let object = value
        .as_object()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "schema node must be an object"))?;
    Ok(object.keys().cloned().collect())
}

#[test]
fn p27_round18_release_bundle_schema_parity() -> Result<(), Box<dyn Error>> {
    let scope = make_scope("release-task")?;
    let failure = incident(&scope, "failure-1", None, "failure", 1800);
    let first = reduce_excursions(&None, &scope, std::slice::from_ref(&failure))?;
    let escalation = incident(
        &scope,
        "escalation-1",
        Some("failure-1"),
        "escalation",
        1801,
    );
    let correlated = reduce_excursions(&Some(first), &scope, std::slice::from_ref(&escalation))?;
    let recurrence = incident(&scope, "failure-2", None, "failure", 1802);
    let quarantined = reduce_excursions(&Some(correlated), &scope, &[recurrence])?;
    assert_eq!(quarantined.episodes.len(), 2);
    assert!(
        quarantined
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );

    let proposal = build_local_proposal(
        &quarantined.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "release-task; revision 18".into()),
                (
                    "steps".into(),
                    "replay the fixed local release fixture".into(),
                ),
                (
                    "expected".into(),
                    "correlated escalation plus isolated recurrence".into(),
                ),
                ("actual".into(), "two bounded scoped episodes".into()),
                (
                    "negation".into(),
                    "no execution or external publication".into(),
                ),
            ]),
        },
        &[],
    )?;
    assert_eq!(proposal.scope, scope);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let bundle = ReleaseBundle {
        schema_version: 1,
        state: quarantined,
        proposal,
    };
    let serialized = serde_json::to_vec(&bundle)?;
    let value: serde_json::Value = serde_json::from_slice(&serialized)?;
    assert_eq!(
        keys(&value)?,
        BTreeSet::from([
            "proposal".to_owned(),
            "schema_version".to_owned(),
            "state".to_owned(),
        ])
    );
    assert_eq!(
        keys(&value["state"])?,
        BTreeSet::from([
            "episodes".to_owned(),
            "receipts".to_owned(),
            "scope".to_owned()
        ])
    );
    assert_eq!(
        keys(&value["state"]["episodes"][0])?,
        BTreeSet::from([
            "episode_id".to_owned(),
            "events".to_owned(),
            "evidence".to_owned(),
            "family".to_owned(),
            "lineage".to_owned(),
            "scope".to_owned(),
            "unknown_attribution".to_owned(),
        ])
    );
    assert_eq!(
        keys(&value["state"]["episodes"][0]["events"][0])?,
        BTreeSet::from([
            "cause_parent".to_owned(),
            "event_id".to_owned(),
            "evidence_digests".to_owned(),
            "family_signature".to_owned(),
            "kind".to_owned(),
            "observed_tick".to_owned(),
        ])
    );
    assert_eq!(
        keys(&value["proposal"])?,
        BTreeSet::from([
            "diagnosis".to_owned(),
            "episode_digest".to_owned(),
            "evidence_digests".to_owned(),
            "execution_granted".to_owned(),
            "external_publication_allowed".to_owned(),
            "proposed_investigator_max_model_turns".to_owned(),
            "proposed_investigator_max_steps".to_owned(),
            "reproduction_contract".to_owned(),
            "scope".to_owned(),
            "title".to_owned(),
            "uncertainty".to_owned(),
        ])
    );
    assert_eq!(
        keys(&value["proposal"]["reproduction_contract"])?,
        BTreeSet::from(["fields".to_owned()])
    );
    assert_eq!(parse_bundle(&serialized)?, bundle);

    // Schema drift and unknown fields reject the whole bundle; the reducer also
    // refuses a release state whose embedded scope crosses the packet boundary.
    let mut unsupported = value.clone();
    unsupported["schema_version"] = serde_json::Value::from(2);
    assert!(parse_bundle(&serde_json::to_vec(&unsupported)?).is_err());
    let mut extended = value.clone();
    extended["unexpected_authority"] = serde_json::Value::Bool(true);
    assert!(parse_bundle(&serde_json::to_vec(&extended)?).is_err());

    let foreign_scope = make_scope("foreign-release-task")?;
    assert!(reduce_excursions(&Some(bundle.state.clone()), &foreign_scope, &[]).is_err());
    let replay = reduce_excursions(&Some(bundle.state.clone()), &scope, &[])?;
    assert_eq!(replay, bundle.state);
    assert_eq!(serde_json::to_vec(&bundle)?, serialized);
    Ok(())
}
