//! Bounded, deterministic conflict risk analysis. This module proposes a frozen
//! generation; it does not perform Git operations or establish merge safety.
use crate::{ProjectId, RootId, TaskId, safe_relative_path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CANDIDATES: usize = 128;
pub const MAX_FILES_PER_CANDIDATE: usize = 1024;
pub const MAX_TOTAL_HUNKS: usize = 8192;
pub const MAX_SERIALIZED_BYTES: usize = 1024 * 1024;

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
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub path: String,
    /// None means the change is whole-file/rename-like, not an empty change.
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
    pub policy_revision: String,
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
}

fn digest_valid(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
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

fn validate(candidates: &[CandidateChange]) -> Result<(), ConflictError> {
    if serde_json::to_vec(candidates)
        .map_err(|_| ConflictError::TooLarge)?
        .len()
        > MAX_SERIALIZED_BYTES
    {
        return Err(ConflictError::TooLarge);
    }
    if candidates.len() > MAX_CANDIDATES {
        return Err(ConflictError::TooManyCandidates);
    }
    let mut hunk_count = 0usize;
    let mut identities = BTreeMap::new();
    for c in candidates {
        if !id_valid(&c.candidate_id)
            || !digest_valid(&c.base_sha)
            || !digest_valid(&c.head_sha)
            || !digest_valid(&c.tree_sha)
            || c.ancestry_digest.as_ref().is_some_and(|d| !digest_valid(d))
        {
            return Err(ConflictError::InvalidCandidate(c.candidate_id.clone()));
        }
        if c.files.len() > MAX_FILES_PER_CANDIDATE {
            return Err(ConflictError::TooManyFiles(c.candidate_id.clone()));
        }
        let encoded = serde_json::to_vec(c).map_err(|_| ConflictError::TooLarge)?;
        if identities
            .insert(c.candidate_id.as_str(), encoded.clone())
            .is_some_and(|prior| prior != encoded)
        {
            return Err(ConflictError::CandidateCollision(c.candidate_id.clone()));
        }
        let mut paths = BTreeSet::new();
        for f in &c.files {
            if !safe_relative_path(&f.path) || f.path.contains(':') {
                return Err(ConflictError::InvalidPath(f.path.clone()));
            }
            if !paths.insert(f.path.as_str()) {
                return Err(ConflictError::DuplicatePath(c.candidate_id.clone()));
            }
            if let Some(hunks) = &f.hunks {
                hunk_count = hunk_count
                    .checked_add(hunks.len())
                    .ok_or(ConflictError::TooManyHunks)?;
                if hunks.iter().any(|h| h.start == 0 || h.end < h.start) {
                    return Err(ConflictError::InvalidRange);
                }
            }
        }
    }
    if hunk_count > MAX_TOTAL_HUNKS {
        return Err(ConflictError::TooManyHunks);
    }
    Ok(())
}

fn paths_related(a: &str, b: &str) -> bool {
    a == b
        || a.strip_prefix(b).is_some_and(|s| s.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|s| s.starts_with('/'))
}
fn range_overlap(a: &HunkRange, b: &HunkRange) -> bool {
    a.start <= b.end && b.start <= a.end
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
    let mut sorted = candidates.to_vec();
    sorted.sort_by(|a, b| a.candidate_id.cmp(&b.candidate_id));
    let payload =
        serde_json::to_vec(&(sorted, policy_revision)).map_err(|_| ConflictError::TooLarge)?;
    Ok(format!("sha256:{:x}", Sha256::digest(payload)))
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
            if a.base_sha != b.base_sha {
                risks.insert(ConflictRisk::BaseMismatch);
            }
            for fa in &a.files {
                for fb in &b.files {
                    if paths_related(&fa.path, &fb.path) {
                        if fa.path != fb.path {
                            risks.insert(ConflictRisk::AncestorPath);
                        } else {
                            match (&fa.hunks, &fb.hunks) {
                                (Some(ha), Some(hb))
                                    if ha
                                        .iter()
                                        .any(|x| hb.iter().any(|y| range_overlap(x, y))) =>
                                {
                                    risks.insert(ConflictRisk::OverlappingHunks);
                                }
                                (Some(_), Some(_)) => {}
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
        policy_revision: policy_revision.into(),
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
}
