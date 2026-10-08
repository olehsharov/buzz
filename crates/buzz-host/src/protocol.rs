//! Wire formats of the agent-host contract (v1).
//!
//! * Pairing payloads ride NIP-AB `PayloadType::Custom`:
//!   [`HostHello`] (host → desktop, the target's return payload) and
//!   [`HostGrant`] (desktop → host, the source's reply payload).
//! * Control frames (owner → host) and telemetry frames (host → owner) are
//!   JSON inside NIP-44 encrypted kind:24200 observer frames.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::{HostError, Result};

/// `type` of the host's pairing hello.
pub const HELLO_TYPE: &str = "buzz-host-hello";
/// `type` of the desktop's pairing grant.
pub const GRANT_TYPE: &str = "buzz-host-grant";
/// Contract version carried in pairing payloads.
pub const CONTRACT_VERSION: u32 = 1;

/// Control `type`: deploy (or redeploy) an agent.
pub const CONTROL_DEPLOY: &str = "host.deploy";
/// Control `type`: stop and remove an agent.
pub const CONTROL_UNDEPLOY: &str = "host.undeploy";
/// Control `type`: request a `host.status` telemetry reply.
pub const CONTROL_STATUS: &str = "host.status";
/// Control `type`: stop everything and leave the paired state.
pub const CONTROL_FORGET: &str = "host.forget";
/// Telemetry `type`: result of a deploy/undeploy/forget.
pub const TELEMETRY_ACK: &str = "host.ack";
/// Telemetry `type`: host details and agent states.
pub const TELEMETRY_STATUS: &str = "host.status";

/// True for a 64-char lowercase hex string (a Nostr pubkey).
///
/// Agent pubkeys become file and systemd unit names, so this strict check is
/// also the path-traversal guard.
pub fn is_pubkey_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Host → desktop pairing payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostHello {
    /// Always [`HELLO_TYPE`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Always [`CONTRACT_VERSION`].
    pub v: u32,
    /// Host pubkey H (hex).
    pub host_pubkey: String,
    /// Human-readable machine name.
    pub name: String,
    /// `linux` or `macos`.
    pub os: String,
    /// CPU architecture (e.g. `x86_64`, `aarch64`).
    pub arch: String,
    /// `buzz host` version.
    pub version: String,
}

impl HostHello {
    /// Build a hello for this machine.
    pub fn new(host_pubkey: String, name: String) -> Self {
        Self {
            kind: HELLO_TYPE.into(),
            v: CONTRACT_VERSION,
            host_pubkey,
            name,
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// Desktop → host pairing payload.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostGrant {
    /// Always [`GRANT_TYPE`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Always [`CONTRACT_VERSION`].
    pub v: u32,
    /// Owner pubkey O (hex).
    pub owner_pubkey: String,
    /// NIP-OA auth tag JSON authorizing H under O.
    pub auth_tag: String,
    /// Relay URL the host should stay connected to.
    pub relay_url: String,
}

impl fmt::Debug for HostGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostGrant")
            .field("owner_pubkey", &self.owner_pubkey)
            .field("auth_tag", &"<redacted>")
            .field("relay_url", &self.relay_url)
            .finish()
    }
}

impl Drop for HostGrant {
    fn drop(&mut self) {
        self.auth_tag.zeroize();
    }
}

impl HostGrant {
    /// Parse a grant and verify it authorizes `host_pubkey`.
    ///
    /// Checks the type and version, that the auth tag is a valid NIP-OA
    /// credential for H, and that its signer is `owner_pubkey`.
    pub fn decode_verified(json: &str, host_pubkey: &nostr::PublicKey) -> Result<Self> {
        let grant: HostGrant = serde_json::from_str(json)
            .map_err(|e| HostError::Pairing(format!("grant is not valid JSON: {e}")))?;
        if grant.kind != GRANT_TYPE || grant.v != CONTRACT_VERSION {
            return Err(HostError::Pairing(format!(
                "unsupported grant {}/v{}",
                grant.kind, grant.v
            )));
        }
        if !is_pubkey_hex(&grant.owner_pubkey) {
            return Err(HostError::Pairing("grant owner_pubkey is not hex".into()));
        }
        let signer =
            buzz_sdk::nip_oa::verify_auth_tag(&grant.auth_tag, host_pubkey).map_err(|_| {
                HostError::Pairing("grant auth tag does not authorize this host".into())
            })?;
        if signer.to_hex() != grant.owner_pubkey {
            return Err(HostError::Pairing(
                "grant auth tag is signed by someone other than owner_pubkey".into(),
            ));
        }
        validate_relay_url(&grant.relay_url)?;
        Ok(grant)
    }
}

