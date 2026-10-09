//! Remove a managed agent from every channel it belongs to before it is
//! deleted.
//!
//! Memberships are discovered with the agent's OWN key (kind:39002 `#p`),
//! which sees private channels the owner may not be in. Each channel is then
//! cleared with the owner's kind:9001 remove-member (the same event the
//! members sidebar publishes); when the owner may not moderate that channel
//! the agent leaves on its own with kind:9022. A channel neither can clear is
//! reported, never silently skipped, and never blocks deleting the record.
//! DMs are conversations, not memberships, and are left alone.

use futures_util::future::BoxFuture;
use serde::Serialize;

/// A channel the agent was found in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentChannel {
    pub id: String,
    pub name: String,
}

/// A channel the agent could not be removed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelRemovalFailure {
    pub channel_id: String,
    pub channel_name: String,
    pub error: String,
}

/// What the cleanup did. `lookup_error` means the memberships could not be
/// listed at all, so the agent may still be in channels nobody saw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChannelCleanupReport {
    pub removed: Vec<AgentChannel>,
    pub failed: Vec<ChannelRemovalFailure>,
    pub lookup_error: Option<String>,
}

impl ChannelCleanupReport {
    /// Fold another agent's report into this one (persona cascade).
    pub fn merge(&mut self, other: ChannelCleanupReport) {
        self.removed.extend(other.removed);
        self.failed.extend(other.failed);
        if let Some(error) = other.lookup_error {
            self.lookup_error = Some(match self.lookup_error.take() {
                Some(previous) => format!("{previous}; {error}"),
                None => error,
            });
        }
    }
}

/// Relay operations the cleanup needs. Production: [`RelayMembership`];
/// tests substitute a scripted relay.
pub trait MembershipPort: Send + Sync {
    /// Channels (not DMs) the agent is a member of.
    fn agent_channels(&self) -> BoxFuture<'_, Result<Vec<AgentChannel>, String>>;
    /// kind:9001 signed by the owner.
    fn remove_as_owner<'a>(&'a self, channel_id: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// kind:9022 signed by the agent.
    fn leave_as_agent<'a>(&'a self, channel_id: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

/// Remove the agent from every channel `port` lists. Never fails: every
/// problem is in the report.
pub async fn remove_agent_from_channels(port: &dyn MembershipPort) -> ChannelCleanupReport {
    let mut report = ChannelCleanupReport::default();
    let channels = match port.agent_channels().await {
        Ok(channels) => channels,
        Err(error) => {
            tracing::warn!("listing an agent's channels failed: {error}");
            report.lookup_error = Some(error);
            return report;
        }
    };
    for channel in channels {
        let owner_error = match port.remove_as_owner(&channel.id).await {
            Ok(()) => {
                report.removed.push(channel);
                continue;
            }
            Err(error) => error,
        };
        match port.leave_as_agent(&channel.id).await {
            Ok(()) => report.removed.push(channel),
            Err(leave_error) => {
                let error =
                    format!("{owner_error}; the agent could not leave either: {leave_error}");
                tracing::warn!(channel = %channel.id, "removing an agent from a channel failed: {error}");
                report.failed.push(ChannelRemovalFailure {
                    channel_id: channel.id,
                    channel_name: channel.name,
                    error,
                });
            }
        }
    }
    report
}

/// [`MembershipPort`] over the relay's HTTP bridge.
pub struct RelayMembership<'a> {
    pub state: &'a crate::app_state::AppState,
    pub api_base: String,
    pub agent_pubkey: String,
    pub agent_keys: nostr::Keys,
    pub agent_auth_tag: Option<String>,
}

impl RelayMembership<'_> {
    async fn list(&self) -> Result<Vec<AgentChannel>, String> {
        let memberships = crate::relay::query_relay_at_with_keys(
            self.state,
            &self.api_base,
            &[serde_json::json!({"kinds": [39002], "#p": [self.agent_pubkey]})],
            &self.agent_keys,
            self.agent_auth_tag.as_deref(),
        )
        .await?;
        let ids: Vec<String> = memberships
            .iter()
            .filter_map(|event| {
                event
                    .tags
                    .iter()
                    .find(|tag| tag.kind().to_string() == "d")
                    .and_then(|tag| tag.content())
                    .map(str::to_string)
            })
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let metadata = crate::relay::query_relay_at_with_keys(
            self.state,
            &self.api_base,
            &[serde_json::json!({"kinds": [39000], "#d": ids})],
            &self.agent_keys,
            self.agent_auth_tag.as_deref(),
        )
        .await?;
        Ok(ids
            .into_iter()
            .filter_map(|id| {
                let info = metadata.iter().find_map(|event| {
                    crate::nostr_convert::channel_info_from_event(event, None, None)
                        .ok()
                        .filter(|info| info.id == id)
                });
                match info {
                    Some(info) if info.channel_type == "dm" => None,
                    Some(info) => Some(AgentChannel {
                        id,
                        name: info.name,
                    }),
                    None => Some(AgentChannel {
                        name: id.clone(),
                        id,
                    }),
                }
            })
            .collect())
    }
}

