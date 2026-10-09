//! Wire formats for agent hosts (`buzz host`), per the hosts contract v1.
//!
//! Control frames (owner → host) and telemetry frames (host → owner) ride
//! kind 24200 observer frames with NIP-44 encrypted JSON content, tagged
//! exactly like `buzz-acp`'s frames: `p` = recipient, `agent` = host pubkey,
//! `frame` = `control` | `telemetry`. The relay routes them through
//! `is_agent_owner(H, O)`, which holds because the host carries an
//! owner-issued NIP-OA auth tag (the pairing grant).

use buzz_core_pkg::kind::KIND_AGENT_OBSERVER_FRAME;
use buzz_core_pkg::observer::{
    decrypt_observer_payload, encrypt_observer_payload, OBSERVER_AGENT_TAG, OBSERVER_FRAME_CONTROL,
    OBSERVER_FRAME_TAG, OBSERVER_FRAME_TELEMETRY,
};
use nostr::{Event, Keys, PublicKey};
use serde::{Deserialize, Serialize};

pub const HOST_DEPLOY: &str = "host.deploy";
pub const HOST_UNDEPLOY: &str = "host.undeploy";
pub const HOST_STATUS: &str = "host.status";
pub const HOST_FORGET: &str = "host.forget";
pub const HOST_ACK: &str = "host.ack";

pub const HOST_HELLO_TYPE: &str = "buzz-host-hello";
pub const HOST_GRANT_TYPE: &str = "buzz-host-grant";

/// Telemetry older or newer than this (seconds) is dropped, matching the
/// relay's and `buzz-acp`'s observer freshness window.
pub const FRESHNESS_SECS: u64 = 300;

/// Current unix time in seconds.
pub fn now_secs() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

/// A fresh correlator for one control request.
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Longest folder path accepted for an agent on a machine.
pub const MAX_HOST_WORKDIR_CHARS: usize = 300;

/// Shape-check the folder an agent runs in on its machine. Blank means the
/// machine's default (`None`). The path belongs to the machine, so it is
/// never checked against this computer's filesystem: `~` is the machine's
/// home and relative paths are relative to it (`buzz host` expands both and
/// creates the folder if missing).
pub fn normalize_host_workdir(workdir: Option<&str>) -> Result<Option<String>, String> {
    let Some(workdir) = workdir.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if workdir.chars().count() > MAX_HOST_WORKDIR_CHARS {
        return Err(format!(
            "The folder path is too long (at most {MAX_HOST_WORKDIR_CHARS} characters)."
        ));
    }
    if workdir.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
        return Err("The folder path cannot contain line breaks or NUL characters.".into());
    }
    Ok(Some(workdir.to_string()))
}

/// `host.deploy`, built from the desktop's standard deploy payload
/// (`build_deploy_payload`). The result carries the agent nsec in plaintext:
/// callers encrypt it immediately and never log or persist it.
///
/// `workdir` is the agent's saved folder on the machine; blank or absent
/// sends `null`, and the machine uses `~/buzz-agents/<agent_pubkey>`.
pub fn deploy_frame(
    request_id: &str,
    agent_pubkey: &str,
    auth_tag: &str,
    workdir: Option<&str>,
    deploy_payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let workdir = workdir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let nsec = deploy_payload
        .get("private_key_nsec")
        .and_then(serde_json::Value::as_str)
        .filter(|nsec| !nsec.is_empty())
        .ok_or("agent has no private key available; not deployed")?;
    let relay_url = deploy_payload
        .get("relay_url")
        .and_then(serde_json::Value::as_str)
        .ok_or("deploy payload carries no relay; not deployed")?;
    let launch = deploy_payload
        .get("launch")
        .cloned()
        .ok_or("deploy payload carries no launch block; not deployed")?;
    let env = deploy_payload
        .get("env_vars")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    // The projected access policy, as every other backend receives it. The
    // host maps it to `BUZZ_ACP_RESPOND_TO*`; absent, buzz-acp defaults to
    // owner-only.
    let respond_to = deploy_payload
        .get("respond_to")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let respond_to_allowlist = deploy_payload
        .get("respond_to_allowlist")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    Ok(serde_json::json!({
        "type": HOST_DEPLOY,
        "request_id": request_id,
        "agent_pubkey": agent_pubkey,
        "agent_nsec": nsec,
        "auth_tag": auth_tag,
        "relay_url": relay_url,
        "workdir": workdir,
        "env": env,
        "launch": launch,
        "respond_to": respond_to,
        "respond_to_allowlist": respond_to_allowlist,
    }))
}