/// Require a `ws://` or `wss://` URL.
pub fn validate_relay_url(url: &str) -> Result<()> {
    let ok = (url.starts_with("wss://") || url.starts_with("ws://")) && url.len() > 6;
    if ok {
        Ok(())
    } else {
        Err(HostError::Invalid(format!(
            "relay URL must start with ws:// or wss:// (got {url:?})"
        )))
    }
}

/// The `launch` block of a deploy, as produced by the desktop's
/// `build_deploy_payload`.
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Launch {
    /// Agent command (resolved by basename on the host).
    pub command: Option<String>,
    /// Agent command arguments.
    pub args: Vec<String>,
    /// Ordinary user environment (tier 2).
    pub env: BTreeMap<String, String>,
    /// Desktop-owned policy environment (tier 1).
    pub policy_env: BTreeMap<String, String>,
    /// Agent owner pubkey (hex).
    pub owner_pubkey: Option<String>,
}

/// `host.deploy` control payload.
#[derive(Clone, Deserialize)]
pub struct DeployRequest {
    /// Request id echoed in the ack.
    pub request_id: String,
    /// Agent pubkey (hex); the idempotency key.
    pub agent_pubkey: String,
    /// Agent secret key (nsec or hex). Never logged.
    pub agent_nsec: String,
    /// Agent NIP-OA auth tag JSON. Never logged.
    #[serde(default)]
    pub auth_tag: Option<String>,
    /// Relay the agent connects to.
    pub relay_url: String,
    /// Working directory on the host (`~` expanded; default `~`).
    #[serde(default)]
    pub workdir: Option<String>,
    /// Legacy flat environment, used only when `launch` is absent.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Resolved launch block.
    #[serde(default)]
    pub launch: Option<Launch>,
    /// Optional `respond_to` policy (extension; mirrors the provider payload).
    #[serde(default)]
    pub respond_to: Option<String>,
    /// Optional allowlist for `respond_to = "allowlist"` (extension).
    #[serde(default)]
    pub respond_to_allowlist: Vec<String>,
}

impl fmt::Debug for DeployRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeployRequest")
            .field("request_id", &self.request_id)
            .field("agent_pubkey", &self.agent_pubkey)
            .field("relay_url", &self.relay_url)
            .field("workdir", &self.workdir)
            .finish_non_exhaustive()
    }
}

impl Drop for DeployRequest {
    fn drop(&mut self) {
        self.agent_nsec.zeroize();
        if let Some(tag) = self.auth_tag.as_mut() {
            tag.zeroize();
        }
        for value in self.env.values_mut() {
            value.zeroize();
        }
    }
}

/// `host.undeploy` control payload.
#[derive(Debug, Clone, Deserialize)]
pub struct UndeployRequest {
    /// Request id echoed in the ack.
    pub request_id: String,
    /// Agent pubkey (hex).
    pub agent_pubkey: String,
}

/// A decoded control frame.
#[derive(Debug)]
pub enum Control {
    /// `host.deploy`.
    Deploy(Box<DeployRequest>),
    /// `host.undeploy`.
    Undeploy(UndeployRequest),
    /// `host.status`.
    Status {
        /// Request id echoed in the status reply.
        request_id: String,
    },
    /// `host.forget`.
    Forget {
        /// Request id echoed in the ack.
        request_id: String,
    },
}

impl Control {
    /// The request id of this control frame.
    pub fn request_id(&self) -> &str {
        match self {
            Control::Deploy(d) => &d.request_id,
            Control::Undeploy(u) => &u.request_id,
            Control::Status { request_id } | Control::Forget { request_id } => request_id,
        }
    }
}

/// Maximum accepted `request_id` length.
pub const MAX_REQUEST_ID_LEN: usize = 128;

