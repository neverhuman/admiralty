//! Bounded in-memory topic subscriptions and authorized read proposals.
//!
//! The trusted service must authenticate the actor and inject an authoritative,
//! fail-closed policy evaluation for the complete topic and target subscriber.
//! Project/task labels and this directory never authenticate or grant authority.
//! No durable subscription, grant, recipient ACK/replay or runtime activation is
//! implemented here. Pages are lexical continuations, not snapshot/replay tokens.
use crate::{PrincipalId, ProjectId, TaskId};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
};

pub const MAX_CHANNEL_BYTES: usize = 256;
pub const MAX_TOPICS: usize = 256;
pub const MAX_TOTAL_SUBSCRIPTIONS: usize = 4096;
pub const MAX_SUBSCRIPTIONS_PER_TOPIC: usize = 256;
pub const MAX_TOPIC_PAGE: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScopedTopic {
    pub project_id: ProjectId,
    pub task_id: Option<TaskId>,
    pub channel: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopicError {
    EmptyChannel,
    ChannelTooLong,
    InvalidChannel,
    Unauthorized,
    InvalidLimits,
    InvalidPageLimit,
    TopicCapacity,
    TotalSubscriptionCapacity,
    TopicSubscriptionCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopicAction {
    Subscribe,
    Resolve,
}

/// A trusted-service dependency, not an authentication mechanism.
///
/// Implementations must evaluate authoritative actor/project/task/channel and
/// subscriber membership. Unknown/stale/unavailable authority must return false.
/// Subscribe always supplies Some(target); Resolve first supplies None for the
/// topic, then Some(target) for every returned/lookahead subscriber. A denied
/// target fails the entire page rather than silently scanning/filtering members.
pub trait TopicAuthorizer {
    fn authorize(
        &self,
        actor: &PrincipalId,
        action: TopicAction,
        topic: &ScopedTopic,
        target_subscriber: Option<&PrincipalId>,
    ) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopicLimits {
    pub max_topics: usize,
    pub max_total_subscriptions: usize,
    pub max_subscriptions_per_topic: usize,
}

impl Default for TopicLimits {
    fn default() -> Self {
        Self {
            max_topics: 64,
            max_total_subscriptions: 1024,
            max_subscriptions_per_topic: 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicPage {
    pub subscribers: Vec<PrincipalId>,
    /// Last returned subscriber when another authorized record exists.
    /// Subscription changes between calls do not confer a stable snapshot.
    pub next_cursor: Option<PrincipalId>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TopicDirectory {
    subscriptions: BTreeMap<ScopedTopic, BTreeSet<PrincipalId>>,
    total_subscriptions: usize,
    limits: TopicLimits,
}

fn validate_channel(channel: &str) -> Result<(), TopicError> {
    if channel.len() > MAX_CHANNEL_BYTES {
        return Err(TopicError::ChannelTooLong);
    }
    if channel.trim().is_empty() {
        return Err(TopicError::EmptyChannel);
    }
    if channel.trim() != channel || channel.chars().any(char::is_control) {
        return Err(TopicError::InvalidChannel);
    }
    Ok(())
}

impl TopicDirectory {
    pub fn with_limits(limits: TopicLimits) -> Result<Self, TopicError> {
        if !(1..=MAX_TOPICS).contains(&limits.max_topics)
            || !(1..=MAX_TOTAL_SUBSCRIPTIONS).contains(&limits.max_total_subscriptions)
            || !(1..=MAX_SUBSCRIPTIONS_PER_TOPIC).contains(&limits.max_subscriptions_per_topic)
            || limits.max_subscriptions_per_topic > limits.max_total_subscriptions
        {
            return Err(TopicError::InvalidLimits);
        }
        Ok(Self {
            subscriptions: BTreeMap::new(),
            total_subscriptions: 0,
            limits,
        })
    }

    pub fn subscribe<A: TopicAuthorizer + ?Sized>(
        &mut self,
        actor: &PrincipalId,
        authorizer: &A,
        topic: ScopedTopic,
        subscriber: PrincipalId,
    ) -> Result<bool, TopicError> {
        validate_channel(&topic.channel)?;
        if !authorizer.authorize(actor, TopicAction::Subscribe, &topic, Some(&subscriber)) {
            return Err(TopicError::Unauthorized);
        }
        let current = self.subscriptions.get(&topic);
        // Re-authorize even an exact retry. Capacity never breaks idempotency.
        if current.is_some_and(|set| set.contains(&subscriber)) {
            return Ok(false);
        }
        if current.is_none() && self.subscriptions.len() >= self.limits.max_topics {
            return Err(TopicError::TopicCapacity);
        }
        if current.is_some_and(|set| set.len() >= self.limits.max_subscriptions_per_topic) {
            return Err(TopicError::TopicSubscriptionCapacity);
        }
        if self.total_subscriptions >= self.limits.max_total_subscriptions {
            return Err(TopicError::TotalSubscriptionCapacity);
        }
        let next = self
            .total_subscriptions
            .checked_add(1)
            .ok_or(TopicError::TotalSubscriptionCapacity)?;
        // Every validation/policy/capacity check precedes container insertion.
        self.subscriptions
            .entry(topic)
            .or_default()
            .insert(subscriber);
        self.total_subscriptions = next;
        Ok(true)
    }

    pub fn resolve_page<A: TopicAuthorizer + ?Sized>(
        &self,
        actor: &PrincipalId,
        authorizer: &A,
        topic: &ScopedTopic,
        after: Option<&PrincipalId>,
        limit: usize,
    ) -> Result<TopicPage, TopicError> {
        validate_channel(&topic.channel)?;
        if !(1..=MAX_TOPIC_PAGE).contains(&limit) {
            return Err(TopicError::InvalidPageLimit);
        }
        if !authorizer.authorize(actor, TopicAction::Resolve, topic, None) {
            return Err(TopicError::Unauthorized);
        }
        let mut page = TopicPage {
            subscribers: Vec::new(),
            next_cursor: None,
        };
        let Some(subscribers) = self.subscriptions.get(topic) else {
            return Ok(page);
        };
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        // Seek directly by BTree cursor; inspect at most limit+1, clone at most
        // limit IDs plus one cursor. No full-directory clone or filtered scan.
        for subscriber in subscribers
            .range::<PrincipalId, _>((lower, Bound::Unbounded))
            .take(limit + 1)
        {
            if !authorizer.authorize(actor, TopicAction::Resolve, topic, Some(subscriber)) {
                return Err(TopicError::Unauthorized);
            }
            if page.subscribers.len() == limit {
                page.next_cursor = page.subscribers.last().cloned();
                break;
            }
            page.subscribers.push(subscriber.clone());
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn principal(value: &str) -> PrincipalId {
        PrincipalId::new(value).expect("bounded test principal")
    }
    fn topic(channel: &str) -> ScopedTopic {
        ScopedTopic {
            project_id: ProjectId::new("project").expect("test project"),
            task_id: Some(TaskId::new("task").expect("test task")),
            channel: channel.into(),
        }
    }
    struct Allow;
    impl TopicAuthorizer for Allow {
        fn authorize(
            &self,
            _: &PrincipalId,
            _: TopicAction,
            _: &ScopedTopic,
            _: Option<&PrincipalId>,
        ) -> bool {
            true
        }
    }
    type Query = (PrincipalId, TopicAction, ScopedTopic, Option<PrincipalId>);
    struct Policy {
        calls: RefCell<Vec<Query>>,
        reads: Cell<bool>,
        targets: Cell<bool>,
    }
    impl Default for Policy {
        fn default() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                reads: Cell::new(true),
                targets: Cell::new(true),
            }
        }
    }
    impl TopicAuthorizer for Policy {
        fn authorize(
            &self,
            actor: &PrincipalId,
            action: TopicAction,
            candidate: &ScopedTopic,
            target: Option<&PrincipalId>,
        ) -> bool {
            self.calls.borrow_mut().push((
                actor.clone(),
                action,
                candidate.clone(),
                target.cloned(),
            ));
            actor == &principal("captain")
                && candidate == &topic("progress")
                && (action != TopicAction::Resolve || self.reads.get())
                && target.is_none_or(|subscriber| {
                    self.targets.get() && subscriber == &principal("worker")
                })
                && (action != TopicAction::Subscribe || target.is_some())
        }
    }
    fn snapshot(
        directory: &TopicDirectory,
    ) -> (BTreeMap<ScopedTopic, BTreeSet<PrincipalId>>, usize) {
        (
            directory.subscriptions.clone(),
            directory.total_subscriptions,
        )
    }
    fn small(topics: usize, total: usize, per_topic: usize) -> TopicDirectory {
        TopicDirectory::with_limits(TopicLimits {
            max_topics: topics,
            max_total_subscriptions: total,
            max_subscriptions_per_topic: per_topic,
        })
        .expect("valid smaller limits")
    }

    #[test]
    fn policy_receives_actor_complete_topic_action_and_target() {
        let mut directory = TopicDirectory::default();
        let policy = Policy::default();
        let actor = principal("captain");
        let target = principal("worker");
        assert_eq!(
            directory.subscribe(&actor, &policy, topic("progress"), target.clone()),
            Ok(true)
        );
        let page = directory
            .resolve_page(&actor, &policy, &topic("progress"), None, 1)
            .expect("authorized page");
        assert_eq!(page.subscribers, vec![target.clone()]);
        assert_eq!(page.next_cursor, None);
        assert_eq!(
            *policy.calls.borrow(),
            vec![
                (
                    actor.clone(),
                    TopicAction::Subscribe,
                    topic("progress"),
                    Some(target.clone())
                ),
                (actor.clone(), TopicAction::Resolve, topic("progress"), None),
                (actor, TopicAction::Resolve, topic("progress"), Some(target))
            ]
        );
    }

    #[test]
    fn forged_actor_project_task_channel_or_subscriber_is_denied_without_empty_containers() {
        let mut directory = TopicDirectory::default();
        let policy = Policy::default();
        let mut foreign_project = topic("progress");
        foreign_project.project_id = ProjectId::new("forged").expect("test ID");
        let mut foreign_task = topic("progress");
        foreign_task.task_id = Some(TaskId::new("forged").expect("test ID"));
        let mut missing_task = topic("progress");
        missing_task.task_id = None;
        for (actor, candidate, target) in [
            (principal("forged"), topic("progress"), principal("worker")),
            (principal("captain"), foreign_project, principal("worker")),
            (principal("captain"), foreign_task, principal("worker")),
            (principal("captain"), missing_task, principal("worker")),
            (
                principal("captain"),
                topic("forged-channel"),
                principal("worker"),
            ),
        ] {
            let before = snapshot(&directory);
            assert_eq!(
                directory.subscribe(&actor, &policy, candidate.clone(), target),
                Err(TopicError::Unauthorized)
            );
            assert_eq!(
                directory.resolve_page(&actor, &policy, &candidate, None, 1),
                Err(TopicError::Unauthorized)
            );
            assert_eq!(snapshot(&directory), before);
            assert!(directory.subscriptions.is_empty());
        }
        assert_eq!(
            directory.subscribe(
                &principal("captain"),
                &policy,
                topic("progress"),
                principal("forged")
            ),
            Err(TopicError::Unauthorized)
        );
        assert!(directory.subscriptions.is_empty());
        assert_eq!(directory.total_subscriptions, 0);
    }

    #[test]
    fn reads_and_exact_duplicate_retries_are_reauthorized_after_policy_changes() {
        let mut directory = small(1, 1, 1);
        let policy = Policy::default();
        let actor = principal("captain");
        directory
            .subscribe(&actor, &policy, topic("progress"), principal("worker"))
            .expect("insert");
        assert_eq!(
            directory.subscribe(&actor, &policy, topic("progress"), principal("worker")),
            Ok(false)
        );
        let before = snapshot(&directory);
        policy.reads.set(false);
        assert_eq!(
            directory.resolve_page(&actor, &policy, &topic("progress"), None, 1),
            Err(TopicError::Unauthorized)
        );
        policy.reads.set(true);
        policy.targets.set(false);
        assert_eq!(
            directory.resolve_page(&actor, &policy, &topic("progress"), None, 1),
            Err(TopicError::Unauthorized)
        );
        assert_eq!(
            directory.subscribe(&actor, &policy, topic("progress"), principal("worker")),
            Err(TopicError::Unauthorized)
        );
        assert_eq!(snapshot(&directory), before);
    }

    #[test]
    fn all_capacity_checks_are_atomic_and_duplicates_remain_idempotent_at_capacity() {
        let actor = principal("captain");
        for (mut directory, first, second, error) in [
            (
                small(1, 2, 2),
                topic("first"),
                topic("second"),
                TopicError::TopicCapacity,
            ),
            (
                small(2, 1, 1),
                topic("first"),
                topic("second"),
                TopicError::TotalSubscriptionCapacity,
            ),
            (
                small(2, 2, 1),
                topic("first"),
                topic("first"),
                TopicError::TopicSubscriptionCapacity,
            ),
        ] {
            directory
                .subscribe(&actor, &Allow, first.clone(), principal("one"))
                .expect("first insertion");
            let before = snapshot(&directory);
            assert_eq!(
                directory.subscribe(&actor, &Allow, second, principal("two")),
                Err(error)
            );
            assert_eq!(snapshot(&directory), before);
            assert_eq!(
                directory.subscribe(&actor, &Allow, first, principal("one")),
                Ok(false)
            );
            assert_eq!(snapshot(&directory), before);
            assert!(
                directory
                    .subscriptions
                    .values()
                    .all(|subscribers| !subscribers.is_empty())
            );
        }
    }

    #[test]
    fn channel_limits_count_utf8_bytes_on_both_reads_and_writes() {
        let actor = principal("captain");
        let mut directory = TopicDirectory::default();
        let exact = "π".repeat(MAX_CHANNEL_BYTES / 2);
        assert_eq!(
            directory.subscribe(&actor, &Allow, topic(&exact), principal("one")),
            Ok(true)
        );
        assert_eq!(
            directory
                .resolve_page(&actor, &Allow, &topic(&exact), None, 1)
                .expect("exact-byte channel")
                .subscribers,
            vec![principal("one")]
        );
        for channel in [
            "".into(),
            " ".into(),
            " padded".into(),
            "trailing ".into(),
            "bad\nchannel".into(),
            "bad\u{7f}".into(),
            "π".repeat(MAX_CHANNEL_BYTES / 2 + 1),
            "x".repeat(MAX_CHANNEL_BYTES + 1),
        ] {
            let before = snapshot(&directory);
            assert!(
                directory
                    .subscribe(&actor, &Allow, topic(&channel), principal("two"))
                    .is_err()
            );
            assert!(
                directory
                    .resolve_page(&actor, &Allow, &topic(&channel), None, 1)
                    .is_err()
            );
            assert_eq!(snapshot(&directory), before);
        }
    }

    #[test]
    fn zero_and_oversized_page_limits_fail_without_state_or_policy_work() {
        let directory = TopicDirectory::default();
        let policy = Policy::default();
        for limit in [0, MAX_TOPIC_PAGE + 1, usize::MAX] {
            assert_eq!(
                directory.resolve_page(
                    &principal("captain"),
                    &policy,
                    &topic("progress"),
                    None,
                    limit
                ),
                Err(TopicError::InvalidPageLimit)
            );
        }
        assert!(policy.calls.borrow().is_empty());
        assert!(directory.subscriptions.is_empty());
    }

    #[test]
    fn pages_seek_exclusive_sorted_cursors_without_duplicates_or_full_set_clones() {
        let actor = principal("captain");
        let mut directory = TopicDirectory::default();
        for name in [
            "worker-05",
            "worker-01",
            "worker-03",
            "worker-02",
            "worker-04",
        ] {
            directory
                .subscribe(&actor, &Allow, topic("progress"), principal(name))
                .expect("subscribe");
        }
        let first = directory
            .resolve_page(&actor, &Allow, &topic("progress"), None, 2)
            .expect("first");
        assert_eq!(
            first.subscribers,
            vec![principal("worker-01"), principal("worker-02")]
        );
        assert_eq!(first.next_cursor, Some(principal("worker-02")));
        let second = directory
            .resolve_page(
                &actor,
                &Allow,
                &topic("progress"),
                first.next_cursor.as_ref(),
                2,
            )
            .expect("second");
        assert_eq!(
            second.subscribers,
            vec![principal("worker-03"), principal("worker-04")]
        );
        let last = directory
            .resolve_page(
                &actor,
                &Allow,
                &topic("progress"),
                second.next_cursor.as_ref(),
                2,
            )
            .expect("last");
        assert_eq!(last.subscribers, vec![principal("worker-05")]);
        assert_eq!(last.next_cursor, None);
        let empty = directory
            .resolve_page(
                &actor,
                &Allow,
                &topic("progress"),
                Some(&principal("worker-99")),
                2,
            )
            .expect("past end");
        assert!(empty.subscribers.is_empty());
        assert_eq!(empty.next_cursor, None);
        assert_eq!(directory.total_subscriptions, 5);
    }

    #[test]
    fn lookahead_is_bounded_and_authorized_before_exposing_a_cursor() {
        struct Recording(RefCell<Vec<Option<PrincipalId>>>);
        impl TopicAuthorizer for Recording {
            fn authorize(
                &self,
                _: &PrincipalId,
                _: TopicAction,
                _: &ScopedTopic,
                target: Option<&PrincipalId>,
            ) -> bool {
                self.0.borrow_mut().push(target.cloned());
                target != Some(&principal("worker-03"))
            }
        }
        let actor = principal("captain");
        let mut directory = TopicDirectory::default();
        for name in ["worker-01", "worker-02", "worker-03", "worker-04"] {
            directory
                .subscribe(&actor, &Allow, topic("progress"), principal(name))
                .expect("subscribe");
        }
        let policy = Recording(RefCell::new(Vec::new()));
        let before = snapshot(&directory);
        assert_eq!(
            directory.resolve_page(&actor, &policy, &topic("progress"), None, 2),
            Err(TopicError::Unauthorized)
        );
        assert_eq!(
            *policy.0.borrow(),
            vec![
                None,
                Some(principal("worker-01")),
                Some(principal("worker-02")),
                Some(principal("worker-03"))
            ]
        );
        assert_eq!(snapshot(&directory), before);
    }

    #[test]
    fn defaults_and_custom_limits_cannot_exceed_hard_bounds() {
        let defaults = TopicDirectory::default();
        assert_eq!(
            defaults.limits,
            TopicLimits {
                max_topics: 64,
                max_total_subscriptions: 1024,
                max_subscriptions_per_topic: 64
            }
        );
        for limits in [
            TopicLimits {
                max_topics: 0,
                ..TopicLimits::default()
            },
            TopicLimits {
                max_topics: MAX_TOPICS + 1,
                ..TopicLimits::default()
            },
            TopicLimits {
                max_total_subscriptions: 0,
                ..TopicLimits::default()
            },
            TopicLimits {
                max_total_subscriptions: MAX_TOTAL_SUBSCRIPTIONS + 1,
                ..TopicLimits::default()
            },
            TopicLimits {
                max_subscriptions_per_topic: 0,
                ..TopicLimits::default()
            },
            TopicLimits {
                max_subscriptions_per_topic: MAX_SUBSCRIPTIONS_PER_TOPIC + 1,
                ..TopicLimits::default()
            },
            TopicLimits {
                max_total_subscriptions: 1,
                max_subscriptions_per_topic: 2,
                ..TopicLimits::default()
            },
        ] {
            assert_eq!(
                TopicDirectory::with_limits(limits),
                Err(TopicError::InvalidLimits)
            );
        }
        assert!(
            TopicDirectory::with_limits(TopicLimits {
                max_topics: MAX_TOPICS,
                max_total_subscriptions: MAX_TOTAL_SUBSCRIPTIONS,
                max_subscriptions_per_topic: MAX_SUBSCRIPTIONS_PER_TOPIC
            })
            .is_ok()
        );
    }
}
