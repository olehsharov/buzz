//! Tauri commands for agent hosts (`buzz host` machines).

use tauri::{AppHandle, State};

use crate::{
    agent_hosts::{channel::RelayHostChannel, frames, ops, store::AgentHostRecord, HostOps},
    app_state::AppState,
    relay::relay_ws_url_with_override,
};

/// Install script for `buzz host`. Set `BUZZ_HOST_INSTALL_URL` at build time
/// to point desktop builds at your distribution; the default placeholder
/// below is deliberately unroutable so a misconfigured build is obvious.
pub const HOST_INSTALL_URL: &str = match option_env!("BUZZ_HOST_INSTALL_URL") {
    Some(url) => url,
    None => "https://example.invalid/buzz-host/install.sh",
};

fn community_relay(state: &AppState) -> String {
    relay_ws_url_with_override(state)
}

fn relay_channel(state: &AppState) -> Result<RelayHostChannel, String> {
    Ok(RelayHostChannel {
        relay_url: community_relay(state),
        owner_keys: state.signing_keys()?,
    })
}

/// The one-liner shown in "Add machine".
pub(crate) fn host_install_command(install_url: &str, relay_url: &str) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', r"'\''"));
    format!(
        "curl -fsSL {} | sh -s -- --relay {}",
        quote(install_url),
        quote(relay_url)
    )
}

#[derive(serde::Serialize)]
pub struct HostInstallInfo {
    pub install_url: String,
    pub command: String,
    pub relay_url: String,
}

#[tauri::command]
pub fn get_host_install_info(state: State<'_, AppState>) -> HostInstallInfo {
    let relay_url = community_relay(&state);
    HostInstallInfo {
        install_url: HOST_INSTALL_URL.to_string(),
        command: host_install_command(HOST_INSTALL_URL, &relay_url),
        relay_url,
    }
}

/// Approved machines in the active community.
#[tauri::command]
pub fn list_agent_hosts(
    app: AppHandle,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<Vec<AgentHostRecord>, String> {
    ops::list_hosts(&app, &hosts, &community_relay(&state))
}

/// Deploy (or redeploy, or move) an agent onto an approved machine and wait
/// for the machine's acknowledgement.
#[tauri::command]
pub async fn deploy_to_host(
    pubkey: String,
    host_pubkey: String,
    app: AppHandle,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<crate::managed_agents::ManagedAgentSummary, String> {
    let channel = relay_channel(&state)?;
    let relay = community_relay(&state);
    ops::deploy_agent_to_host(
        &app,
        &state,
        &hosts,
        &channel,
        &pubkey,
        &host_pubkey,
        &relay,
        |record| super::agents::build_deploy_payload(&app, &state, record),
    )
    .await?;
    summary_for(&app, &state, &pubkey)
}

/// Remove an agent from its machine (requires the machine's ack).
#[tauri::command]
pub async fn undeploy_from_host(
    pubkey: String,
    app: AppHandle,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<crate::managed_agents::ManagedAgentSummary, String> {
    let channel = relay_channel(&state)?;
    ops::undeploy_agent_from_host(&app, &state, &hosts, &channel, &pubkey).await?;
    summary_for(&app, &state, &pubkey)
}

/// Ask a machine for fresh `host.status` and return the updated record.
#[tauri::command]
pub async fn request_host_status(
    host_pubkey: String,
    app: AppHandle,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<AgentHostRecord, String> {
    let channel = relay_channel(&state)?;
    let relay = community_relay(&state);
    ops::refresh_host_status(&app, &state, &hosts, &channel, &relay, &host_pubkey).await
}

/// Tell the machine to forget this owner, then remove it locally.
#[tauri::command]
pub async fn forget_host(
    host_pubkey: String,
    app: AppHandle,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<ops::ForgetHostOutcome, String> {
    let channel = relay_channel(&state)?;
    let relay = community_relay(&state);
    ops::forget_host(&app, &state, &hosts, &channel, &relay, &host_pubkey).await
}

/// Fold an unsolicited telemetry frame (periodic `host.status`) received on
/// the renderer's observer subscription into the host store. The event is
/// verified here; the renderer is only the courier. Returns the updated host
/// when the frame was an accepted status, `None` otherwise.
#[tauri::command]
pub fn ingest_host_telemetry(
    event_json: String,
    app: AppHandle,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<Option<AgentHostRecord>, String> {
    use nostr::JsonUtil;
    let event =
        nostr::Event::from_json(event_json).map_err(|error| format!("invalid event: {error}"))?;
    let relay = community_relay(&state);
    let known = ops::list_hosts(&app, &hosts, &relay)?;
    if !known
        .iter()
        .any(|host| host.pubkey.eq_ignore_ascii_case(&event.pubkey.to_hex()))
    {
        return Ok(None);
    }
    let owner_keys = state.signing_keys()?;
    match frames::parse_host_telemetry(&owner_keys, &event.pubkey, &event, frames::now_secs())? {
        Some(frames::HostTelemetry::Status(status)) => ops::apply_status_to_store(
            &app,
            &hosts,
            &relay,
            &event.pubkey.to_hex(),
            &status,
            event.created_at.as_secs(),
        )
        .map(Some),
        _ => Ok(None),
    }
}

fn summary_for(
    app: &AppHandle,
    state: &AppState,
    pubkey: &str,
) -> Result<crate::managed_agents::ManagedAgentSummary, String> {
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let records = crate::managed_agents::load_managed_agents(app)?;
    let runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|e| e.to_string())?;
    let record = records
        .iter()
        .find(|record| record.pubkey == pubkey)
        .ok_or_else(|| format!("agent {pubkey} not found"))?;
    super::agents::summarize_from_disk(app, record, &runtimes)
}

#[cfg(test)]
mod tests {
    use super::host_install_command;

    #[test]
    fn install_command_quotes_relay_and_url() {
        assert_eq!(
            host_install_command("https://get.example/install.sh", "wss://relay.example"),
            "curl -fsSL 'https://get.example/install.sh' | sh -s -- --relay 'wss://relay.example'"
        );
        assert!(host_install_command("u", "wss://x'; rm -rf ~").contains(r"'\''"));
    }
}