/// Decode decrypted control JSON.
///
/// Returns `Err((request_id, message))` when the frame names a request id but
/// cannot be handled, so the caller can still ack the failure.
pub fn decode_control(
    value: serde_json::Value,
) -> std::result::Result<Control, (Option<String>, String)> {
    let request_id = value
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let Some(id) = request_id.clone() else {
        return Err((None, "control frame has no request_id".into()));
    };
    if id.is_empty() || id.len() > MAX_REQUEST_ID_LEN {
        return Err((None, "control frame request_id is empty or too long".into()));
    }
    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let bad = |e: serde_json::Error| (request_id.clone(), format!("malformed {kind}: {e}"));
    match kind {
        CONTROL_DEPLOY => {
            let req: DeployRequest = serde_json::from_value(value.clone()).map_err(bad)?;
            Ok(Control::Deploy(Box::new(req)))
        }
        CONTROL_UNDEPLOY => {
            let req: UndeployRequest = serde_json::from_value(value.clone()).map_err(bad)?;
            Ok(Control::Undeploy(req))
        }
        CONTROL_STATUS => Ok(Control::Status { request_id: id }),
        CONTROL_FORGET => Ok(Control::Forget { request_id: id }),
        other => Err((request_id, format!("unknown control type {other:?}"))),
    }
}

/// `host.ack` telemetry payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ack {
    /// Always [`TELEMETRY_ACK`].
    #[serde(rename = "type")]
    pub kind: String,
    /// The request being acknowledged.
    pub request_id: String,
    /// Whether the request succeeded.
    pub ok: bool,
    /// Failure reason (no secrets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Ack {
    /// Build an ack from a request id and its outcome.
    pub fn new(request_id: &str, outcome: std::result::Result<(), String>) -> Self {
        let (ok, error) = match outcome {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        Self {
            kind: TELEMETRY_ACK.into(),
            request_id: request_id.into(),
            ok,
            error,
        }
    }
}

/// Lifecycle state of one agent on this host.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentRunState {
    /// The agent process is running.
    Running,
    /// The agent is not running (stopped by `!shutdown` or never started).
    Stopped,
    /// The agent crashed and is not (yet) running.
    Failed,
}

/// One agent entry in `host.status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentStatus {
    /// Agent pubkey (hex).
    pub agent_pubkey: String,
    /// Current state.
    pub state: AgentRunState,
    /// Unix seconds since the agent entered `state`.
    pub since: u64,
}

/// Claude Code availability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeStatus {
    /// `claude` is on PATH.
    pub installed: bool,
    /// Whether Claude is logged in; `null` when unknown.
    pub auth_ok: Option<bool>,
}

/// Tool availability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolsStatus {
    /// `node` is on PATH.
    pub node: bool,
    /// `claude-agent-acp` is on PATH.
    pub claude_agent_acp: bool,
    /// `buzz-acp` is on PATH.
    pub buzz_acp: bool,
}

