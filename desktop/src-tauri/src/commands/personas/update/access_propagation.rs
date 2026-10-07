//! Template (agent definition) access edits reaching existing instances.
//!
//! A definition's access policy is copied onto an instance when it is minted.
//! When the template's policy changes later, an instance that still runs the
//! template's previous policy follows the edit; an instance whose access was
//! set individually keeps it.
//!
//! The template Save does not wait for the instances: each saved change is
//! queued in save order and applied in the background, one change at a time.
//! The instances to move are selected when the change is applied, not when it
//! was saved, so a quick second edit (A→B, then B→C) moves every instance that
//! followed the first edit on to C.

use std::{collections::VecDeque, sync::Mutex};

use crate::managed_agents::{
    AgentDefinition, ManagedAgentRecord, RespondTo, UpdateManagedAgentRequest,
};

/// An access policy as the runtime gate reads it: the allowlist only counts
/// in allowlist mode.
pub(crate) type AccessPolicy = (RespondTo, Vec<String>);

/// The access policy an instance minted from `definition` receives (absent
/// means the instance default, owner-only). `None` when the stored wire value
/// is not a mode this build understands: nothing is propagated from it.
pub(super) fn definition_access_policy(definition: &AgentDefinition) -> Option<AccessPolicy> {
    let mode = match definition.respond_to.as_deref() {
        None => RespondTo::default(),
        Some(wire) => RespondTo::parse_wire(wire).ok()?,
    };
    let allowlist = if mode == RespondTo::Allowlist {
        definition.respond_to_allowlist.clone()
    } else {
        Vec::new()
    };
    Some((mode, allowlist))
}

/// A saved template access change waiting to reach its instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccessChange {
    pub(super) definition_id: String,
    /// The community the template was edited in; instance edits run there.
    pub(super) community_relay: String,
    pub(super) previous: AccessPolicy,
    pub(super) next: AccessPolicy,
}

/// The change a template edit from `previous` to `next` makes, if any.
/// `None` when the policy did not change, or either side could not be read.
pub(super) fn access_change(
    definition_id: &str,
    community_relay: &str,
    previous: Option<AccessPolicy>,
    next: Option<AccessPolicy>,
) -> Option<AccessChange> {
    let (previous, next) = (previous?, next?);
    (previous != next).then(|| AccessChange {
        definition_id: definition_id.to_string(),
        community_relay: community_relay.to_string(),
        previous,
        next,
    })
}

/// Saved template access changes, applied in save order by one runner at a
/// time.
pub(super) struct AccessChangeQueue {
    changes: Mutex<VecDeque<AccessChange>>,
    turn: tokio::sync::Mutex<()>,
}

impl AccessChangeQueue {
    pub(super) const fn new() -> Self {
        Self {
            changes: Mutex::new(VecDeque::new()),
            turn: tokio::sync::Mutex::const_new(()),
        }
    }

    /// Queue a saved change. Call it while the store lock that saved the
    /// template is still held, so queue order is save order.
    pub(super) fn push(&self, change: AccessChange) {
        self.changes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(change);
    }

    fn pop(&self) -> Option<AccessChange> {
        self.changes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front()
    }

    /// Apply every queued change in order with `apply`, holding the runner
    /// turn so no other runner interleaves. A runner started for a change an
    /// earlier runner already applied finds the queue empty.
    pub(super) async fn drain<F, Fut>(&self, mut apply: F)
    where
        F: FnMut(AccessChange) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let _turn = self.turn.lock().await;
        while let Some(change) = self.pop() {
            apply(change).await;
        }
    }
}

/// The process-wide queue of template access changes.
pub(super) static ACCESS_CHANGES: AccessChangeQueue = AccessChangeQueue::new();

/// Instances to move from the template's `previous` policy to `next`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AccessPropagation {
    pub(super) previous: AccessPolicy,
    pub(super) next: AccessPolicy,
    pub(super) pubkeys: Vec<String>,
}

impl AccessPropagation {
    /// The instance edit that applies `next`; the caller fences it on
    /// `previous` so an instance changed meanwhile is left alone.
    pub(super) fn request_for(&self, pubkey: &str) -> UpdateManagedAgentRequest {
        let (mode, allowlist) = &self.next;
        UpdateManagedAgentRequest {
            pubkey: pubkey.to_string(),
            respond_to: Some(*mode),
            respond_to_allowlist: (*mode == RespondTo::Allowlist).then(|| allowlist.clone()),
            ..UpdateManagedAgentRequest::default()
        }
    }
}

/// Select the linked instances of `definition_id` that still carry the
/// template's `previous` policy. `None` when the policy did not change, or
/// either side could not be read.
pub(super) fn plan_access_propagation(
    records: &[ManagedAgentRecord],
    definition_id: &str,
    previous: Option<AccessPolicy>,
    next: Option<AccessPolicy>,
) -> Option<AccessPropagation> {
    let (previous, next) = (previous?, next?);
    if previous == next {
        return None;
    }
    let pubkeys: Vec<String> = records
        .iter()
        .filter(|record| !record.pubkey.is_empty())
        .filter(|record| record.persona_id.as_deref() == Some(definition_id))
        .filter(|record| crate::commands::record_has_access_policy(record, previous.0, &previous.1))
        .map(|record| record.pubkey.clone())
        .collect();
    (!pubkeys.is_empty()).then_some(AccessPropagation {
        previous,
        next,
        pubkeys,
    })
}

#[cfg(test)]
#[path = "access_propagation_tests.rs"]
mod tests;
