//! Production spawn-key derivation — split from `runtime.rs` (file-size
//! guard). The regression tests live beside the function so they exercise
//! the exact seam production spawn keys on.

use crate::managed_agents::types::ManagedAgentRecord;
use crate::managed_agents::ManagedAgentRuntimeKey;

/// The one production derivation from a caller-bound workspace relay to the
/// runtime-pair key `start_managed_agent_process` spawns and persists under.
/// Extracted so the regression suite exercises the exact seam production
/// uses: a mutation that keys the spawn to anything but the bound value now
/// fails the tests below, instead of leaving them green while a painted
/// guard watches the door.
pub(crate) fn bound_runtime_key(
    record: &ManagedAgentRecord,
    workspace_relay: &crate::relay::ScopedWorkspaceRelay,
) -> Result<ManagedAgentRuntimeKey, String> {
    // An agent belongs to ONE community: refuse to spawn it on any other
    // relay rather than key a pair where it must not run.
    crate::relay::ensure_agent_belongs_to_relay(
        &record.name,
        &record.relay_url,
        workspace_relay.as_str(),
        workspace_relay.as_str(),
    )?;
    ManagedAgentRuntimeKey::new(record.pubkey.clone(), workspace_relay.as_str())
}

#[cfg(test)]
mod tests {
    use super::bound_runtime_key;
    use crate::managed_agents::types::ManagedAgentRecord;

    fn record(pubkey: &str, relay_url: &str) -> ManagedAgentRecord {
        serde_json::from_value(serde_json::json!({
            "pubkey": pubkey,
            "name": "test",
            "private_key_nsec": "nsec1fake",
            "relay_url": relay_url,
            "acp_command": "buzz-acp",
            "agent_command": "buzz-agent",
            "agent_args": [],
            "mcp_command": "",
            "turn_timeout_seconds": 320,
            "created_at": "",
            "updated_at": ""
        }))
        .expect("record fixture")
    }

    #[test]
    fn production_spawn_key_derives_from_the_bound_relay_not_the_post_switch_workspace() {
        // Round-8 regression: the previous test reconstructed the key
        // derivation by hand, so hard-coding a wrong tenant inside production
        // spawn stayed green. This calls `bound_runtime_key` — the exact
        // function `start_managed_agent_process` keys its spawn, receipt, and
        // runtimes-map insert on — so that mutation now fails here.
        let record = record(&"aa".repeat(32), ""); // never-pinned record
        let mut workspace = "wss://tenant-a.example".to_string();
        let bound = crate::relay::bind_expected_relay_scope(
            Some("wss://tenant-a.example"),
            workspace.clone(),
        )
        .expect("scope matches at bind time");
        workspace = "wss://tenant-b.example".to_string(); // the switch lands post-check

        let key = bound_runtime_key(&record, &bound).expect("keyable record and relay");
        assert_eq!(key.relay_url, "wss://tenant-a.example");
        assert_eq!(key.pubkey, "aa".repeat(32));
        assert_ne!(
            key.relay_url, workspace,
            "the production spawn key must be unrepresentable for the post-switch tenant"
        );
    }

    #[test]
    fn production_spawn_key_refuses_an_agent_from_another_community() {
        // Agents belong to ONE community (narrows #2122): an agent pinned to
        // another relay must never be keyed (and so never spawned) here.
        let record = record(&"bb".repeat(32), "wss://other-community.example");
        let bound =
            crate::relay::bind_expected_relay_scope(None, "wss://tenant-a.example".to_string())
                .expect("unscoped bind");

        let error = bound_runtime_key(&record, &bound).expect_err("foreign agent refused");
        assert!(error.contains("belongs to the community on wss://other-community.example"));
    }

    #[test]
    fn production_spawn_key_accepts_the_agents_own_community() {
        let record = record(&"bb".repeat(32), "WSS://Tenant-A.example:443/");
        let bound =
            crate::relay::bind_expected_relay_scope(None, "wss://tenant-a.example".to_string())
                .expect("unscoped bind");

        let key = bound_runtime_key(&record, &bound).expect("own community");
        assert_eq!(key.relay_url, "wss://tenant-a.example");
    }
}