fn channel_uuid(channel_id: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(channel_id).map_err(|_| format!("invalid channel id {channel_id}"))
}

impl MembershipPort for RelayMembership<'_> {
    fn agent_channels(&self) -> BoxFuture<'_, Result<Vec<AgentChannel>, String>> {
        Box::pin(self.list())
    }

    fn remove_as_owner<'a>(&'a self, channel_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let builder =
                crate::events::build_remove_member(channel_uuid(channel_id)?, &self.agent_pubkey)?;
            crate::relay::submit_event_at(builder, self.state, &self.api_base)
                .await
                .map(|_| ())
        })
    }

    fn leave_as_agent<'a>(&'a self, channel_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let event = crate::events::build_leave(channel_uuid(channel_id)?)?
                .sign_with_keys(&self.agent_keys)
                .map_err(|error| format!("failed to sign leave: {error}"))?;
            crate::relay::submit_signed_event_with_keys_at(
                &event,
                self.state,
                &self.api_base,
                &self.agent_keys,
                self.agent_auth_tag.as_deref(),
            )
            .await
            .map(|_| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct ScriptedRelay {
        channels: Result<Vec<AgentChannel>, String>,
        /// Channel ids where the owner may not remove members.
        owner_refused: Vec<&'static str>,
        /// Channel ids the agent may not leave.
        leave_refused: Vec<&'static str>,
        calls: Mutex<Vec<String>>,
    }

    impl MembershipPort for ScriptedRelay {
        fn agent_channels(&self) -> BoxFuture<'_, Result<Vec<AgentChannel>, String>> {
            Box::pin(async move { self.channels.clone() })
        }
        fn remove_as_owner<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(format!("9001 {id}"));
                if self.owner_refused.contains(&id) {
                    Err("not a member".into())
                } else {
                    Ok(())
                }
            })
        }
        fn leave_as_agent<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(format!("9022 {id}"));
                if self.leave_refused.contains(&id) {
                    Err("sole owner".into())
                } else {
                    Ok(())
                }
            })
        }
    }

    fn channel(id: &str) -> AgentChannel {
        AgentChannel {
            id: id.into(),
            name: format!("#{id}"),
        }
    }

    #[tokio::test]
    async fn every_channel_is_cleared_falling_back_to_the_agent_leaving() {
        let relay = ScriptedRelay {
            channels: Ok(vec![channel("a"), channel("b"), channel("c")]),
            owner_refused: vec!["b", "c"],
            leave_refused: vec!["c"],
            calls: Mutex::new(Vec::new()),
        };
        let report = remove_agent_from_channels(&relay).await;
        assert_eq!(report.removed, vec![channel("a"), channel("b")]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].channel_id, "c");
        assert_eq!(report.failed[0].channel_name, "#c");
        assert!(report.failed[0].error.contains("not a member"));
        assert!(report.failed[0].error.contains("sole owner"));
        assert_eq!(
            *relay.calls.lock().unwrap(),
            vec!["9001 a", "9001 b", "9022 b", "9001 c", "9022 c"]
        );
    }

    #[test]
    fn merged_reports_keep_every_failure_and_lookup_error() {
        let mut total = ChannelCleanupReport {
            removed: vec![channel("a")],
            lookup_error: Some("first".into()),
            ..ChannelCleanupReport::default()
        };
        total.merge(ChannelCleanupReport {
            failed: vec![ChannelRemovalFailure {
                channel_id: "b".into(),
                channel_name: "#b".into(),
                error: "denied".into(),
            }],
            lookup_error: Some("second".into()),
            ..ChannelCleanupReport::default()
        });
        assert_eq!(total.removed, vec![channel("a")]);
        assert_eq!(total.failed.len(), 1);
        assert_eq!(total.lookup_error.as_deref(), Some("first; second"));
    }

    #[tokio::test]
    async fn a_failed_lookup_is_reported_not_treated_as_no_channels() {
        let relay = ScriptedRelay {
            channels: Err("relay unreachable".into()),
            owner_refused: vec![],
            leave_refused: vec![],
            calls: Mutex::new(Vec::new()),
        };
        let report = remove_agent_from_channels(&relay).await;
        assert_eq!(report.lookup_error.as_deref(), Some("relay unreachable"));
        assert!(relay.calls.lock().unwrap().is_empty());
    }
}
