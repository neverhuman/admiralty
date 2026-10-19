//! Exclusive, durable publication boundary. Worker proposals carry no credentials.
use admiralty_core::{ProjectId, publication::*};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const JOURNAL_LIMIT: u64 = 16 * 1024 * 1024;
const ENTRY_LIMIT: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayPolicy {
    pub project_id: ProjectId,
    pub repository: String,
    pub target_ref: String,
    pub authority_generation: DecimalU64,
    pub required_reviewers: Vec<String>,
    pub required_checks: Vec<String>,
    pub owner_only: bool,
    pub owners: Vec<String>,
}

/// Rebase is the sole supported method. There is no admin/force/auto-merge mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRequest {
    pub intent_key: String,
    pub pull_request: u64,
    pub scope: PublicationScope,
    pub expected_head: String,
    pub expected_base: String,
}

#[derive(Clone, Debug)]
pub struct ForgeObservation {
    pub repository: String,
    pub target_ref: String,
    pub pull_request: u64,
    pub head: String,
    pub base: String,
    pub open: bool,
    pub approvals: Vec<ApprovalEvidence>,
    pub checks: Vec<CheckEvidence>,
}

/// Only an adapter with authoritative, request-bound evidence may return a
/// terminal result. An unchanged branch/open PR after a timeout is Unknown.
pub enum EffectObservation {
    Applied {
        head: String,
        commit: String,
        provenance: String,
    },
    NoEffect {
        provenance: String,
    },
    Unknown,
}

/// Trusted capability: implementations own forge reads and isolated write
/// credentials. Never construct an adapter from worker-supplied evidence.
pub trait Forge {
    fn observe(
        &mut self,
        repository: &str,
        pull: u64,
    ) -> std::result::Result<ForgeObservation, String>;
    fn merge_rebase(
        &mut self,
        repository: &str,
        request: &PublicationRequest,
    ) -> std::result::Result<(), String>;
    fn reconcile(
        &mut self,
        repository: &str,
        request: &PublicationRequest,
    ) -> std::result::Result<EffectObservation, String>;
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("publication storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("publication journal: {0}")]
    Json(#[from] serde_json::Error),
    #[error("publication contract: {0}")]
    Contract(#[from] PublicationError),
    #[error("publication denied: {0}")]
    Denied(String),
    #[error("publication service requires restart after uncertain persistence")]
    Poisoned,
}
pub type Result<T> = std::result::Result<T, GatewayError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayEntry {
    pub request: PublicationRequest,
    pub caller: String,
    pub intent: PublicationIntent,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    policy: GatewayPolicy,
    entries: Vec<GatewayEntry>,
}

pub struct PublicationGateway<F> {
    root: PathBuf,
    _lock: File,
    journal: Journal,
    forge: F,
    poisoned: bool,
}

fn denied(message: &str) -> GatewayError {
    GatewayError::Denied(message.into())
}

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(denied("private directory required"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(denied("directory must be owned by publisher and mode 0700"));
        }
    }
    Ok(())
}

fn pending(entry: &GatewayEntry) -> bool {
    !matches!(
        entry.intent.stage,
        IntentStage::ReconciledSuccess | IntentStage::ReconciledNoEffect
    )
}

impl<F: Forge> PublicationGateway<F> {
    /// The directory must be private, outside worker workspaces, and shared by
    /// every publisher for this repository/target. A second owner fails closed.
    pub fn open(root: &Path, policy: GatewayPolicy, forge: F) -> Result<Self> {
        private_directory(root)?;
        if !valid_repository(&policy.repository) || policy.authority_generation.parse().is_err() {
            return Err(denied("invalid repository or generation"));
        }
        let lock_path = root.join("publisher.lock");
        if fs::symlink_metadata(&lock_path)
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err(denied("invalid lock file"));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let lock = options.open(lock_path)?;
        lock.try_lock_exclusive()?;
        let path = root.join("journal.json");
        let journal = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || metadata.len() > JOURNAL_LIMIT
                {
                    return Err(denied("invalid or oversized journal"));
                }
                let mut bytes = Vec::new();
                File::open(path)?
                    .take(JOURNAL_LIMIT + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > JOURNAL_LIMIT {
                    return Err(denied("oversized journal"));
                }
                serde_json::from_slice::<Journal>(&bytes)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal {
                version: 1,
                policy: policy.clone(),
                entries: Vec::new(),
            },
            Err(e) => return Err(e.into()),
        };
        if journal.version != 1 || journal.policy != policy || journal.entries.len() > ENTRY_LIMIT {
            return Err(denied(
                "journal policy changed; retain target custody and use rotate_policy",
            ));
        }
        let mut keys = std::collections::BTreeSet::new();
        let mut occupied = false;
        for entry in &journal.entries {
            if !keys.insert(&entry.request.intent_key) || (pending(entry) && occupied) {
                return Err(denied("journal has duplicate keys or target slots"));
            }
            occupied |= pending(entry);
            if entry.intent.scope != entry.request.scope
                || entry.intent.intent_key != entry.request.intent_key
                || entry.intent.expected_head != entry.request.expected_head
                || entry.intent.expected_base != entry.request.expected_base
                || entry.intent.scope.project_id != policy.project_id
                || entry.intent.scope.target_ref != policy.target_ref
                || entry.request.pull_request == 0
                || !matches!(
                    entry.intent.stage,
                    IntentStage::Ambiguous
                        | IntentStage::ReconciledSuccess
                        | IntentStage::ReconciledNoEffect
                )
            {
                return Err(denied("inconsistent journal binding"));
            }
            reduce_publication_intent(
                Some(&entry.intent),
                &entry.intent,
                entry.intent.effect_receipt.as_ref(),
            )?;
        }
        let mut gateway = Self {
            root: root.to_path_buf(),
            _lock: lock,
            journal,
            forge,
            poisoned: false,
        };
        gateway.persist()?;
        Ok(gateway)
    }

    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            Err(GatewayError::Poisoned)
        } else {
            Ok(())
        }
    }

