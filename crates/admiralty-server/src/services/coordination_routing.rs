//! Opt-in native agent communication. Trusted startup installs exact routes;
//! authenticated requests cannot choose sender, project, task revision or peers.
//! Messages and application receipts confer no execution or acceptance authority.
use super::coordination::{AuthenticatedCoordinationActor, CoordinationCredentialAuthority};
use admiralty_core::{CoordinationMessage, MessageId, MessagePayload, SourceIdentity, TaskView};
use admiralty_store::{
    AppendReceipt, Store, StoreError,
    inbox::InboxScope,
    subscriptions::{Subscription, SubscriptionAction, SubscriptionAuthority},
};
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    collections::BTreeSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use uuid::Uuid;

pub const MAX_NATIVE_ROUTES: usize = 1024;

/// A finite route, loaded only from protected startup configuration. Membership
/// remains bound to the current Store contract on every send and retry.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageRoute {
    pub subscription: Subscription,
    pub authority_ref: String,
}

pub struct InstalledRoute {
    pub credential_id: String,
    pub route: AgentMessageRoute,
}

pub struct NativeRoutingAuthority {
    credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
    routes: Vec<InstalledRoute>,
}

struct RoutingProof {
    actor: AuthenticatedCoordinationActor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    pub request_id: String,
    pub route_id: MessageId,
    pub causation_id: Option<MessageId>,
    pub payload: MessagePayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingAckRequest {
    pub request_id: String,
    pub project_id: admiralty_core::ProjectId,
    pub recipient: admiralty_core::PrincipalId,
    pub context_epoch: String,
    pub message_id: MessageId,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationReceipt {
    pub message_id: MessageId,
    pub event_seq: String,
    pub duplicate: bool,
    pub execution_granted: bool,
    pub acceptance_granted: bool,
}

fn denied() -> StoreError {
    StoreError::Invalid("native communication request is invalid or unauthorized".into())
}

fn now() -> Result<u64, StoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| denied())
}

fn request_source(
    actor: &AuthenticatedCoordinationActor,
    request_id: &str,
) -> Result<SourceIdentity, StoreError> {
    let parsed = Uuid::parse_str(request_id).map_err(|_| denied())?;
    if parsed.to_string() != request_id || parsed.is_nil() {
        return Err(denied());
    }
    // A principal, rather than a token or registry incarnation, is the stable
    // replay namespace. Credential rotation/restart cannot duplicate a request.
    Ok(SourceIdentity {
        producer_id: format!("native-agent-http-{}", actor.principal()),
        incarnation: request_id.to_owned(),
        sequence: "1".into(),
    })
}

fn receipt(message_id: MessageId, appended: AppendReceipt) -> CommunicationReceipt {
    CommunicationReceipt {
        message_id,
        event_seq: appended.event_seq,
        duplicate: appended.duplicate,
        execution_granted: false,
        acceptance_granted: false,
    }
}

fn request_message_id(
    actor: &AuthenticatedCoordinationActor,
    request_id: &str,
) -> Result<MessageId, StoreError> {
    let digest =
        admiralty_core::content_digest(&(actor.principal(), request_id)).map_err(|_| denied())?;
    MessageId::new(format!("native-{}", digest.trim_start_matches("sha256:"))).map_err(|_| denied())
}

impl NativeRoutingAuthority {
    pub fn new(
        credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
        routes: Vec<InstalledRoute>,
    ) -> Result<Self, StoreError> {
        if routes.len() > MAX_NATIVE_ROUTES {
            return Err(denied());
        }
        let registry = credentials.try_read().map_err(|_| denied())?;
        let mut identities = BTreeSet::new();
        let mut definitions = std::collections::BTreeMap::new();
        for installed in &routes {
            let route = &installed.route;
            route.subscription.validate()?;
            if !registry.has_credential(&installed.credential_id)
                || route.authority_ref.trim().is_empty()
                || route.authority_ref.len() > 256
                || route.authority_ref.chars().any(char::is_control)
                || !identities.insert((
                    installed.credential_id.clone(),
                    route.subscription.subscription_id.clone(),
                ))
            {
                return Err(denied());
            }
            if let Some(previous) = definitions.insert(
                route.subscription.subscription_id.clone(),
                &route.subscription,
            ) && previous != &route.subscription
            {
                return Err(denied());
            }
        }
        drop(registry);
        Ok(Self {
            credentials,
            routes,
        })
    }