pub fn undeploy_frame(request_id: &str, agent_pubkey: &str) -> serde_json::Value {
    serde_json::json!({ "type": HOST_UNDEPLOY, "request_id": request_id, "agent_pubkey": agent_pubkey })
}

pub fn status_frame(request_id: &str) -> serde_json::Value {
    serde_json::json!({ "type": HOST_STATUS, "request_id": request_id })
}

pub fn forget_frame(request_id: &str) -> serde_json::Value {
    serde_json::json!({ "type": HOST_FORGET, "request_id": request_id })
}

/// Encrypt `payload` to `host` and sign the kind 24200 control frame as the
/// owner. Tags: `p` = host, `agent` = host, `frame` = `control`.
pub fn build_control_event(
    owner_keys: &Keys,
    host: &PublicKey,
    payload: &serde_json::Value,
) -> Result<Event, String> {
    let encrypted = encrypt_observer_payload(owner_keys, host, payload)
        .map_err(|error| format!("encrypt host control frame failed: {error}"))?;
    let host_hex = host.to_hex();
    buzz_sdk_pkg::build_agent_observer_frame(
        &host_hex,
        &host_hex,
        OBSERVER_FRAME_CONTROL,
        &encrypted,
    )
    .map_err(|error| format!("build host control frame failed: {error}"))?
    .sign_with_keys(owner_keys)
    .map_err(|error| format!("sign host control frame failed: {error}"))
}

/// One agent's state as reported by `host.status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostAgentState {
    pub agent_pubkey: String,
    pub state: String,
    #[serde(default)]
    pub since: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostClaudeState {
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub auth_ok: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostTools {
    #[serde(default)]
    pub node: bool,
    #[serde(default)]
    pub claude_agent_acp: bool,
    #[serde(default)]
    pub buzz_acp: bool,
}

/// `host.status` telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostStatus {
    #[serde(default)]
    pub request_id: Option<String>,
    pub name: String,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub agents: Vec<HostAgentState>,
    #[serde(default)]
    pub claude: HostClaudeState,
    #[serde(default)]
    pub tools: HostTools,
}

/// `host.ack` telemetry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HostAck {
    pub request_id: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostTelemetry {
    Ack(HostAck),
    Status(HostStatus),
}

impl HostTelemetry {
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Ack(ack) => Some(ack.request_id.as_str()),
            Self::Status(status) => status.request_id.as_deref(),
        }
    }
}

/// Verify and decrypt a telemetry frame from `expected_host` to the owner.
///
/// Rejects (with a reason) anything that is not exactly a kind 24200 frame
/// signed by the host, tagged `p` = owner, `agent` = host and
/// `frame` = `telemetry`, within the freshness window. Unknown `type`s yield
/// `Ok(None)` so newer hosts can add telemetry without breaking this client.
pub fn parse_host_telemetry(
    owner_keys: &Keys,
    expected_host: &PublicKey,
    event: &Event,
    now_secs: u64,
) -> Result<Option<HostTelemetry>, String> {
    if event.kind.as_u16() as u32 != KIND_AGENT_OBSERVER_FRAME {
        return Err("not an observer frame".into());
    }
    if event.pubkey != *expected_host {
        return Err("frame is not from this host".into());
    }
    event
        .verify()
        .map_err(|error| format!("host frame failed verification: {error}"))?;
    let owner_hex = owner_keys.public_key().to_hex();
    let host_hex = expected_host.to_hex();
    if single_tag(event, "p")? != owner_hex {
        return Err("host frame is not addressed to this owner".into());
    }
    if single_tag(event, OBSERVER_AGENT_TAG)? != host_hex {
        return Err("host frame agent tag does not name the host".into());
    }
    if single_tag(event, OBSERVER_FRAME_TAG)? != OBSERVER_FRAME_TELEMETRY {
        return Err("host frame is not telemetry".into());
    }
    if event.created_at.as_secs().abs_diff(now_secs) > FRESHNESS_SECS {
        return Err("host frame is outside the freshness window".into());
    }
    let payload: serde_json::Value = decrypt_observer_payload(owner_keys, event)
        .map_err(|error| format!("decrypt host frame failed: {error}"))?;
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some(HOST_ACK) => serde_json::from_value(payload)
            .map(|ack| Some(HostTelemetry::Ack(ack)))
            .map_err(|error| format!("malformed host.ack: {error}")),
        Some(HOST_STATUS) => serde_json::from_value(payload)
            .map(|status| Some(HostTelemetry::Status(status)))
            .map_err(|error| format!("malformed host.status: {error}")),
        _ => Ok(None),
    }
}

