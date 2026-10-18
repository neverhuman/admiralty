//! Deterministic, bounded, in-memory forge fixtures. Nothing contacts a forge.
//!
//! Server facts and replies are simulated observations, never approval,
//! publication authority, task acceptance, or proof of real transport behavior.
//! A before-mutation fault records a simulated server intent and `NotApplied`;
//! it deliberately does not model loss before server admission.

use std::{collections::BTreeMap, fmt};

pub const MAX_IDENTIFIER_BYTES: usize = 128;
pub const MAX_REFS: usize = 256;
pub const MAX_REQUESTS: usize = 1024;
pub const DEFAULT_REFS: usize = 32;
pub const DEFAULT_REQUESTS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForgeKey {
    pub repository: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeRequest {
    pub id: String,
    pub key: ForgeKey,
    pub expected_head: String,
    pub new_head: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    /// Records an admitted simulated intent with no mutation, then loses its reply.
    LoseReplyBeforeMutation,
    /// Records the normal CAS outcome, then loses its reply. Rejection is possible.
    LoseReplyAfterMutation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForgeRejection {
    MissingReference,
    HeadMismatch { actual_head: String },
}

/// An immutable simulated server fact. `Applied` includes successful no-op CAS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForgeOutcome {
    Applied {
        previous_head: String,
        new_head: String,
    },
    Rejected(ForgeRejection),
    NotApplied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForgeReply {
    Observed(ForgeOutcome),
    OutcomeUnknown,
}

impl ForgeOutcome {
    pub const fn simulated(&self) -> bool {
        true
    }

    pub const fn acceptance_granted(&self) -> bool {
        false
    }

    pub const fn execution_granted(&self) -> bool {
        false
    }
}

impl ForgeReply {
    pub const fn simulated(&self) -> bool {
        true
    }

    pub const fn acceptance_granted(&self) -> bool {
        false
    }

    pub const fn execution_granted(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgeError {
    InvalidCapacity,
    InvalidIdentifier,
    InvalidHead,
    HeadLengthMismatch,
    ReferenceCapacity,
    RequestCapacity,
    RequestCollision,
    SeedCollision,
    MutationCounterOverflow,
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCapacity => "simulated forge capacity is outside hard bounds",
            Self::InvalidIdentifier => "simulated forge identifier is malformed or oversized",
            Self::InvalidHead => "simulated forge requires a full lowercase hexadecimal head",
            Self::HeadLengthMismatch => "simulated forge request head lengths differ",
            Self::ReferenceCapacity => "simulated forge reference capacity exhausted",
            Self::RequestCapacity => "simulated forge immutable request history exhausted",
            Self::RequestCollision => "simulated forge request ID has different immutable content",
            Self::SeedCollision => "simulated forge seed reference already has another head",
            Self::MutationCounterOverflow => "simulated forge mutation counter overflow",
        })
    }
}

impl std::error::Error for ForgeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedRequest {
    request: ForgeRequest,
    outcome: ForgeOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeSimulator {
    refs: BTreeMap<ForgeKey, String>,
    requests: BTreeMap<String, RecordedRequest>,
    ref_capacity: usize,
    request_capacity: usize,
    applied_mutations: u64,
}

impl Default for ForgeSimulator {
    fn default() -> Self {
        Self {
            refs: BTreeMap::new(),
            requests: BTreeMap::new(),
            ref_capacity: DEFAULT_REFS,
            request_capacity: DEFAULT_REQUESTS,
            applied_mutations: 0,
        }
    }
}

impl ForgeSimulator {
    pub fn new(ref_capacity: usize, request_capacity: usize) -> Result<Self, ForgeError> {
        if !(1..=MAX_REFS).contains(&ref_capacity)
            || !(1..=MAX_REQUESTS).contains(&request_capacity)
        {
            return Err(ForgeError::InvalidCapacity);
        }
        Ok(Self {
            ref_capacity,
            request_capacity,
            ..Self::default()
        })
    }

    /// Bootstrap an immutable reference. Moving a seeded reference requires CAS.
    pub fn seed_ref(&mut self, key: ForgeKey, head: String) -> Result<(), ForgeError> {
        validate_key(&key)?;
        validate_head(&head)?;
        if let Some(existing) = self.refs.get(&key) {
            return if existing == &head {
                Ok(())
            } else {
                Err(ForgeError::SeedCollision)
            };
        }
        if self.refs.len() >= self.ref_capacity {
            return Err(ForgeError::ReferenceCapacity);
        }
        self.refs.insert(key, head);
        Ok(())
    }

    /// Exact retries return the original server fact without reapplying CAS.
    /// Fault selection describes this attempted exchange, not immutable content.
    /// Capacity and validation errors do not admit an intent or mutate any state.
    pub fn submit(
        &mut self,
        request: ForgeRequest,
        fault: Fault,
    ) -> Result<ForgeReply, ForgeError> {
        validate_identifier(&request.id)?;
        // Bound all comparisons before consulting an existing immutable request.
        if request.key.repository.len() > MAX_IDENTIFIER_BYTES
            || request.key.target.len() > MAX_IDENTIFIER_BYTES
        {
            return Err(ForgeError::InvalidIdentifier);
        }
        if request.expected_head.len() > 64 || request.new_head.len() > 64 {
            return Err(ForgeError::InvalidHead);
        }
        if let Some(recorded) = self.requests.get(&request.id) {
            return if recorded.request == request {
                Ok(ForgeReply::Observed(recorded.outcome.clone()))
            } else {
                Err(ForgeError::RequestCollision)
            };
        }
        validate_key(&request.key)?;
        validate_head(&request.expected_head)?;
        validate_head(&request.new_head)?;
        if request.expected_head.len() != request.new_head.len() {
            return Err(ForgeError::HeadLengthMismatch);
        }
        if self.requests.len() >= self.request_capacity {
            return Err(ForgeError::RequestCapacity);
        }

        let outcome = if fault == Fault::LoseReplyBeforeMutation {
            ForgeOutcome::NotApplied
        } else {
            match self.refs.get(&request.key) {
                None => ForgeOutcome::Rejected(ForgeRejection::MissingReference),
                Some(actual) if actual != &request.expected_head => {
                    ForgeOutcome::Rejected(ForgeRejection::HeadMismatch {
                        actual_head: actual.clone(),
                    })
                }
                Some(actual) => ForgeOutcome::Applied {
                    previous_head: actual.clone(),
                    new_head: request.new_head.clone(),
                },
            }
        };
        let changes_ref = matches!(
            &outcome,
            ForgeOutcome::Applied { previous_head, new_head } if previous_head != new_head
        );
        let mutations = self
            .applied_mutations
            .checked_add(u64::from(changes_ref))
            .ok_or(ForgeError::MutationCounterOverflow)?;

        // Every fallible validation/capacity/counter check precedes mutation.
        if changes_ref {
            self.refs
                .insert(request.key.clone(), request.new_head.clone());
        }
        let reply = if fault == Fault::None {
            ForgeReply::Observed(outcome.clone())
        } else {
            ForgeReply::OutcomeUnknown
        };
        self.requests
            .insert(request.id.clone(), RecordedRequest { request, outcome });
        self.applied_mutations = mutations;
        Ok(reply)
    }

    /// An explicit independent simulated probe; reading it never retries a write.
    pub fn reconcile(&self, request_id: &str) -> Option<&ForgeOutcome> {
        validate_identifier(request_id).ok()?;
        self.requests.get(request_id).map(|record| &record.outcome)
    }

    pub fn head(&self, key: &ForgeKey) -> Option<&str> {
        validate_key(key).ok()?;
        self.refs.get(key).map(String::as_str)
    }

    pub fn ref_count(&self) -> usize {
        self.refs.len()
    }

    pub fn request_count(&self) -> usize {
        self.requests.len()
    }

    /// Number of actual simulated head changes; retries and no-op CAS add zero.
    pub fn applied_mutations(&self) -> u64 {
        self.applied_mutations
    }
}

fn validate_identifier(value: &str) -> Result<(), ForgeError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(ForgeError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_key(key: &ForgeKey) -> Result<(), ForgeError> {
    validate_identifier(&key.repository)?;
    validate_identifier(&key.target)
}

fn validate_head(head: &str) -> Result<(), ForgeError> {
    if !matches!(head.len(), 40 | 64)
        || !head
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ForgeError::InvalidHead);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ForgeKey {
        ForgeKey {
            repository: "fixture/repository".into(),
            target: "refs/heads/main".into(),
        }
    }

    fn head(ch: char) -> String {
        ch.to_string().repeat(40)
    }

    fn request(id: &str, expected: char, new: char) -> ForgeRequest {
        ForgeRequest {
            id: id.into(),
            key: key(),
            expected_head: head(expected),
            new_head: head(new),
        }
    }

    fn seeded() -> ForgeSimulator {
        let mut forge = ForgeSimulator::default();
        forge.seed_ref(key(), head('a')).expect("valid seed");
        forge
    }

    #[test]
    fn exact_head_cas_applies_once_and_wrong_or_missing_refs_are_rejected() {
        let mut forge = seeded();
        assert_eq!(
            forge.submit(request("apply", 'a', 'b'), Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::Applied {
                previous_head: head('a'),
                new_head: head('b'),
            }))
        );
        assert_eq!(
            forge.submit(request("stale", 'a', 'c'), Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::Rejected(
                ForgeRejection::HeadMismatch {
                    actual_head: head('b')
                }
            )))
        );
        let mut missing = request("missing", 'a', 'b');
        missing.key.repository = "other/repository".into();
        assert_eq!(
            forge.submit(missing, Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::Rejected(
                ForgeRejection::MissingReference
            )))
        );
        assert_eq!(forge.head(&key()), Some(head('b').as_str()));
        assert_eq!(forge.applied_mutations(), 1);
        assert_eq!(forge.request_count(), 3);
    }

    #[test]
    fn replay_returns_original_success_after_another_request_moves_the_ref() {
        let mut forge = seeded();
        let original = request("first", 'a', 'b');
        let reply = forge.submit(original.clone(), Fault::None).expect("CAS");
        forge
            .submit(request("second", 'b', 'c'), Fault::None)
            .expect("second CAS");
        let before = forge.clone();
        assert_eq!(
            forge.submit(original, Fault::LoseReplyAfterMutation),
            Ok(reply)
        );
        assert_eq!(forge, before);
        assert_eq!(forge.head(&key()), Some(head('c').as_str()));
        assert_eq!(forge.applied_mutations(), 2);
    }

    #[test]
    fn rejection_is_cached_even_when_current_head_later_changes() {
        let mut forge = seeded();
        let rejected = request("stale", 'c', 'd');
        let old_reply = forge.submit(rejected.clone(), Fault::None).expect("reject");
        forge
            .submit(request("move", 'a', 'b'), Fault::None)
            .expect("move ref");
        let before = forge.clone();
        assert_eq!(forge.submit(rejected, Fault::None), Ok(old_reply));
        assert_eq!(forge, before);
        assert_eq!(forge.request_count(), 2);
    }

    #[test]
    fn immutable_id_rejects_changed_scope_heads_and_bounded_malformed_content() {
        let mut forge = seeded();
        let original = request("immutable", 'a', 'b');
        forge.submit(original.clone(), Fault::None).expect("CAS");
        let before = forge.clone();
        for field in 0..5 {
            let mut changed = original.clone();
            match field {
                0 => changed.key.repository = "other/repository".into(),
                1 => changed.key.target = "refs/heads/other".into(),
                2 => changed.expected_head = head('c'),
                3 => changed.new_head = head('d'),
                _ => changed.new_head = "partial".into(),
            }
            assert_eq!(
                forge.submit(changed, Fault::None),
                Err(ForgeError::RequestCollision)
            );
            assert_eq!(forge, before);
        }
    }

    #[test]
    fn before_mutation_lost_reply_records_not_applied_and_never_reapplies() {
        let mut forge = seeded();
        let req = request("lost-before", 'a', 'b');
        assert_eq!(
            forge.submit(req.clone(), Fault::LoseReplyBeforeMutation),
            Ok(ForgeReply::OutcomeUnknown)
        );
        assert_eq!(forge.reconcile(&req.id), Some(&ForgeOutcome::NotApplied));
        assert_eq!(forge.head(&key()), Some(head('a').as_str()));
        assert_eq!(forge.applied_mutations(), 0);
        let before = forge.clone();
        assert_eq!(
            forge.submit(req, Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::NotApplied))
        );
        assert_eq!(forge, before);
        assert_eq!(forge.request_count(), 1);
    }

    #[test]
    fn after_mutation_lost_reply_requires_probe_and_exact_retry_cannot_reapply() {
        let mut forge = seeded();
        let req = request("lost-after", 'a', 'b');
        assert_eq!(
            forge.submit(req.clone(), Fault::LoseReplyAfterMutation),
            Ok(ForgeReply::OutcomeUnknown)
        );
        let applied = ForgeOutcome::Applied {
            previous_head: head('a'),
            new_head: head('b'),
        };
        assert_eq!(forge.reconcile(&req.id), Some(&applied));
        assert_eq!(forge.applied_mutations(), 1);
        let before = forge.clone();
        assert_eq!(
            forge.submit(req, Fault::None),
            Ok(ForgeReply::Observed(applied))
        );
        assert_eq!(forge, before);
        assert_eq!(forge.reconcile("never-admitted"), None);
    }

    #[test]
    fn reply_loss_after_a_rejected_cas_does_not_invent_a_mutation() {
        let mut forge = seeded();
        assert_eq!(
            forge.submit(
                request("lost-rejection", 'b', 'c'),
                Fault::LoseReplyAfterMutation
            ),
            Ok(ForgeReply::OutcomeUnknown)
        );
        assert_eq!(
            forge.reconcile("lost-rejection"),
            Some(&ForgeOutcome::Rejected(ForgeRejection::HeadMismatch {
                actual_head: head('a')
            }))
        );
        assert_eq!(forge.applied_mutations(), 0);
    }

    #[test]
    fn capacity_rejections_preserve_refs_intents_and_collision_history() {
        let mut forge = ForgeSimulator::new(1, 1).expect("bounded capacities");
        forge.seed_ref(key(), head('a')).expect("seed");
        let req = request("only-request", 'a', 'b');
        let cached = forge.submit(req.clone(), Fault::None).expect("CAS");
        let before = forge.clone();
        let other = ForgeKey {
            repository: "other".into(),
            ..key()
        };
        assert_eq!(
            forge.seed_ref(other, head('a')),
            Err(ForgeError::ReferenceCapacity)
        );
        assert_eq!(forge, before);
        assert_eq!(
            forge.submit(request("full", 'b', 'c'), Fault::LoseReplyAfterMutation),
            Err(ForgeError::RequestCapacity)
        );
        assert_eq!(forge.reconcile("full"), None);
        assert_eq!(forge, before);
        assert_eq!(forge.submit(req.clone(), Fault::None), Ok(cached));
        assert_eq!(forge, before);
        let mut changed = req;
        changed.new_head = head('c');
        assert_eq!(
            forge.submit(changed, Fault::None),
            Err(ForgeError::RequestCollision)
        );
        assert_eq!(forge, before);
    }

    #[test]
    fn seed_is_immutable_and_noop_cas_does_not_increment_head_changes() {
        let mut forge = seeded();
        let before = forge.clone();
        assert_eq!(forge.seed_ref(key(), head('a')), Ok(()));
        assert_eq!(
            forge.seed_ref(key(), head('b')),
            Err(ForgeError::SeedCollision)
        );
        assert_eq!(forge, before);
        assert!(matches!(
            forge.submit(request("noop", 'a', 'a'), Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::Applied { .. }))
        ));
        assert_eq!(forge.applied_mutations(), 0);
    }

    #[test]
    fn malformed_identifiers_and_heads_never_admit_or_mutate() {
        let mut forge = seeded();
        let before = forge.clone();
        for invalid in [
            "".into(),
            " ".into(),
            " padded".into(),
            "padded ".into(),
            "line\nbreak".into(),
            "null\0byte".into(),
            "x".repeat(129),
        ] {
            let mut req = request("valid", 'a', 'b');
            req.id = invalid.clone();
            assert_eq!(
                forge.submit(req, Fault::None),
                Err(ForgeError::InvalidIdentifier)
            );
            assert_eq!(forge.reconcile(&invalid), None);
            let mut bad_key = key();
            bad_key.repository = invalid.clone();
            assert_eq!(
                forge.seed_ref(bad_key.clone(), head('a')),
                Err(ForgeError::InvalidIdentifier)
            );
            assert_eq!(forge.head(&bad_key), None);
            let mut bad_target = key();
            bad_target.target = invalid;
            assert_eq!(
                forge.seed_ref(bad_target, head('a')),
                Err(ForgeError::InvalidIdentifier)
            );
            assert_eq!(forge, before);
        }
        for invalid in [
            "a".repeat(39),
            "a".repeat(41),
            "A".repeat(40),
            "g".repeat(64),
            "0".repeat(65),
            format!("sha256:{}", "a".repeat(64)),
        ] {
            let mut req = request("valid", 'a', 'b');
            req.new_head = invalid.clone();
            assert_eq!(forge.submit(req, Fault::None), Err(ForgeError::InvalidHead));
            assert_eq!(forge.seed_ref(key(), invalid), Err(ForgeError::InvalidHead));
            assert_eq!(forge, before);
        }
        let mut mismatch = request("length-mismatch", 'a', 'b');
        mismatch.new_head = "b".repeat(64);
        assert_eq!(
            forge.submit(mismatch, Fault::None),
            Err(ForgeError::HeadLengthMismatch)
        );
        assert_eq!(forge, before);
    }

    #[test]
    fn exact_byte_and_hard_capacity_bounds_are_supported() {
        for (refs, requests) in [
            (0, 1),
            (1, 0),
            (MAX_REFS + 1, 1),
            (1, MAX_REQUESTS + 1),
            (usize::MAX, usize::MAX),
        ] {
            assert_eq!(
                ForgeSimulator::new(refs, requests),
                Err(ForgeError::InvalidCapacity)
            );
        }
        let exact_key = ForgeKey {
            repository: "r".repeat(128),
            target: "é".repeat(64),
        };
        let mut forge = ForgeSimulator::new(MAX_REFS, MAX_REQUESTS).expect("hard caps");
        forge
            .seed_ref(exact_key.clone(), "a".repeat(64))
            .expect("128-byte identifiers and 64-byte head");
        let req = ForgeRequest {
            id: "i".repeat(128),
            key: exact_key,
            expected_head: "a".repeat(64),
            new_head: "b".repeat(64),
        };
        assert!(matches!(
            forge.submit(req, Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::Applied { .. }))
        ));
        for n in 1..MAX_REFS {
            forge
                .seed_ref(
                    ForgeKey {
                        repository: format!("repo-{n}"),
                        ..key()
                    },
                    head('a'),
                )
                .expect("within reference cap");
        }
        assert_eq!(forge.ref_count(), MAX_REFS);
        for n in 1..MAX_REQUESTS {
            forge
                .submit(request(&format!("missing-{n}"), 'a', 'b'), Fault::None)
                .expect("bounded cached rejection");
        }
        assert_eq!(forge.request_count(), MAX_REQUESTS);
        let before = forge.clone();
        assert_eq!(
            forge.submit(request("overflow", 'a', 'b'), Fault::None),
            Err(ForgeError::RequestCapacity)
        );
        assert_eq!(forge, before);
    }

    #[test]
    fn checked_counter_failure_is_atomic_and_fixed_sequences_replay_deterministically() {
        let mut forge = seeded();
        forge.applied_mutations = u64::MAX;
        let before = forge.clone();
        assert_eq!(
            forge.submit(request("overflow", 'a', 'b'), Fault::None),
            Err(ForgeError::MutationCounterOverflow)
        );
        assert_eq!(forge, before);

        let mut first = seeded();
        let mut second = seeded();
        for (id, expected, new, fault) in [
            ("one", 'a', 'b', Fault::LoseReplyAfterMutation),
            ("two", 'b', 'c', Fault::LoseReplyBeforeMutation),
            ("three", 'a', 'd', Fault::None),
        ] {
            assert_eq!(
                first.submit(request(id, expected, new), fault),
                second.submit(request(id, expected, new), fault)
            );
        }
        assert_eq!(first, second);
    }

    #[test]
    fn simulated_results_grant_no_real_execution_or_acceptance() {
        let mut forge = seeded();
        let reply = forge
            .submit(request("symbolic", 'a', 'b'), Fault::LoseReplyAfterMutation)
            .expect("pure simulated request");
        assert!(reply.simulated());
        assert!(!reply.acceptance_granted());
        assert!(!reply.execution_granted());
        let outcome = forge.reconcile("symbolic").expect("simulated server fact");
        assert!(outcome.simulated());
        assert!(!outcome.acceptance_granted());
        assert!(!outcome.execution_granted());
    }
}