    /// Retains the credential registry's read guard until the Store turn
    /// finishes, excluding successful revocation through the SQL commit.
    pub fn send(
        &self,
        store: &Store,
        actor: &AuthenticatedCoordinationActor,
        input: SendMessageRequest,
    ) -> Result<CommunicationReceipt, StoreError> {
        let registry = self.credentials.try_read().map_err(|_| denied())?;
        if !registry.is_active_actor(actor) {
            return Err(denied());
        }
        let route = self
            .routes
            .iter()
            .find(|installed| {
                installed.credential_id == actor.credential_id()
                    && installed.route.subscription.subscription_id == input.route_id
            })
            .ok_or_else(denied)?;
        let source = request_source(actor, &input.request_id)?;
        let message_id = request_message_id(actor, &input.request_id)?;
        let subscription = route.route.subscription.clone();
        let message = CoordinationMessage {
            schema_version: 1,
            message_id: message_id.clone(),
            project_id: subscription.project_id.clone(),
            sender: actor.principal().clone(),
            recipient: subscription.recipient.clone(),
            causation_id: input.causation_id,
            context_epoch: subscription.context_epoch.clone(),
            authority_ref: route.route.authority_ref.clone(),
            payload: input.payload,
        };
        message.validate().map_err(|_| denied())?;
        let timestamp = now()?;
        let proof = Arc::new(RoutingProof {
            actor: actor.clone(),
        });
        store.subscribe(subscription.clone(), timestamp, proof.clone())?;
        let appended = store.route_subscribed(
            subscription,
            timestamp,
            proof,
            source,
            message,
            chrono::Utc::now().to_rfc3339(),
        )?;
        Ok(receipt(message_id, appended))
    }
}

impl SubscriptionAuthority for NativeRoutingAuthority {
    fn authorize(
        &self,
        subscription: &Subscription,
        action: SubscriptionAction<'_>,
        observed_utc: u64,
        current_task: &TaskView,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        let Some(proof) = proof.downcast_ref::<RoutingProof>() else {
            return false;
        };
        let Ok(registry) = self.credentials.try_read() else {
            return false;
        };
        let Ok(current_utc) = now() else {
            return false;
        };
        if !registry.is_active_actor(&proof.actor)
            || observed_utc > current_utc
            || current_utc - observed_utc > 5
            || subscription
                .expires_at_utc
                .is_some_and(|expiry| current_utc >= expiry)
        {
            return false;
        }
        let Some(installed) = self.routes.iter().find(|installed| {
            installed.credential_id == proof.actor.credential_id()
                && installed.route.subscription == *subscription
                && installed.route.authority_ref == current_task.contract.authority_ref
        }) else {
            return false;
        };
        match action {
            SubscriptionAction::Subscribe | SubscriptionAction::Read => true,
            SubscriptionAction::Route { source, message } => {
                source.sequence == "1"
                    && request_source(&proof.actor, &source.incarnation)
                        .is_ok_and(|expected| expected == *source)
                    && message.sender == *proof.actor.principal()
                    && request_message_id(&proof.actor, &source.incarnation)
                        .is_ok_and(|expected| expected == message.message_id)
                    && message.authority_ref == installed.route.authority_ref
            }
        }
    }
}

/// A separately granted own-inbox application receipt. Reading alone cannot ACK;
/// another actor's inbox and caller-selected producer identities are forbidden.
pub fn acknowledge_pending(
    store: &Store,
    actor: &AuthenticatedCoordinationActor,
    credentials: &CoordinationCredentialAuthority,
    input: PendingAckRequest,
) -> Result<CommunicationReceipt, StoreError> {
    let scope = InboxScope {
        project_id: input.project_id,
        recipient: input.recipient,
        context_epoch: input.context_epoch,
    };
    scope.validate()?;
    if !credentials.permits_pending_ack(actor, &scope) {
        return Err(denied());
    }
    let source = request_source(actor, &input.request_id)?;
    let appended = store.acknowledge_scoped_message(
        source,
        scope,
        input.message_id.clone(),
        chrono::Utc::now().to_rfc3339(),
    )?;
    Ok(receipt(input.message_id, appended))
}

#[cfg(all(test, unix))]
#[path = "coordination_routing_tests.rs"]
mod tests;