    fn persist(&mut self) -> Result<()> {
        let result = (|| {
            private_directory(&self.root)?;
            let bytes = serde_json::to_vec(&self.journal)?;
            if bytes.len() as u64 > JOURNAL_LIMIT {
                return Err(denied("journal capacity exhausted"));
            }
            let mut staged = tempfile::NamedTempFile::new_in(&self.root)?;
            staged.write_all(&bytes)?;
            staged.as_file().sync_all()?;
            staged
                .persist(self.root.join("journal.json"))
                .map_err(|e| e.error)?;
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Policy is supervisor-owned. Rotation preserves unresolved occupancy even
    /// when the old executor's generation is revoked. Repository/target custody
    /// cannot be changed by rotating a policy.
    pub fn rotate_policy(&mut self, policy: GatewayPolicy) -> Result<()> {
        self.healthy()?;
        let old = &self.journal.policy;
        if policy.repository != old.repository
            || policy.target_ref != old.target_ref
            || policy.project_id != old.project_id
            || policy.authority_generation.parse()? <= old.authority_generation.parse()?
        {
            return Err(denied(
                "policy rotation requires a newer generation on the same target",
            ));
        }
        self.journal.policy = policy;
        self.persist()
    }

    /// Assessment and execution use exactly the core admission predicate. The
    /// caller identity must come from the supervisor's authenticated session.
    pub fn assess(
        &mut self,
        request: &PublicationRequest,
        caller: &str,
    ) -> Result<PublicationProposal> {
        self.assess_inner(request, caller, false)
    }

    fn assess_inner(
        &mut self,
        request: &PublicationRequest,
        caller: &str,
        dispatch: bool,
    ) -> Result<PublicationProposal> {
        self.healthy()?;
        let policy = &self.journal.policy;
        if request.pull_request == 0
            || request.scope.project_id != policy.project_id
            || request.scope.target_ref != policy.target_ref
            || request.scope.authority_generation != policy.authority_generation
        {
            return Err(denied("request scope or authority is stale"));
        }
        let observation = self
            .forge
            .observe(&policy.repository, request.pull_request)
            .map_err(GatewayError::Denied)?;
        if observation.repository != policy.repository
            || observation.target_ref != policy.target_ref
            || observation.pull_request != request.pull_request
            || !observation.open
        {
            return Err(denied(
                "forge observation is for a different target or closed PR",
            ));
        }
        let snapshot = PublicationSnapshot {
            current_head: Some(observation.head),
            current_base: Some(observation.base),
            expected_head: Some(request.expected_head.clone()),
            expected_base: Some(request.expected_base.clone()),
            required_reviewers: policy.required_reviewers.clone(),
            approvals: observation
                .approvals
                .into_iter()
                .filter(|a| policy.required_reviewers.contains(&a.reviewer))
                .collect(),
            required_checks: policy.required_checks.clone(),
            checks: observation.checks,
            owner_only: policy.owner_only,
            caller_is_owner: policy.owners.iter().any(|o| o == caller),
            unresolved_intent: self
                .journal
                .entries
                .iter()
                .any(|e| pending(e) && (!dispatch || e.request != *request)),
        };
        let digest = publication_request_digest(
            &request.scope,
            &snapshot,
            &request.intent_key,
            &policy.authority_generation,
        )?;
        Ok(propose_publication(
            &request.scope,
            &snapshot,
            &request.intent_key,
            &digest,
            &policy.authority_generation,
        ))
    }

    pub fn entries(&self) -> &[GatewayEntry] {
        &self.journal.entries
    }

    /// A retry never invokes the forge twice, even after a process restart.
    /// Persist uncertainty BEFORE crossing the external action boundary.
    pub fn publish(&mut self, request: &PublicationRequest, caller: &str) -> Result<GatewayEntry> {
        self.healthy()?;
        if let Some(old) = self
            .journal
            .entries
            .iter()
            .find(|e| e.request.intent_key == request.intent_key)
        {
            return if old.request == *request && old.caller == caller {
                Ok(old.clone())
            } else {
                Err(PublicationError::KeyCollision.into())
            };
        }
        if self.journal.entries.len() >= ENTRY_LIMIT {
            return Err(denied("journal capacity exhausted"));
        }
        let proposed = self.assess(request, caller)?;
        let Some(intent) = proposed.intent else {
            return Err(denied(&format!("{:?}", proposed.reasons)));
        };
        // Read evidence again immediately before reserving the action slot.
        if self.assess(request, caller)?.intent.as_ref() != Some(&intent) {
            return Err(denied("admission evidence changed at action boundary"));
        }
        let proposed = reduce_publication_intent(None, &intent, None)?;
        let mut claimed = proposed.clone();
        claimed.stage = IntentStage::Claimed;
        let claimed = reduce_publication_intent(Some(&proposed), &claimed, None)?;
        let mut ambiguous = claimed.clone();
        ambiguous.stage = IntentStage::Ambiguous;
        let intent = reduce_publication_intent(Some(&claimed), &ambiguous, None)?;
        self.journal.entries.push(GatewayEntry {
            request: request.clone(),
            caller: caller.into(),
            intent,
        });
        self.persist()?;
        let current = self.assess_inner(request, caller, true)?;
        if current.intent.as_ref().map(|i| &i.admission_snapshot)
            != Some(
                &self
                    .journal
                    .entries
                    .last()
                    .ok_or_else(|| denied("missing intent"))?
                    .intent
                    .admission_snapshot,
            )
        {
            return Err(denied(
                "admission evidence changed after intent persistence",
            ));
        }
        // Success/failure of the RPC is not reconciliation evidence. A timeout,
        // rejected call or lost reply all retain their durable slot until read.
        let _reply = self
            .forge
            .merge_rebase(&self.journal.policy.repository, request);
        self.reconcile(&request.intent_key)
    }

    pub fn reconcile(&mut self, key: &str) -> Result<GatewayEntry> {
        self.healthy()?;
        let index = self
            .journal
            .entries
            .iter()
            .position(|e| e.request.intent_key == key)
            .ok_or_else(|| denied("unknown intent"))?;
        let entry = self.journal.entries[index].clone();
        if !pending(&entry) {
            return Ok(entry);
        }
        let observation = self
            .forge
            .reconcile(&self.journal.policy.repository, &entry.request);
        let (effect_occurred, observed_head, provenance_ref) = match observation {
            Ok(EffectObservation::Applied {
                head,
                commit,
                provenance,
            }) if head == entry.request.expected_head => (true, commit, provenance),
            Ok(EffectObservation::NoEffect { provenance }) => {
                (false, entry.request.expected_base.clone(), provenance)
            }
            _ => return Ok(entry),
        };
        let intent = &entry.intent;
        let receipt = ReconciliationReceipt {
            intent_key: intent.intent_key.clone(),
            request_digest: intent.request_digest.clone(),
            scope: intent.scope.clone(),
            expected_head: intent.expected_head.clone(),
            expected_base: intent.expected_base.clone(),
            authority_generation: intent.authority_generation.clone(),
            observed_head,
            effect_occurred,
            provenance_ref,
        };
        let reconciled = reduce_publication_intent(Some(intent), intent, Some(&receipt))?;
        self.journal.entries[index].intent = reconciled;
        self.persist()?;
        Ok(self.journal.entries[index].clone())
    }
}

pub(crate) fn valid_repository(repository: &str) -> bool {
    let parts: Vec<_> = repository.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 100
                && *p != "."
                && *p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
