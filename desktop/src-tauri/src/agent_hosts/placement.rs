//! How an agent on a paired machine is doing, from the machine's own
//! reports rather than the deploy receipt alone.
//!
//! `backend_agent_id` only says the machine once acknowledged a deploy. The
//! agent can stop there afterwards (`!shutdown`, a crash, a removal on the
//! machine), and the machine says so in its `host.status` telemetry. A
//! report that predates the latest deploy says nothing about that deploy and
//! is ignored.

use super::store::{self, AgentHostRecord};
use crate::managed_agents::ManagedAgentRecord;

/// Summary status of an agent deployed on a machine.
pub const STATUS_DEPLOYED: &str = "deployed";
pub const STATUS_STOPPED: &str = "stopped";
pub const STATUS_NOT_DEPLOYED: &str = "not_deployed";
/// A report must be at least this much newer than the deploy to count:
/// unsolicited reports carry the machine's clock, which can run ahead of
/// this one, and a report from before the deploy must not mark the fresh
/// agent stopped.
pub const CLOCK_SKEW_SECS: u64 = 30;

/// What the summary shows for a machine agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPlacement {
    pub status: &'static str,
    /// The machine's name, when it is still approved here.
    pub host_name: Option<String>,
}

fn unix_secs(iso: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .and_then(|time| u64::try_from(time.timestamp()).ok())
}

/// Status and machine name for `record`, deployed on `host_pubkey`, given
/// the approved machines. `community_relay` resolves an unpinned record.
pub fn host_placement(
    record: &ManagedAgentRecord,
    host_pubkey: &str,
    hosts: &[AgentHostRecord],
    community_relay: &str,
) -> HostPlacement {
    let agent_scope = store::scope_key(&crate::relay::effective_agent_relay_url(
        &record.relay_url,
        community_relay,
    ));
    let approvals: Vec<&AgentHostRecord> = hosts
        .iter()
        .filter(|host| host.pubkey.eq_ignore_ascii_case(host_pubkey))
        .collect();
    let host = approvals
        .iter()
        .find(|host| store::scope_key(&host.relay_url) == agent_scope)
        .or_else(|| approvals.first());
    let host_name = host.map(|host| host.name.clone());
    if record.backend_agent_id.is_none() {
        return HostPlacement {
            status: STATUS_NOT_DEPLOYED,
            host_name,
        };
    }
    let deployed_at = record.last_started_at.as_deref().and_then(unix_secs);
    let report = host
        .and_then(|host| host.status.as_ref())
        // Unknown deploy time: a report cannot be shown to be newer.
        .filter(|report| {
            deployed_at.is_some_and(|deployed| report.received_at >= deployed + CLOCK_SKEW_SECS)
        });
    let status = match report {
        None => STATUS_DEPLOYED,
        Some(report) => match report
            .agents
            .iter()
            .find(|agent| agent.agent_pubkey.eq_ignore_ascii_case(&record.pubkey))
        {
            Some(agent) if agent.state == "running" => STATUS_DEPLOYED,
            // Stopped, failed, or no longer on the machine at all.
            _ => STATUS_STOPPED,
        },
    };
    HostPlacement { status, host_name }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_hosts::frames::HostAgentState;
    use crate::agent_hosts::store::HostStatusSnapshot;

    const HOST: &str = "aa";
    const AGENT: &str = "bb";
    const RELAY: &str = "wss://community.example";
    /// 2026-01-01T00:00:00Z
    const DEPLOYED_AT: u64 = 1_767_225_600;

    fn record(deployed: bool) -> ManagedAgentRecord {
        serde_json::from_value(serde_json::json!({
            "pubkey": AGENT, "name": "Example", "relay_url": RELAY,
            "acp_command": "", "agent_command": "", "agent_args": [],
            "mcp_command": "", "turn_timeout_seconds": 0, "system_prompt": null,
            "private_key_nsec": "", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_started_at": "2026-01-01T00:00:00Z", "last_stopped_at": null,
            "last_exit_code": null, "last_error": null,
            "backend": {"type": "host", "host_pubkey": HOST},
            "backend_agent_id": deployed.then_some(HOST),
        }))
        .unwrap()
    }

    fn host(report: Option<(u64, &[(&str, &str)])>) -> AgentHostRecord {
        AgentHostRecord {
            pubkey: HOST.into(),
            name: "workstation".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            relay_url: RELAY.into(),
            added_at: "2026-01-01T00:00:00Z".into(),
            status: report.map(|(received_at, agents)| HostStatusSnapshot {
                agents: agents
                    .iter()
                    .map(|(pubkey, state)| HostAgentState {
                        agent_pubkey: (*pubkey).into(),
                        state: (*state).into(),
                        since: None,
                    })
                    .collect(),
                received_at,
                ..HostStatusSnapshot::default()
            }),
        }
    }

    fn status(deployed: bool, report: Option<(u64, &[(&str, &str)])>) -> &'static str {
        host_placement(&record(deployed), HOST, &[host(report)], RELAY).status
    }

    #[test]
    fn the_machines_latest_report_decides_running_or_stopped() {
        let after = DEPLOYED_AT + CLOCK_SKEW_SECS + 1;
        assert_eq!(
            status(true, Some((after, &[(AGENT, "running")]))),
            STATUS_DEPLOYED
        );
        assert_eq!(
            status(true, Some((after, &[(AGENT, "stopped")]))),
            STATUS_STOPPED
        );
        assert_eq!(
            status(true, Some((after, &[(AGENT, "failed")]))),
            STATUS_STOPPED
        );
        // The machine no longer has the agent at all.
        assert_eq!(status(true, Some((after, &[]))), STATUS_STOPPED);
    }

    #[test]
    fn a_report_from_before_the_deploy_or_no_report_keeps_the_receipt() {
        assert_eq!(
            status(true, Some((DEPLOYED_AT + 10, &[(AGENT, "stopped")]))),
            STATUS_DEPLOYED,
            "a report within clock skew of the deploy may predate it"
        );
        assert_eq!(
            status(true, Some((DEPLOYED_AT - 10, &[(AGENT, "stopped")]))),
            STATUS_DEPLOYED
        );
        assert_eq!(status(true, None), STATUS_DEPLOYED);
    }

    #[test]
    fn an_undeployed_agent_is_not_deployed_and_names_its_machine() {
        let placement = host_placement(
            &record(false),
            HOST,
            &[host(Some((
                DEPLOYED_AT + CLOCK_SKEW_SECS + 1,
                &[(AGENT, "running")],
            )))],
            RELAY,
        );
        assert_eq!(placement.status, STATUS_NOT_DEPLOYED);
        assert_eq!(placement.host_name.as_deref(), Some("workstation"));
        assert_eq!(
            host_placement(&record(true), HOST, &[], RELAY).host_name,
            None
        );
    }
}
