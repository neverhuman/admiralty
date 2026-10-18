//! Bounded, deterministic conflict risk analysis. This module proposes a frozen
//! generation; it does not perform Git operations or establish merge safety.
use crate::{ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CANDIDATES: usize = 128;
/// Bump whenever analysis/cache semantics change, independently of caller policy labels.
pub const ANALYSIS_VERSION: u32 = 2;
pub const MAX_FILES_PER_CANDIDATE: usize = 1024;
pub const MAX_TOTAL_HUNKS: usize = 8192;
pub const MAX_SERIALIZED_BYTES: usize = 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_PATH_COMPONENTS: usize = 128;
pub const MAX_INTERFACES_PER_CANDIDATE: usize = 128;
pub const MAX_PATH_COMPARISONS: usize = 1_000_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateChange {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub candidate_id: String,
    pub base_sha: String,
    pub head_sha: String,
    pub tree_sha: String,
    /// Missing ancestry is explicitly unknown and prevents a clear result.
    pub ancestry_digest: Option<String>,
    pub files: Vec<FileChange>,
    /// Declared task metadata, not a verified contract or write capability.
    #[serde(default)]
    pub constraints: Option<CandidateConstraints>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateConstraints {
    pub contract_revision: u32,
    pub contract_digest: String,
    pub owned_paths: Vec<String>,
    pub denied_paths: Vec<String>,
    pub interfaces: Vec<InterfaceRevision>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceRevision {
    pub interface_id: String,
    pub revision: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConflictScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub path: String,
    /// None or an empty list means whole-file/rename-like unknown hunk evidence.
    pub hunks: Option<Vec<HunkRange>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct HunkRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictRisk {
    OverlappingHunks,
    SamePathUnknownHunks,
    AncestorPath,
    UnknownAncestry,
    BaseMismatch,
    CaseAlias,
    UnknownWriteScope,
    OverlappingWriteScopes,
    WriteAgainstDenial,
    InterfaceRevisionMismatch,
    SameTaskCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConflictEdge {
    pub left: String,
    pub right: String,
    pub risks: Vec<ConflictRisk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenGeneration {
    pub candidate_ids: Vec<String>,
    pub combined_verification_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictGraph {
    pub analysis_version: u32,
    /// Empty input has no scope. Mixed project/root sets are rejected.
    pub scope: Option<ConflictScope>,
    pub policy_revision: String,
    pub source_digest: String,
    /// Missing declared constraints or ancestry stays visible even for one candidate.
    pub unresolved_candidates: Vec<String>,
    pub edges: Vec<ConflictEdge>,
    pub generation: FrozenGeneration,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConflictError {
    #[error("serialized candidates exceed 1 MiB")]
    TooLarge,
    #[error("candidate count exceeds 128")]
    TooManyCandidates,
    #[error("candidate {0} has invalid identifier or digest")]
    InvalidCandidate(String),
    #[error("candidate {0} exceeds 1024 files")]
    TooManyFiles(String),
    #[error("total hunk count exceeds 8192")]
    TooManyHunks,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("invalid ordered nonzero hunk range")]
    InvalidRange,
    #[error("duplicate file path in candidate {0}")]
    DuplicatePath(String),
    #[error("candidate ID has different content: {0}")]
    CandidateCollision(String),
    #[error("invalid policy revision")]
    InvalidPolicy,
    #[error("invalid candidate JSON: {0}")]
    Json(String),
    #[error("candidate {candidate_id} has a different project or root scope")]
    ScopeMismatch { candidate_id: String },
    #[error("invalid declared write/interface constraints for candidate {0}")]
    InvalidConstraints(String),
    #[error("candidate {candidate_id} changes a path outside its declared write scope: {path}")]
    OutsideWriteScope { candidate_id: String, path: String },
    #[error("candidate {candidate_id} changes a denied path: {path}")]
    DeniedWrite { candidate_id: String, path: String },
    #[error("conflict analysis exceeds the comparison budget")]
    TooComplex,
    #[error("task ID has different declared immutable contract identities: {0}")]
    ContractIdentityCollision(String),
}

fn digest_valid(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn object_id(value: &str) -> Option<String> {
    let (algorithm, hex) = if let Some(hex) = value.strip_prefix("sha1:") {
        ("sha1", hex)
    } else if let Some(hex) = value.strip_prefix("sha256:") {
        ("sha256", hex)
    } else if value.len() == 40 {
        ("sha1", value)
    } else {
        ("sha256", value)
    };
    let length = if algorithm == "sha1" { 40 } else { 64 };
    (hex.len() == length && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("{algorithm}:{}", hex.to_ascii_lowercase()))
}
fn id_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && value != "."
        && value != ".."
}

/// Decode and validate without commands, filesystem access or authority changes.
/// File/body readers must bound their reads before creating this byte slice.
pub fn parse_candidates(bytes: &[u8]) -> Result<Vec<CandidateChange>, ConflictError> {
    if bytes.len() > MAX_SERIALIZED_BYTES {
        return Err(ConflictError::TooLarge);
    }
    let candidates: Vec<CandidateChange> =
        serde_json::from_slice(bytes).map_err(|error| ConflictError::Json(error.to_string()))?;
    validate(&candidates)?;
    Ok(candidates)
}

fn validate(candidates: &[CandidateChange]) -> Result<(), ConflictError> {
    if candidates.len() > MAX_CANDIDATES {
        return Err(ConflictError::TooManyCandidates);
    }
    // Also bound already-typed callers, without allocating a complete JSON copy.
    bounded_json_size(candidates, MAX_SERIALIZED_BYTES).map_err(|_| ConflictError::TooLarge)?;
    check_comparison_budget(candidates)?;
    let mut hunk_count = 0usize;
    let mut identities: BTreeMap<&str, &CandidateChange> = BTreeMap::new();
    let mut contract_identities = BTreeMap::new();
    for c in candidates {
        if !id_valid(&c.candidate_id)
            || object_id(&c.base_sha).is_none()
            || object_id(&c.head_sha).is_none()
            || object_id(&c.tree_sha).is_none()
            || c.ancestry_digest.as_ref().is_some_and(|d| !digest_valid(d))
        {
            return Err(ConflictError::InvalidCandidate(c.candidate_id.clone()));
        }
        if candidates
            .first()
            .is_some_and(|first| c.project_id != first.project_id || c.root_id != first.root_id)
        {
            return Err(ConflictError::ScopeMismatch {
                candidate_id: c.candidate_id.clone(),
            });
        }
        if c.files.len() > MAX_FILES_PER_CANDIDATE {
            return Err(ConflictError::TooManyFiles(c.candidate_id.clone()));
        }
        if identities
            .insert(c.candidate_id.as_str(), c)
            .is_some_and(|prior| prior != c)
        {
            return Err(ConflictError::CandidateCollision(c.candidate_id.clone()));
        }
        let mut paths = BTreeSet::new();
        for f in &c.files {
            if !path_valid(&f.path, false) {
                return Err(ConflictError::InvalidPath(f.path.clone()));
            }
            if !paths.insert(f.path.to_ascii_lowercase()) {
                return Err(ConflictError::DuplicatePath(c.candidate_id.clone()));
            }
            if let Some(hunks) = &f.hunks {
                hunk_count = hunk_count
                    .checked_add(hunks.len())
                    .ok_or(ConflictError::TooManyHunks)?;
                if hunk_count > MAX_TOTAL_HUNKS {
                    return Err(ConflictError::TooManyHunks);
                }
                if hunks.iter().any(|h| h.start == 0 || h.end < h.start)
                    || hunks.windows(2).any(|pair| pair[0].end >= pair[1].start)
                {
                    return Err(ConflictError::InvalidRange);
                }
            }
        }
        if let Some(constraints) = &c.constraints {
            validate_constraints(c, constraints)?;
            let identity = (
                constraints.contract_revision,
                &constraints.contract_digest,
                &constraints.owned_paths,
                &constraints.denied_paths,
            );
            if contract_identities
                .insert(&c.task_id, identity)
                .is_some_and(|previous| previous != identity)
            {
                return Err(ConflictError::ContractIdentityCollision(
                    c.task_id.to_string(),
                ));
            }
        }
    }
    if hunk_count > MAX_TOTAL_HUNKS {
        return Err(ConflictError::TooManyHunks);
    }
    Ok(())
}

fn check_comparison_budget(candidates: &[CandidateChange]) -> Result<(), ConflictError> {
    let mut comparisons = 0usize;
    let mut add = |left: usize, right: usize| -> Result<(), ConflictError> {
        comparisons = left
            .checked_mul(right)
            .and_then(|amount| comparisons.checked_add(amount))
            .filter(|total| *total <= MAX_PATH_COMPARISONS)
            .ok_or(ConflictError::TooComplex)?;
        Ok(())
    };
    for (index, a) in candidates.iter().enumerate() {
        if let Some(ca) = &a.constraints {
            add(a.files.len(), ca.owned_paths.len())?;
            add(a.files.len(), ca.denied_paths.len())?;
        }
        for b in candidates.iter().skip(index + 1) {
            // Exact repeats do not add graph work after identity validation.
            if a.candidate_id == b.candidate_id {
                continue;
            }
            add(a.files.len(), b.files.len())?;
            if let (Some(ca), Some(cb)) = (&a.constraints, &b.constraints) {
                add(ca.owned_paths.len(), cb.owned_paths.len())?;
                add(ca.interfaces.len(), cb.interfaces.len())?;
            }
            for (writer, guarded) in [(a, b), (b, a)] {
                if let Some(guard) = &guarded.constraints {
                    add(
                        writer
                            .constraints
                            .as_ref()
                            .map_or(writer.files.len(), |c| c.owned_paths.len()),
                        guard.denied_paths.len(),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn path_valid(path: &str, allow_root: bool) -> bool {
    if path == "." {
        return allow_root;
    }
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && path.is_ascii()
        && !path.chars().any(char::is_control)
        && !path.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
        && path.split('/').count() <= MAX_PATH_COMPONENTS
        && path.split('/').all(|part| {
            let base = part
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_end_matches(' ')
                .to_ascii_uppercase();
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !((base.starts_with("COM") || base.starts_with("LPT"))
                    && base.len() == 4
                    && matches!(base.as_bytes()[3], b'1'..=b'9'))
        })
}

fn validate_constraints(
    candidate: &CandidateChange,
    constraints: &CandidateConstraints,
) -> Result<(), ConflictError> {
    if constraints.contract_revision == 0
        || !digest_valid(&constraints.contract_digest)
        || constraints.owned_paths.is_empty()
        || constraints.owned_paths.len() > MAX_FILES_PER_CANDIDATE
        || constraints.denied_paths.len() > MAX_FILES_PER_CANDIDATE
        || constraints.interfaces.len() > MAX_INTERFACES_PER_CANDIDATE
    {
        return Err(ConflictError::InvalidConstraints(
            candidate.candidate_id.clone(),
        ));
    }
    for paths in [&constraints.owned_paths, &constraints.denied_paths] {
        let mut seen = BTreeSet::new();
        for path in paths {
            if !path_valid(path, true) || !seen.insert(path.to_ascii_lowercase()) {
                return Err(ConflictError::InvalidConstraints(
                    candidate.candidate_id.clone(),
                ));
            }
        }
    }
    let mut interfaces = BTreeSet::new();
    for interface in &constraints.interfaces {
        if !id_valid(&interface.interface_id)
            || interface.revision == 0
            || !interfaces.insert(&interface.interface_id)
        {
            return Err(ConflictError::InvalidConstraints(
                candidate.candidate_id.clone(),
            ));
        }
    }
    for file in &candidate.files {
        if constraints
            .denied_paths
            .iter()
            .any(|path| paths_related(path, &file.path))
        {
            return Err(ConflictError::DeniedWrite {
                candidate_id: candidate.candidate_id.clone(),
                path: file.path.clone(),
            });
        }
        if !constraints
            .owned_paths
            .iter()
            .any(|path| path_contains(path, &file.path))
        {
            return Err(ConflictError::OutsideWriteScope {
                candidate_id: candidate.candidate_id.clone(),
                path: file.path.clone(),
            });
        }
    }
    Ok(())
}

fn path_contains(scope: &str, path: &str) -> bool {
    scope == "."
        || scope.eq_ignore_ascii_case(path)
        || (path
            .get(..scope.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scope))
            && path.as_bytes().get(scope.len()) == Some(&b'/'))
}
fn paths_related(a: &str, b: &str) -> bool {
    path_contains(a, b) || path_contains(b, a)
}
fn hunks_overlap(a: &[HunkRange], b: &[HunkRange]) -> bool {
    let (mut left, mut right) = (0, 0);
    while let (Some(a), Some(b)) = (a.get(left), b.get(right)) {
        if a.start <= b.end && b.start <= a.end {
            return true;
        }
        if a.end < b.start {
            left += 1;
        } else {
            right += 1;
        }
    }
    false
}

/// Stable identity for probe/cache use, binding the full candidate identities and policy.
pub fn probe_cache_key(
    candidates: &[CandidateChange],
    policy_revision: &str,
) -> Result<String, ConflictError> {
    validate(candidates)?;
    if !id_valid(policy_revision) {
        return Err(ConflictError::InvalidPolicy);
    }
    cache_key(candidates, policy_revision)
}

fn cache_key(
    candidates: &[CandidateChange],
    policy_revision: &str,
) -> Result<String, ConflictError> {
    let unique: BTreeMap<_, _> = candidates.iter().map(|c| (&c.candidate_id, c)).collect();
    content_digest(&(
        ANALYSIS_VERSION,
        unique.into_values().collect::<Vec<_>>(),
        policy_revision,
    ))
    .map_err(|_| ConflictError::TooLarge)
}

pub fn analyze_conflicts(
    candidates: &[CandidateChange],
    policy_revision: &str,
) -> Result<ConflictGraph, ConflictError> {
    validate(candidates)?;
    if !id_valid(policy_revision) {
        return Err(ConflictError::InvalidPolicy);
    }
    let mut unique = BTreeMap::<String, CandidateChange>::new();
    for c in candidates {
        unique
            .entry(c.candidate_id.clone())
            .or_insert_with(|| c.clone());
    }
    let values: Vec<_> = unique.into_values().collect();
    let mut edges = Vec::new();
    for (i, a) in values.iter().enumerate() {
        for b in values.iter().skip(i + 1) {
            let mut risks = BTreeSet::new();
            if a.ancestry_digest.is_none() || b.ancestry_digest.is_none() {
                risks.insert(ConflictRisk::UnknownAncestry);
            }
            if object_id(&a.base_sha) != object_id(&b.base_sha) {
                risks.insert(ConflictRisk::BaseMismatch);
            }
            if a.task_id == b.task_id {
                risks.insert(ConflictRisk::SameTaskCandidate);
            }
            match (&a.constraints, &b.constraints) {
                (Some(ca), Some(cb)) => {
                    if ca
                        .owned_paths
                        .iter()
                        .any(|pa| cb.owned_paths.iter().any(|pb| paths_related(pa, pb)))
                    {
                        risks.insert(ConflictRisk::OverlappingWriteScopes);
                    }
                    if ca.interfaces.iter().any(|ia| {
                        cb.interfaces.iter().any(|ib| {
                            ia.interface_id == ib.interface_id && ia.revision != ib.revision
                        })
                    }) {
                        risks.insert(ConflictRisk::InterfaceRevisionMismatch);
                    }
                }
                _ => {
                    risks.insert(ConflictRisk::UnknownWriteScope);
                }
            }
            for (writer, guarded) in [(a, b), (b, a)] {
                if let Some(guard) = &guarded.constraints {
                    let writes: Vec<_> = writer
                        .constraints
                        .as_ref()
                        .map(|c| c.owned_paths.iter().map(String::as_str).collect())
                        .unwrap_or_else(|| writer.files.iter().map(|f| f.path.as_str()).collect());
                    if writes.iter().any(|write| {
                        guard
                            .denied_paths
                            .iter()
                            .any(|denied| paths_related(write, denied))
                    }) {
                        risks.insert(ConflictRisk::WriteAgainstDenial);
                    }
                }
            }
            for fa in &a.files {
                for fb in &b.files {
                    if paths_related(&fa.path, &fb.path) {
                        if !fa.path.eq_ignore_ascii_case(&fb.path) {
                            risks.insert(ConflictRisk::AncestorPath);
                        } else if fa.path != fb.path {
                            risks.insert(ConflictRisk::CaseAlias);
                            risks.insert(ConflictRisk::SamePathUnknownHunks);
                        } else {
                            match (&fa.hunks, &fb.hunks) {
                                (Some(ha), Some(hb))
                                    if !ha.is_empty()
                                        && !hb.is_empty()
                                        && hunks_overlap(ha, hb) =>
                                {
                                    risks.insert(ConflictRisk::OverlappingHunks);
                                }
                                (Some(ha), Some(hb)) if !ha.is_empty() && !hb.is_empty() => {}
                                _ => {
                                    risks.insert(ConflictRisk::SamePathUnknownHunks);
                                }
                            }
                        }
                    }
                }
            }
            if !risks.is_empty() {
                edges.push(ConflictEdge {
                    left: a.candidate_id.clone(),
                    right: b.candidate_id.clone(),
                    risks: risks.into_iter().collect(),
                });
            }
        }
    }
    edges.sort();
    Ok(ConflictGraph {
        analysis_version: ANALYSIS_VERSION,
        scope: values.first().map(|candidate| ConflictScope {
            project_id: candidate.project_id.clone(),
            root_id: candidate.root_id.clone(),
        }),
        policy_revision: policy_revision.into(),
        source_digest: cache_key(candidates, policy_revision)?,
        unresolved_candidates: values
            .iter()
            .filter(|c| c.constraints.is_none() || c.ancestry_digest.is_none())
            .map(|c| c.candidate_id.clone())
            .collect(),
        edges,
        generation: FrozenGeneration {
            candidate_ids: values.iter().map(|c| c.candidate_id.clone()).collect(),
            combined_verification_required: values.len() > 1,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }
    fn candidate(
        id: &str,
        base: char,
        ancestry: Option<char>,
        files: Vec<FileChange>,
    ) -> CandidateChange {
        CandidateChange {
            project_id: ProjectId::new("p").expect("project id"),
            root_id: RootId::new("r").expect("root id"),
            task_id: TaskId::new(id).expect("task id"),
            candidate_id: id.into(),
            base_sha: d(base),
            head_sha: d('b'),
            tree_sha: d('c'),
            ancestry_digest: ancestry.map(d),
            files,
            constraints: None,
        }
    }
    fn file(path: &str, hunks: Option<Vec<(u32, u32)>>) -> FileChange {
        FileChange {
            path: path.into(),
            hunks: hunks.map(|v| {
                v.into_iter()
                    .map(|(start, end)| HunkRange { start, end })
                    .collect()
            }),
        }
    }
    #[test]
    fn overlap_disjoint_and_path_boundaries_are_distinguished() {
        let a = candidate(
            "a",
            'a',
            Some('d'),
            vec![file("src/a.rs", Some(vec![(1, 3)])), file("src", None)],
        );
        let b = candidate(
            "b",
            'a',
            Some('d'),
            vec![
                file("src/a.rs", Some(vec![(3, 4)])),
                file("src2/a.rs", None),
            ],
        );
        let g = analyze_conflicts(&[b, a], "policy1").expect("analysis");
        assert!(
            g.edges
                .iter()
                .any(|e| e.risks.contains(&ConflictRisk::OverlappingHunks))
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e.risks.contains(&ConflictRisk::AncestorPath))
        );
        assert_eq!(g.generation.candidate_ids, vec!["a", "b"]);
    }
    #[test]
    fn unknown_base_and_ancestry_are_risks_and_cache_binds_ancestry() {
        let a = candidate("a", 'a', None, vec![]);
        let b = candidate("b", 'b', Some('d'), vec![]);
        let g = analyze_conflicts(&[a.clone(), b], "policy1").expect("analysis");
        assert!(g.edges[0].risks.contains(&ConflictRisk::BaseMismatch));
        assert!(g.edges[0].risks.contains(&ConflictRisk::UnknownAncestry));
        let mut other = a.clone();
        other.ancestry_digest = Some(d('e'));
        assert_ne!(
            probe_cache_key(&[a], "policy1"),
            probe_cache_key(&[other], "policy1")
        );
    }
    #[test]
    fn three_candidates_require_combined_verification_and_input_order_is_irrelevant() {
        let cs = (b'a'..=b'c')
            .map(|x| candidate(&(x as char).to_string(), 'a', Some('d'), vec![]))
            .collect::<Vec<_>>();
        let mut rev = cs.clone();
        rev.reverse();
        let a = analyze_conflicts(&cs, "policy1").expect("analysis");
        let b = analyze_conflicts(&rev, "policy1").expect("analysis");
        assert_eq!(a, b);
        assert!(a.generation.combined_verification_required);
    }
    #[test]
    fn rejects_collisions_invalid_sha_ranges_paths_and_caps() {
        let a = candidate("a", 'a', Some('d'), vec![file("../bad", None)]);
        assert!(matches!(
            analyze_conflicts(&[a], "p"),
            Err(ConflictError::InvalidPath(_))
        ));
        let bad = candidate("a", 'a', Some('d'), vec![file("x", Some(vec![(0, 1)]))]);
        assert_eq!(
            analyze_conflicts(&[bad], "p"),
            Err(ConflictError::InvalidRange)
        );
        let mut bad = candidate("a", 'a', Some('d'), vec![]);
        bad.head_sha = "bad".into();
        assert!(matches!(
            analyze_conflicts(&[bad], "p"),
            Err(ConflictError::InvalidCandidate(_))
        ));
        let one = candidate("a", 'a', Some('d'), vec![]);
        let mut changed = one.clone();
        changed.head_sha = d('e');
        assert!(matches!(
            analyze_conflicts(&[one.clone(), changed], "p"),
            Err(ConflictError::CandidateCollision(_))
        ));
        assert!(analyze_conflicts(&[one.clone(), one], "p").is_ok());
        let large = (0..=MAX_CANDIDATES)
            .map(|i| candidate(&format!("c{i}"), 'a', Some('d'), vec![]))
            .collect::<Vec<_>>();
        assert_eq!(
            analyze_conflicts(&large, "p"),
            Err(ConflictError::TooManyCandidates)
        );
    }

    fn scoped(mut candidate: CandidateChange, owned: &[&str], denied: &[&str]) -> CandidateChange {
        candidate.constraints = Some(CandidateConstraints {
            contract_revision: 1,
            contract_digest: d('d'),
            owned_paths: owned.iter().map(|path| (*path).into()).collect(),
            denied_paths: denied.iter().map(|path| (*path).into()).collect(),
            interfaces: vec![],
        });
        candidate
    }

    #[test]
    fn rejects_portable_path_aliases_and_case_duplicates_in_one_candidate() {
        for path in [
            ".",
            "./src/a",
            "src/./a",
            "src//a",
            "src/",
            "/src/a",
            "src/../a",
            "a\\b",
            "C:a",
            "a\nb",
            "a\u{7f}b",
            "a/résumé",
            "a/*",
            "a/child.",
            "a/child ",
            "a/CON.txt",
            "a/lpt9",
        ] {
            assert!(
                matches!(
                    analyze_conflicts(
                        &[candidate("a", 'a', Some('d'), vec![file(path, None)])],
                        "p"
                    ),
                    Err(ConflictError::InvalidPath(_))
                ),
                "accepted {path:?}"
            );
        }
        for path in [
            "x".repeat(MAX_PATH_BYTES + 1),
            vec!["a"; MAX_PATH_COMPONENTS + 1].join("/"),
        ] {
            assert!(matches!(
                analyze_conflicts(
                    &[candidate("a", 'a', Some('d'), vec![file(&path, None)])],
                    "p"
                ),
                Err(ConflictError::InvalidPath(_))
            ));
        }
        let repeated = candidate(
            "a",
            'a',
            Some('d'),
            vec![file("Src/a.rs", None), file("src/A.rs", None)],
        );
        assert_eq!(
            analyze_conflicts(&[repeated], "p"),
            Err(ConflictError::DuplicatePath("a".into()))
        );
        let a = candidate(
            "a",
            'a',
            Some('d'),
            vec![file("Src/A.rs", Some(vec![(1, 1)]))],
        );
        let b = candidate(
            "b",
            'a',
            Some('d'),
            vec![file("src/a.rs", Some(vec![(10, 10)]))],
        );
        let graph = analyze_conflicts(&[a, b], "p").expect("case aliases are risks");
        assert!(graph.edges[0].risks.contains(&ConflictRisk::CaseAlias));
        assert!(
            graph.edges[0]
                .risks
                .contains(&ConflictRisk::SamePathUnknownHunks)
        );
    }

    #[test]
    fn rejects_mixed_project_root_graphs_and_supports_real_git_object_ids() {
        let a = candidate("a", 'a', Some('d'), vec![]);
        let mut other = candidate("b", 'a', Some('d'), vec![]);
        other.project_id = ProjectId::new("other").expect("project");
        assert!(matches!(
            analyze_conflicts(&[a.clone(), other.clone()], "p"),
            Err(ConflictError::ScopeMismatch { .. })
        ));
        other.project_id = a.project_id.clone();
        other.root_id = RootId::new("other").expect("root");
        assert!(matches!(
            analyze_conflicts(&[a.clone(), other.clone()], "p"),
            Err(ConflictError::ScopeMismatch { .. })
        ));

        other.root_id = a.root_id.clone();
        let mut sha1 = a;
        sha1.base_sha = "a".repeat(40);
        sha1.head_sha = "b".repeat(40);
        sha1.tree_sha = "c".repeat(40);
        other.base_sha = format!("sha1:{}", "A".repeat(40));
        let graph =
            analyze_conflicts(&[sha1, other], "p").expect("native SHA1 and SHA256 identities");
        assert!(!graph.edges[0].risks.contains(&ConflictRisk::BaseMismatch));
        assert_eq!(
            graph.scope,
            Some(ConflictScope {
                project_id: ProjectId::new("p").expect("project"),
                root_id: RootId::new("r").expect("root")
            })
        );
    }

    #[test]
    fn declared_scope_denials_and_component_boundaries_fail_closed() {
        let a = scoped(
            candidate("a", 'a', Some('d'), vec![file("src/a.rs", None)]),
            &["src"],
            &[],
        );
        let b = scoped(
            candidate("b", 'a', Some('d'), vec![file("src2/b.rs", None)]),
            &["src2"],
            &[],
        );
        assert!(
            analyze_conflicts(&[a.clone(), b], "p")
                .expect("sibling scopes")
                .edges
                .is_empty()
        );
        let overlap = scoped(
            candidate("b", 'a', Some('d'), vec![file("src/b.rs", None)]),
            &["src"],
            &[],
        );
        assert!(
            analyze_conflicts(&[a.clone(), overlap], "p")
                .expect("scope overlap")
                .edges[0]
                .risks
                .contains(&ConflictRisk::OverlappingWriteScopes)
        );

        let outside = scoped(
            candidate("a", 'a', Some('d'), vec![file("src2/a.rs", None)]),
            &["src"],
            &[],
        );
        assert!(matches!(
            analyze_conflicts(&[outside], "p"),
            Err(ConflictError::OutsideWriteScope { .. })
        ));
        for (changed, denied) in [
            ("SRC/private/key", "src/private"),
            ("src", "src/private"),
            ("src/a.rs", "."),
        ] {
            let denied = scoped(
                candidate("a", 'a', Some('d'), vec![file(changed, None)]),
                &["."],
                &[denied],
            );
            assert!(matches!(
                analyze_conflicts(&[denied], "p"),
                Err(ConflictError::DeniedWrite { .. })
            ));
        }
        let guard = scoped(
            candidate("guard", 'a', Some('d'), vec![file("docs/readme.md", None)]),
            &["docs"],
            &["src/private"],
        );
        assert!(
            analyze_conflicts(&[a, guard], "p")
                .expect("cross-candidate denial risk")
                .edges[0]
                .risks
                .contains(&ConflictRisk::WriteAgainstDenial)
        );
    }

    #[test]
    fn interface_and_contract_revisions_are_bound_without_becoming_authority() {
        let mut a = scoped(
            candidate("a", 'a', Some('d'), vec![file("a.rs", None)]),
            &["a.rs"],
            &[],
        );
        let mut b = scoped(
            candidate("b", 'a', Some('d'), vec![file("b.rs", None)]),
            &["b.rs"],
            &[],
        );
        a.constraints.as_mut().expect("constraints").interfaces = vec![InterfaceRevision {
            interface_id: "shared-api".into(),
            revision: 1,
        }];
        b.constraints.as_mut().expect("constraints").interfaces = vec![InterfaceRevision {
            interface_id: "shared-api".into(),
            revision: 2,
        }];
        let graph = analyze_conflicts(&[a.clone(), b.clone()], "p").expect("interface risk");
        assert_eq!(
            graph.edges[0].risks,
            [ConflictRisk::InterfaceRevisionMismatch]
        );
        let first_key = probe_cache_key(&[a.clone(), b.clone()], "p").expect("key");
        b.constraints.as_mut().expect("constraints").interfaces[0].revision = 1;
        assert_ne!(
            first_key,
            probe_cache_key(&[a.clone(), b.clone()], "p").expect("changed interface key")
        );
        b.task_id = a.task_id.clone();
        b.constraints.as_mut().expect("constraints").owned_paths =
            vec!["a.rs".into(), "b.rs".into()];
        assert!(matches!(
            analyze_conflicts(&[a.clone(), b.clone()], "p"),
            Err(ConflictError::ContractIdentityCollision(_))
        ));
        b.constraints = a.constraints.clone();
        b.files = a.files.clone();
        assert!(
            analyze_conflicts(&[a.clone(), b.clone()], "p")
                .expect("two attempts of one task")
                .edges[0]
                .risks
                .contains(&ConflictRisk::SameTaskCandidate)
        );
        b.constraints
            .as_mut()
            .expect("constraints")
            .contract_revision = 2;
        assert!(matches!(
            analyze_conflicts(&[a, b], "p"),
            Err(ConflictError::ContractIdentityCollision(_))
        ));
    }

    #[test]
    fn empty_hunks_are_unknown_and_noncanonical_ranges_are_rejected() {
        let a = candidate("a", 'a', Some('d'), vec![file("same.rs", Some(vec![]))]);
        let b = candidate(
            "b",
            'a',
            Some('d'),
            vec![file("same.rs", Some(vec![(10, 20)]))],
        );
        assert!(
            analyze_conflicts(&[a, b], "p")
                .expect("empty hunk evidence stays unknown")
                .edges[0]
                .risks
                .contains(&ConflictRisk::SamePathUnknownHunks)
        );
        for ranges in [
            vec![(2, 3), (1, 1)],
            vec![(1, 3), (3, 4)],
            vec![(1, 1), (1, 1)],
            vec![(4, 3)],
        ] {
            assert_eq!(
                analyze_conflicts(
                    &[candidate(
                        "a",
                        'a',
                        Some('d'),
                        vec![file("same.rs", Some(ranges))]
                    )],
                    "p"
                ),
                Err(ConflictError::InvalidRange)
            );
        }
        assert!(hunks_overlap(
            &[
                HunkRange { start: 1, end: 2 },
                HunkRange { start: 8, end: 10 }
            ],
            &[
                HunkRange { start: 3, end: 4 },
                HunkRange {
                    start: 10,
                    end: u32::MAX
                }
            ]
        ));
        assert!(!hunks_overlap(
            &[
                HunkRange { start: 1, end: 2 },
                HunkRange { start: 8, end: 10 }
            ],
            &[
                HunkRange { start: 3, end: 4 },
                HunkRange {
                    start: 11,
                    end: u32::MAX
                }
            ]
        ));
    }

    #[test]
    fn cache_identity_deduplicates_replay_and_unknowns_are_visible_for_singletons() {
        let a = candidate("a", 'a', Some('d'), vec![file("a.rs", None)]);
        let b = candidate("b", 'a', Some('d'), vec![file("b.rs", None)]);
        let inputs = vec![b.clone(), a.clone(), a.clone()];
        let before = inputs.clone();
        let graph = analyze_conflicts(&inputs, "p").expect("repeated identities");
        assert_eq!(
            graph,
            analyze_conflicts(&[a.clone(), b.clone()], "p").expect("deduplicated graph")
        );
        assert_eq!(
            graph.source_digest,
            probe_cache_key(&inputs, "p").expect("cache identity")
        );
        assert_eq!(inputs, before);
        assert!(
            graph.edges[0]
                .risks
                .contains(&ConflictRisk::UnknownWriteScope)
        );
        assert_eq!(
            analyze_conflicts(&[a], "p")
                .expect("missing context")
                .unresolved_candidates,
            ["a"]
        );
        let absent = scoped(candidate("unknown", 'a', None, vec![]), &["."], &[]);
        assert_eq!(
            analyze_conflicts(&[absent], "p")
                .expect("missing ancestry")
                .unresolved_candidates,
            ["unknown"]
        );
        let empty = analyze_conflicts(&[], "p").expect("empty analysis");
        assert_eq!(empty.scope, None);
        assert!(empty.generation.candidate_ids.is_empty());
        assert_ne!(
            probe_cache_key(std::slice::from_ref(&b), "p"),
            probe_cache_key(&[b], "p2")
        );
    }

    #[test]
    fn parsers_and_checked_work_budgets_reject_unbounded_or_ambiguous_input() {
        let base = candidate("a", 'a', Some('d'), vec![]);
        let mut bytes = serde_json::to_vec(std::slice::from_ref(&base)).expect("JSON");
        bytes.resize(MAX_SERIALIZED_BYTES, b' ');
        assert_eq!(
            parse_candidates(&bytes)
                .expect("exact input bound")
                .as_slice(),
            std::slice::from_ref(&base)
        );
        bytes.push(b' ');
        assert_eq!(parse_candidates(&bytes), Err(ConflictError::TooLarge));
        let mut large = base.clone();
        large.files = vec![file(&"x".repeat(MAX_SERIALIZED_BYTES), None)];
        assert_eq!(
            analyze_conflicts(&[large], "p"),
            Err(ConflictError::TooLarge)
        );
        let mut extra = serde_json::to_value(std::slice::from_ref(&base)).expect("value");
        extra[0]["acceptance_granted"] = true.into();
        assert!(matches!(
            parse_candidates(&serde_json::to_vec(&extra).expect("JSON")),
            Err(ConflictError::Json(_))
        ));
        let duplicate = serde_json::to_string(&[base]).expect("JSON").replacen(
            "{",
            "{\"candidate_id\":\"injected\",",
            1,
        );
        assert!(matches!(
            parse_candidates(duplicate.as_bytes()),
            Err(ConflictError::Json(_))
        ));
        let expensive: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|id| {
                candidate(
                    id,
                    'a',
                    Some('d'),
                    (0..600)
                        .map(|index| file(&format!("path-{index}"), None))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            analyze_conflicts(&expensive, "p"),
            Err(ConflictError::TooComplex)
        );
    }

    #[test]
    fn file_hunk_interface_and_scope_metadata_caps_are_enforced() {
        let files = candidate(
            "a",
            'a',
            Some('d'),
            (0..=MAX_FILES_PER_CANDIDATE)
                .map(|index| file(&format!("path-{index}"), None))
                .collect(),
        );
        assert_eq!(
            analyze_conflicts(&[files], "p"),
            Err(ConflictError::TooManyFiles("a".into()))
        );
        let hunks = candidate(
            "a",
            'a',
            Some('d'),
            vec![file(
                "a.rs",
                Some(
                    (1..=MAX_TOTAL_HUNKS as u32 + 1)
                        .map(|line| (line, line))
                        .collect(),
                ),
            )],
        );
        assert_eq!(
            analyze_conflicts(&[hunks], "p"),
            Err(ConflictError::TooManyHunks)
        );
        let base = scoped(candidate("a", 'a', Some('d'), vec![]), &["."], &[]);
        let mut many = base.clone();
        many.constraints.as_mut().expect("constraints").interfaces = (0
            ..=MAX_INTERFACES_PER_CANDIDATE)
            .map(|index| InterfaceRevision {
                interface_id: format!("interface-{index}"),
                revision: 1,
            })
            .collect();
        assert!(matches!(
            analyze_conflicts(&[many], "p"),
            Err(ConflictError::InvalidConstraints(_))
        ));
        let mut repeated = base.clone();
        repeated
            .constraints
            .as_mut()
            .expect("constraints")
            .owned_paths = vec!["Src".into(), "src".into()];
        assert!(matches!(
            analyze_conflicts(&[repeated], "p"),
            Err(ConflictError::InvalidConstraints(_))
        ));
        let mut zero = base.clone();
        zero.constraints.as_mut().expect("constraints").interfaces = vec![InterfaceRevision {
            interface_id: "api".into(),
            revision: 0,
        }];
        assert!(matches!(
            analyze_conflicts(&[zero], "p"),
            Err(ConflictError::InvalidConstraints(_))
        ));
        assert_eq!(
            analyze_conflicts(std::slice::from_ref(&base), "../policy"),
            Err(ConflictError::InvalidPolicy)
        );
        let mut extra = serde_json::to_value(&[base]).expect("JSON");
        extra[0]["constraints"]["may_accept"] = true.into();
        assert!(matches!(
            parse_candidates(&serde_json::to_vec(&extra).expect("JSON")),
            Err(ConflictError::Json(_))
        ));
    }
}
