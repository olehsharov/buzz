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

use std::time::Duration;

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

/// Each relay request gets this long (the HTTP client has no timeout).
pub const RELAY_OP_TIMEOUT: Duration = Duration::from_secs(15);
/// The whole cleanup gets this long; channels not reached by then are
/// reported, so a slow relay cannot hold a delete indefinitely.
pub const CLEANUP_BUDGET: Duration = Duration::from_secs(90);

async fn bounded<T>(
    operation: BoxFuture<'_, Result<T, String>>,
    deadline: tokio::time::Instant,
) -> Result<T, String> {
    let limit = deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(RELAY_OP_TIMEOUT);
    tokio::time::timeout(limit, operation)
        .await
        .map_err(|_| "the relay did not answer in time".to_string())?
}

/// Remove the agent from every channel `port` lists. Never fails: every
/// problem is in the report.
pub async fn remove_agent_from_channels(port: &dyn MembershipPort) -> ChannelCleanupReport {
    let deadline = tokio::time::Instant::now() + CLEANUP_BUDGET;
    let mut report = ChannelCleanupReport::default();
    let channels = match bounded(port.agent_channels(), deadline).await {
        Ok(channels) => channels,
        Err(error) => {
            tracing::warn!("listing an agent's channels failed: {error}");
            report.lookup_error = Some(error);
            return report;
        }
    };
    for channel in channels {
        if tokio::time::Instant::now() >= deadline {
            report.failed.push(ChannelRemovalFailure {
                channel_id: channel.id,
                channel_name: channel.name,
                error: "not attempted: the relay was too slow".into(),
            });
            continue;
        }
        let owner_error = match bounded(port.remove_as_owner(&channel.id), deadline).await {
            Ok(()) => {
                report.removed.push(channel);
                continue;
            }
            Err(error) => error,
        };
        match bounded(port.leave_as_agent(&channel.id), deadline).await {
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

/// Page size for membership and metadata queries (the relay serves 100 rows
/// when a filter names no limit, at most 1000).
pub const MEMBERSHIP_PAGE_SIZE: usize = 500;

fn d_tag(event: &nostr::Event) -> Option<String> {
    event
        .tags
        .iter()
        .find(|tag| tag.kind().to_string() == "d")
        .and_then(|tag| tag.content())
        .map(str::to_string)
}

/// Every event matching `filter`, page by page on the relay's composite
/// `(until, before_id)` cursor (as `get_channels` pages its directory), so a
/// long membership list is never silently cut at one page.
pub async fn query_all_pages<F, Fut>(
    mut filter: serde_json::Value,
    mut query: F,
) -> Result<Vec<nostr::Event>, String>
where
    F: FnMut(serde_json::Value) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<nostr::Event>, String>>,
{
    filter["limit"] = serde_json::json!(MEMBERSHIP_PAGE_SIZE);
    let mut all = Vec::new();
    loop {
        let page = query(filter.clone()).await?;
        let full = page.len() >= MEMBERSHIP_PAGE_SIZE;
        if let Some(last) = page.last().filter(|_| full) {
            filter["until"] = serde_json::json!(last.created_at.as_secs());
            filter["before_id"] = serde_json::json!(last.id.to_hex());
        }
        all.extend(page);
        if !full {
            return Ok(all);
        }
    }
}

impl RelayMembership<'_> {
    async fn query(&self, filter: serde_json::Value) -> Result<Vec<nostr::Event>, String> {
        crate::relay::query_relay_at_with_keys(
            self.state,
            &self.api_base,
            &[filter],
            &self.agent_keys,
            self.agent_auth_tag.as_deref(),
        )
        .await
    }

    async fn list(&self) -> Result<Vec<AgentChannel>, String> {
        let memberships = query_all_pages(
            serde_json::json!({"kinds": [39002], "#p": [self.agent_pubkey]}),
            |filter| self.query(filter),
        )
        .await?;
        let mut ids: Vec<String> = memberships.iter().filter_map(d_tag).collect();
        ids.sort();
        ids.dedup();
        let mut metadata = Vec::new();
        for chunk in ids.chunks(MEMBERSHIP_PAGE_SIZE) {
            metadata.extend(
                self.query(serde_json::json!({
                    "kinds": [39000],
                    "#d": chunk,
                    "limit": MEMBERSHIP_PAGE_SIZE,
                }))
                .await?,
            );
        }
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
        /// Channel ids where the relay never answers.
        silent: Vec<&'static str>,
        calls: Mutex<Vec<String>>,
    }

    impl MembershipPort for ScriptedRelay {
        fn agent_channels(&self) -> BoxFuture<'_, Result<Vec<AgentChannel>, String>> {
            Box::pin(async move { self.channels.clone() })
        }
        fn remove_as_owner<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(format!("9001 {id}"));
                if self.silent.contains(&id) {
                    std::future::pending::<()>().await;
                }
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
                if self.silent.contains(&id) {
                    std::future::pending::<()>().await;
                }
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
            silent: vec![],
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

    #[tokio::test]
    async fn memberships_are_read_past_the_first_page() {
        let keys = nostr::Keys::generate();
        let event = |n: usize| {
            nostr::EventBuilder::new(nostr::Kind::Custom(39002), "")
                .tags([nostr::Tag::identifier(format!("ch{n}"))])
                .sign_with_keys(&keys)
                .unwrap()
        };
        let pages = [
            (0..MEMBERSHIP_PAGE_SIZE).map(event).collect::<Vec<_>>(),
            (0..3).map(|n| event(MEMBERSHIP_PAGE_SIZE + n)).collect(),
        ];
        let seen = Mutex::new(Vec::new());
        let all = query_all_pages(serde_json::json!({"kinds": [39002]}), |filter| {
            let page_index = {
                let mut seen = seen.lock().unwrap();
                seen.push(filter);
                seen.len() - 1
            };
            let page = pages[page_index].clone();
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(all.len(), MEMBERSHIP_PAGE_SIZE + 3);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0]["limit"], MEMBERSHIP_PAGE_SIZE);
        let last = pages[0].last().unwrap();
        assert_eq!(seen[1]["until"], last.created_at.as_secs());
        assert_eq!(seen[1]["before_id"], last.id.to_hex());
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_relay_is_bounded_and_every_channel_is_still_reported() {
        let ids: Vec<String> = (0..20).map(|n| format!("ch{n}")).collect();
        let leaked: &'static [String] = Box::leak(ids.into_boxed_slice());
        let relay = ScriptedRelay {
            channels: Ok(leaked.iter().map(|id| channel(id)).collect()),
            owner_refused: vec![],
            leave_refused: vec![],
            silent: leaked.iter().map(String::as_str).collect(),
            calls: Mutex::new(Vec::new()),
        };
        let started = tokio::time::Instant::now();
        let report = remove_agent_from_channels(&relay).await;
        assert!(started.elapsed() <= CLEANUP_BUDGET + RELAY_OP_TIMEOUT);
        assert!(report.removed.is_empty());
        assert_eq!(report.failed.len(), 20, "no channel goes unreported");
        assert!(report.failed[0].error.contains("did not answer in time"));
        assert!(report
            .failed
            .last()
            .unwrap()
            .error
            .contains("not attempted"));
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
            silent: vec![],
            calls: Mutex::new(Vec::new()),
        };
        let report = remove_agent_from_channels(&relay).await;
        assert_eq!(report.lookup_error.as_deref(), Some("relay unreachable"));
        assert!(relay.calls.lock().unwrap().is_empty());
    }
}
