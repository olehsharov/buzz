//! Tauri commands for agent hosts (`buzz host` machines).

use tauri::{AppHandle, State};

use crate::{
    agent_hosts::{channel::RelayHostChannel, frames, ops, store::AgentHostRecord, HostOps},
    app_state::AppState,
    relay::relay_http_base_url,
};

/// Machines are approved per community: every host command acts on the
/// invoking window's community relay.
fn community_relay(relay: &crate::window_relay::WindowRelay) -> String {
    relay.ws_url().to_string()
}

fn relay_channel(
    state: &AppState,
    relay: &crate::window_relay::WindowRelay,
) -> Result<RelayHostChannel, String> {
    Ok(RelayHostChannel {
        relay_url: community_relay(relay),
        owner_keys: state.signing_keys()?,
    })
}

/// Where the community relay serves the agent-host installer: the relay
/// image bundles `install.sh`, the Sprig tarballs and `SHA256SUMS` under
/// `/host/` on the same origin as the relay (`ws` → `http`, `wss` → `https`).
pub(crate) fn host_install_base(relay_ws_url: &str) -> String {
    let http = relay_http_base_url(relay_ws_url);
    let origin = url::Url::parse(&http)
        .ok()
        .filter(|url| url.has_host())
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or(http);
    format!("{origin}/host")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The one line shown in "Add machine": download the installer from the
/// relay, install `buzz host` and everything a Claude agent needs, then pair
/// with `pairing_uri`. Without a URI it is the repair line for an already
/// paired machine (upgrade, reinstall tools, restart the daemon). It never
/// passes `--relay`, which would override the pairing relay in the URI.
/// The machine names itself (its hostname) unless the user adds `--name`.
pub(crate) fn host_install_command(base: &str, pairing_uri: Option<&str>) -> String {
    let mut command = format!(
        "curl -fsSL {} | bash -s -- --base {}",
        shell_quote(&format!("{base}/install.sh")),
        shell_quote(base),
    );
    if let Some(uri) = pairing_uri.filter(|uri| !uri.is_empty()) {
        command.push_str(" --uri ");
        command.push_str(&shell_quote(uri));
    }
    command
}

#[derive(serde::Serialize)]
pub struct HostInstallInfo {
    pub base_url: String,
    pub command: String,
    pub session_ttl_secs: u64,
}

/// The install line for one pairing session, or (no `pairing_uri`) the
/// repair line for machines that are already paired.
#[tauri::command]
pub fn get_host_install_info(
    relay: crate::window_relay::WindowRelay,
    pairing_uri: Option<String>,
) -> HostInstallInfo {
    let base_url = host_install_base(&community_relay(&relay));
    HostInstallInfo {
        command: host_install_command(&base_url, pairing_uri.as_deref()),
        base_url,
        session_ttl_secs: super::pairing::PAIRING_SESSION_TIMEOUT.as_secs(),
    }
}

/// Approved machines in the active community.
#[tauri::command]
pub fn list_agent_hosts(
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
    hosts: State<'_, HostOps>,
) -> Result<Vec<AgentHostRecord>, String> {
    ops::list_hosts(&app, &hosts, &community_relay(&relay))
}

/// Deploy (or redeploy, or move) an agent onto an approved machine and wait
/// for the machine's acknowledgement.
#[tauri::command]
pub async fn deploy_to_host(
    pubkey: String,
    host_pubkey: String,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<crate::managed_agents::ManagedAgentSummary, String> {
    let channel = relay_channel(&state, &relay)?;
    let relay = community_relay(&relay);
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

/// Change the folder an agent runs in on its machine (blank: the machine's
/// default). A deployed agent is redeployed so it restarts in the new
/// folder; when that fails the folder stays saved and the redeploy is
/// retried the next time this community loads.
#[tauri::command]
pub async fn set_host_agent_workdir(
    pubkey: String,
    workdir: Option<String>,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<crate::managed_agents::ManagedAgentSummary, String> {
    let channel = relay_channel(&state, &relay)?;
    let relay = community_relay(&relay);
    let result = ops::set_host_agent_workdir(
        &app,
        &state,
        &hosts,
        &channel,
        &pubkey,
        workdir.as_deref(),
        &relay,
        |record| super::agents::build_deploy_payload(&app, &state, record),
    )
    .await;
    {
        use tauri::Emitter;
        // The row's folder, pending mark and error change either way.
        let _ = app.emit("agents-data-changed", ());
    }
    result?;
    summary_for(&app, &state, &pubkey)
}

/// Remove an agent from its machine (requires the machine's ack).
#[tauri::command]
pub async fn undeploy_from_host(
    pubkey: String,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<crate::managed_agents::ManagedAgentSummary, String> {
    let owner_keys = state.signing_keys()?;
    let held = hosts.hold(&pubkey).await?;
    ops::undeploy_agent_via_its_host(
        &app,
        &state,
        &hosts,
        &held,
        &community_relay(&relay),
        &pubkey,
        |route| {
            Ok(RelayHostChannel {
                relay_url: route.relay_url.clone(),
                owner_keys,
            })
        },
    )
    .await
    .map_err(ops::UndeployError::into_message)?;
    drop(held);
    summary_for(&app, &state, &pubkey)
}

/// Ask a machine for fresh `host.status` and return the updated record.
#[tauri::command]
pub async fn request_host_status(
    host_pubkey: String,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<AgentHostRecord, String> {
    let channel = relay_channel(&state, &relay)?;
    let relay = community_relay(&relay);
    ops::refresh_host_status(&app, &state, &hosts, &channel, &relay, &host_pubkey).await
}

/// Tell the machine to forget this owner, then remove it locally.
#[tauri::command]
pub async fn forget_host(
    host_pubkey: String,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<ops::ForgetHostOutcome, String> {
    let channel = relay_channel(&state, &relay)?;
    let relay = community_relay(&relay);
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
    relay: crate::window_relay::WindowRelay,
    state: State<'_, AppState>,
    hosts: State<'_, HostOps>,
) -> Result<Option<AgentHostRecord>, String> {
    use nostr::JsonUtil;
    let event =
        nostr::Event::from_json(event_json).map_err(|error| format!("invalid event: {error}"))?;
    let relay = community_relay(&relay);
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

pub(crate) fn summary_for(
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
    use super::{host_install_base, host_install_command};

    const URI: &str = "nostrpair://abc?relay=wss%3A%2F%2Fpair.example&secret=s&v=1";

    #[test]
    fn install_base_is_the_relay_origin_over_http() {
        assert_eq!(
            host_install_base("wss://relay.example"),
            "https://relay.example/host"
        );
        assert_eq!(
            host_install_base("wss://relay.example/"),
            "https://relay.example/host"
        );
        assert_eq!(
            host_install_base("ws://localhost:3000"),
            "http://localhost:3000/host"
        );
        // Only the origin: a path on the relay URL never leaks into the base.
        assert_eq!(
            host_install_base("wss://relay.example/ws?x=1"),
            "https://relay.example/host"
        );
    }

    #[test]
    fn install_command_is_one_bash_pipeline_from_the_relay() {
        let base = host_install_base("wss://buzz.example");
        let command = host_install_command(&base, Some(URI));
        assert_eq!(
            command,
            format!(
                "curl -fsSL 'https://buzz.example/host/install.sh' | bash -s -- \
                 --base 'https://buzz.example/host' --uri '{URI}'"
            )
        );
        assert!(!command.contains("--relay"), "{command}");
        assert!(!command.contains("| sh "), "{command}");
        assert!(!command.contains('\n'), "one line: {command}");
    }

    #[test]
    fn repair_command_is_the_same_line_without_a_uri() {
        let base = host_install_base("wss://buzz.example");
        let expected = "curl -fsSL 'https://buzz.example/host/install.sh' | bash -s -- \
                        --base 'https://buzz.example/host'";
        assert_eq!(host_install_command(&base, None), expected);
        assert_eq!(host_install_command(&base, Some("")), expected);
    }

    #[test]
    fn commands_quote_hostile_values() {
        let hostile = "x'; rm -rf ~";
        assert!(host_install_command("https://r/host", Some(hostile))
            .ends_with(r"--uri 'x'\''; rm -rf ~'"));
    }
}