/// `host.status` telemetry payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostStatus {
    /// Always [`TELEMETRY_STATUS`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Present when replying to a `host.status` control frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Machine name.
    pub name: String,
    /// `linux` or `macos`.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// `buzz host` version.
    pub version: String,
    /// Deployed agents.
    pub agents: Vec<AgentStatus>,
    /// Claude Code availability.
    pub claude: ClaudeStatus,
    /// Tool availability.
    pub tools: ToolsStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    /// The hello serializes to exactly the contract's field set and values.
    ///
    /// Mutation: rename any field or the `type` tag → RED.
    #[test]
    fn hello_matches_contract_json() {
        let hello = HostHello {
            kind: HELLO_TYPE.into(),
            v: 1,
            host_pubkey: "ab".repeat(32),
            name: "box".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            version: "0.1.0".into(),
        };
        let got = serde_json::to_value(&hello).expect("serialize");
        let want = serde_json::json!({
            "type": "buzz-host-hello", "v": 1, "host_pubkey": "ab".repeat(32),
            "name": "box", "os": "linux", "arch": "x86_64", "version": "0.1.0"
        });
        assert_eq!(got, want);
        let back: HostHello = serde_json::from_value(want).expect("decode");
        assert_eq!(back, hello);
    }

    fn grant_json(owner: &Keys, host: &nostr::PublicKey, signer: &Keys) -> String {
        let tag = buzz_sdk::nip_oa::compute_auth_tag(signer, host, "").expect("tag");
        serde_json::json!({
            "type": "buzz-host-grant", "v": 1,
            "owner_pubkey": owner.public_key().to_hex(),
            "auth_tag": tag, "relay_url": "wss://relay.example"
        })
        .to_string()
    }

    /// The contract's grant JSON decodes; a tag for another host, a tag
    /// signed by a non-owner, or a wrong type is refused.
    ///
    /// Mutations: skip `verify_auth_tag` → the wrong-host case decodes → RED;
    /// skip the signer == owner check → the impostor case decodes → RED.
    #[test]
    fn grant_decodes_and_verifies_against_contract_json() {
        let owner = Keys::generate();
        let host = Keys::generate();
        let json = grant_json(&owner, &host.public_key(), &owner);
        let grant = HostGrant::decode_verified(&json, &host.public_key()).expect("valid grant");
        assert_eq!(grant.owner_pubkey, owner.public_key().to_hex());
        assert_eq!(grant.relay_url, "wss://relay.example");

        let other_host = Keys::generate();
        assert!(HostGrant::decode_verified(&json, &other_host.public_key()).is_err());

        let impostor = Keys::generate();
        let forged = grant_json(&owner, &host.public_key(), &impostor);
        assert!(HostGrant::decode_verified(&forged, &host.public_key()).is_err());

        let wrong_type = json.replace("buzz-host-grant", "buzz-host-hello");
        assert!(HostGrant::decode_verified(&wrong_type, &host.public_key()).is_err());
    }

    #[test]
    fn grant_debug_redacts_auth_tag() {
        let owner = Keys::generate();
        let host = Keys::generate();
        let json = grant_json(&owner, &host.public_key(), &owner);
        let grant = HostGrant::decode_verified(&json, &host.public_key()).expect("grant");
        let dbg = format!("{grant:?}");
        assert!(!dbg.contains(&grant.auth_tag), "{dbg}");
    }

    #[test]
    fn decode_control_table() {
        use serde_json::json;
        let pk = "ab".repeat(32);
        let deploy = json!({"type":"host.deploy","request_id":"r1","agent_pubkey":pk,
            "agent_nsec":"nsec1x","auth_tag":"[]","relay_url":"wss://r","workdir":"~",
            "env":{},"launch":{"command":"claude-agent-acp","args":[],"env":{},"policy_env":{},"owner_pubkey":pk}});
        assert!(matches!(decode_control(deploy), Ok(Control::Deploy(_))));
        let undeploy = json!({"type":"host.undeploy","request_id":"r2","agent_pubkey":pk});
        assert!(matches!(decode_control(undeploy), Ok(Control::Undeploy(_))));
        assert!(matches!(
            decode_control(json!({"type":"host.status","request_id":"r3"})),
            Ok(Control::Status { .. })
        ));
        assert!(matches!(
            decode_control(json!({"type":"host.forget","request_id":"r4"})),
            Ok(Control::Forget { .. })
        ));
        assert!(matches!(
            decode_control(json!({"type":"host.reboot","request_id":"r5"})),
            Err((Some(_), _))
        ));
        assert!(matches!(
            decode_control(json!({"type":"host.status"})),
            Err((None, _))
        ));
        assert!(matches!(
            decode_control(json!({"type":"host.deploy","request_id":"r6"})),
            Err((Some(_), _))
        ));
    }

    #[test]
    fn status_serializes_contract_shape() {
        let status = HostStatus {
            kind: TELEMETRY_STATUS.into(),
            request_id: None,
            name: "box".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            version: "0.1.0".into(),
            agents: vec![AgentStatus {
                agent_pubkey: "ab".repeat(32),
                state: AgentRunState::Running,
                since: 5,
            }],
            claude: ClaudeStatus {
                installed: true,
                auth_ok: None,
            },
            tools: ToolsStatus {
                node: true,
                claude_agent_acp: false,
                buzz_acp: true,
            },
        };
        let v = serde_json::to_value(&status).expect("serialize");
        assert_eq!(v["type"], "host.status");
        assert!(v.get("request_id").is_none());
        assert_eq!(v["agents"][0]["state"], "running");
        assert_eq!(v["claude"]["auth_ok"], serde_json::Value::Null);
        assert_eq!(v["tools"]["claude_agent_acp"], false);
        let ack = serde_json::to_value(Ack::new("r", Err("boom".into()))).expect("ack");
        assert_eq!(
            ack,
            serde_json::json!({"type":"host.ack","request_id":"r","ok":false,"error":"boom"})
        );
    }
}
