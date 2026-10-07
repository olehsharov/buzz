//! Template (agent definition) access edits reaching existing instances.
//!
//! A definition's access policy is copied onto an instance when it is minted.
//! When the template's policy changes later, an instance that still runs the
//! template's previous policy follows the edit; an instance whose access was
//! set individually keeps it.

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
