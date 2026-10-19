//! Explicit startup grants for exact incident scopes and authenticated evidence.
use admiralty_core::{
    content_digest,
    excursions::{ExcursionScope, reduce_excursions},
};
use admiralty_store::excursions::{
    ExcursionAction, ExcursionAuthority, ExcursionCommand, ExcursionOperation, ExcursionProof,
};
use serde::Deserialize;
use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcursionGrant {
    pub scope: ExcursionScope,
    pub actions: Vec<ExcursionAction>,
    /// Trusted immutable references for this exact scope. No transcript access.
    pub evidence_digests: Vec<String>,
}
struct Actor(uuid::Uuid);
pub struct ExcursionPolicy {
    token_digest: String,
    identity: uuid::Uuid,
    grants: Vec<ExcursionGrant>,
    active: AtomicBool,
}
impl ExcursionPolicy {
    pub fn new(token: &str, grants: Vec<ExcursionGrant>) -> Result<Self, &'static str> {
        if token.len() < 32 || token.len() > 4096 || grants.is_empty() || grants.len() > 128 {
            return Err("invalid excursion configuration");
        }
        for (i, grant) in grants.iter().enumerate() {
            reduce_excursions(&None, &grant.scope, &[]).map_err(|_| "invalid excursion scope")?;
            if grant.actions.is_empty()
                || grant.actions.len() > 3
                || grant.evidence_digests.len() > 1024
                || grants[..i].iter().any(|g| g.scope == grant.scope)
                || grant.evidence_digests.iter().any(|d| {
                    !d.strip_prefix("sha256:").is_some_and(|h| {
                        h.len() == 64
                            && h.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                })
            {
                return Err("invalid excursion grant");
            }
        }
        Ok(Self {
            token_digest: content_digest(&token).map_err(|_| "invalid excursion credential")?,
            identity: uuid::Uuid::new_v4(),
            grants,
            active: AtomicBool::new(true),
        })
    }
    pub(super) fn scopes(&self, proof: &(dyn Any + Send + Sync)) -> Vec<ExcursionScope> {
        self.grants
            .iter()
            .filter(|g| self.authorize(&g.scope, ExcursionAction::Read, None, proof))
            .map(|g| g.scope.clone())
            .collect()
    }
    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }
    pub(super) fn authenticate(&self, token: &str) -> Option<ExcursionProof> {
        (self.active.load(Ordering::Acquire)
            && token.len() <= 4096
            && content_digest(&token).ok().as_ref() == Some(&self.token_digest))
        .then(|| Arc::new(Actor(self.identity)) as ExcursionProof)
    }
}
impl ExcursionAuthority for ExcursionPolicy {
    fn authorize(
        &self,
        scope: &ExcursionScope,
        action: ExcursionAction,
        command: Option<&ExcursionCommand>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        if !self.active.load(Ordering::Acquire)
            || proof
                .downcast_ref::<Actor>()
                .is_none_or(|a| a.0 != self.identity)
        {
            return false;
        }
        let Some(grant) = self
            .grants
            .iter()
            .find(|g| g.scope == *scope && g.actions.contains(&action))
        else {
            return false;
        };
        match command.map(|c| &c.operation) {
            Some(ExcursionOperation::Observe { incidents }) => incidents.iter().all(|i| {
                i.scope == *scope
                    && i.event
                        .evidence_digests
                        .iter()
                        .all(|d| grant.evidence_digests.contains(d))
            }),
            _ => true,
        }
    }
}