fn single_tag<'a>(event: &'a Event, name: &str) -> Result<&'a str, String> {
    let mut values = event
        .tags
        .iter()
        .filter(|tag| tag.kind().to_string() == name)
        .filter_map(|tag| tag.content());
    let value = values
        .next()
        .ok_or_else(|| format!("host frame has no {name} tag"))?;
    if values.next().is_some() {
        return Err(format!("host frame has more than one {name} tag"));
    }
    Ok(value)
}

/// `buzz-host-hello`, the host's NIP-AB return payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostHello {
    #[serde(rename = "type")]
    pub kind: String,
    pub v: u32,
    pub host_pubkey: String,
    pub name: String,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub version: Option<String>,
}

const MAX_HELLO_FIELD_CHARS: usize = 128;

/// Host-supplied text is untrusted: trim, strip control characters, cap.
fn clean_label(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_HELLO_FIELD_CHARS)
        .collect()
}

/// Bound every host-supplied display field of a `host.status`.
pub fn sanitize_status(status: &HostStatus) -> HostStatus {
    const MAX_AGENTS: usize = 256;
    HostStatus {
        request_id: status.request_id.clone(),
        name: clean_label(&status.name),
        os: clean_label(&status.os),
        arch: clean_label(&status.arch),
        version: status.version.as_deref().map(clean_label),
        agents: status
            .agents
            .iter()
            .take(MAX_AGENTS)
            .map(|agent| HostAgentState {
                agent_pubkey: clean_label(&agent.agent_pubkey),
                state: clean_label(&agent.state),
                since: agent.since.clone(),
            })
            .collect(),
        claude: status.claude.clone(),
        tools: status.tools.clone(),
    }
}

/// Parse and validate a `buzz-host-hello` payload.
pub fn parse_host_hello(payload: &str) -> Result<HostHello, String> {
    let hello: HostHello = serde_json::from_str(payload)
        .map_err(|_| "The machine sent an unrecognized pairing message.".to_string())?;
    if hello.kind != HOST_HELLO_TYPE || hello.v != 1 {
        return Err("The machine sent an unsupported pairing message version.".into());
    }
    let host = PublicKey::from_hex(hello.host_pubkey.trim())
        .map_err(|_| "The machine sent an invalid host key.".to_string())?;
    let clean = |value: &str| -> Result<String, String> {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > MAX_HELLO_FIELD_CHARS {
            return Err("The machine sent an invalid name, OS, or architecture.".into());
        }
        Ok(value.chars().filter(|c| !c.is_control()).collect())
    };
    Ok(HostHello {
        host_pubkey: host.to_hex(),
        name: clean(&hello.name)?,
        os: clean(&hello.os)?,
        arch: clean(&hello.arch)?,
        version: hello.version.as_deref().map(clean).transpose()?,
        ..hello
    })
}

/// `buzz-host-grant`: the owner's NIP-OA authorization for the host key,
/// with empty conditions (same as agents).
pub fn build_host_grant(
    owner_keys: &Keys,
    host_pubkey: &PublicKey,
    relay_url: &str,
) -> Result<serde_json::Value, String> {
    let auth_tag = buzz_sdk_pkg::nip_oa::compute_auth_tag(owner_keys, host_pubkey, "")
        .map_err(|error| format!("failed to compute NIP-OA auth tag for host: {error}"))?;
    Ok(serde_json::json!({
        "type": HOST_GRANT_TYPE,
        "v": 1,
        "owner_pubkey": owner_keys.public_key().to_hex(),
        "auth_tag": auth_tag,
        "relay_url": relay_url,
    }))
}

#[cfg(test)]
#[path = "frames_tests.rs"]
mod tests;
