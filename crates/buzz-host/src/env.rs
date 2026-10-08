//! Turn a `host.deploy` request into the agent's `buzz-acp` environment.
//!
//! Same three-tier precedence as `scripts/buzz-backend-ssh` and
//! `buzz-backend-kubernetes`: `launch.policy_env` (tier 1), then
//! `launch.env` — or the flat `env` when no launch block is sent — (tier 2),
//! then the authoritative keys the host sets itself (tier 3).

use std::collections::BTreeMap;

use nostr::{Keys, SecretKey};

use crate::error::{HostError, Result};
use crate::protocol::{is_pubkey_hex, validate_relay_url, DeployRequest};
use crate::store::{now_secs, AgentRecord};

/// Keys the host always sets itself; any caller-supplied value is dropped.
pub const AUTHORITATIVE_KEYS: &[&str] = &[
    "BUZZ_RELAY_URL",
    "BUZZ_PRIVATE_KEY",
    "NOSTR_PRIVATE_KEY",
    "BUZZ_AUTH_TAG",
    "BUZZ_ACP_AGENT_OWNER",
    "BUZZ_ACP_AGENT_COMMAND",
    "BUZZ_ACP_AGENT_ARGS",
    "BUZZ_ACP_RESPOND_TO",
    "BUZZ_ACP_RESPOND_TO_ALLOWLIST",
    "BUZZ_ACP_MCP_COMMAND",
    "BUZZ_ACP_EXIT_AFTER_INACTIVITY",
];

/// Refused on remote agents: presence is the only liveness signal.
pub const FORBIDDEN_KEY: &str = "BUZZ_ACP_NO_PRESENCE";

fn is_posix_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn nonempty(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|s| !s.is_empty())
}

/// Expand a leading `~` against `home`. Relative paths are resolved
/// against `home` too, so the result is always absolute.
pub fn expand_workdir(workdir: Option<&str>, home: &std::path::Path) -> String {
    let raw = nonempty(workdir).unwrap_or("~");
    let path = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if raw.starts_with('/') {
        std::path::PathBuf::from(raw)
    } else {
        home.join(raw)
    };
    path.to_string_lossy().into_owned()
}

/// Per-agent working directory used when a deploy sends `workdir: null`:
/// `~/buzz-agents/<agent_pubkey>`.
pub fn default_workdir(home: &std::path::Path, agent_pubkey: &str) -> String {
    home.join("buzz-agents")
        .join(agent_pubkey)
        .to_string_lossy()
        .into_owned()
}

/// Validate `req` and build the agent's record.
///
/// `owner_pubkey` is the paired owner O, the fallback agent owner when the
/// launch block names none. `path` is the host search path (see
/// [`crate::tools::agent_path`]). Error messages never contain secrets.
pub fn build_agent_record(
    req: &DeployRequest,
    owner_pubkey: &str,
    home: &std::path::Path,
    path: &str,
) -> Result<AgentRecord> {
    if !is_pubkey_hex(&req.agent_pubkey) {
        return Err(HostError::Invalid(
            "agent_pubkey must be 64 lowercase hex characters".into(),
        ));
    }
    let secret = SecretKey::parse(req.agent_nsec.trim())
        .map_err(|_| HostError::Invalid("agent_nsec is not a valid secret key".into()))?;
    let keys = Keys::new(secret);
    if keys.public_key().to_hex() != req.agent_pubkey {
        return Err(HostError::Invalid(
            "agent_nsec does not belong to agent_pubkey".into(),
        ));
    }
    validate_relay_url(req.relay_url.trim())?;

    let launch = req.launch.as_ref();
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    if let Some(launch) = launch {
        env.extend(launch.policy_env.clone());
        env.extend(launch.env.clone());
    } else {
        env.extend(req.env.clone());
    }
    for key in env.keys() {
        if !is_posix_key(key) {
            return Err(HostError::Invalid(format!(
                "env key {key:?} is not a POSIX environment variable name"
            )));
        }
        if key.eq_ignore_ascii_case(FORBIDDEN_KEY) {
            return Err(HostError::Invalid(format!(
                "{FORBIDDEN_KEY} must not be set on a remote agent: presence is the only signal that a remote agent is alive"
            )));
        }
    }
    for key in AUTHORITATIVE_KEYS {
        env.remove(*key);
    }

    let nsec = req.agent_nsec.trim().to_string();
    env.insert("BUZZ_RELAY_URL".into(), req.relay_url.trim().to_string());
    env.insert("BUZZ_PRIVATE_KEY".into(), nsec.clone());
    env.insert("NOSTR_PRIVATE_KEY".into(), nsec);

    let tag = nonempty(req.auth_tag.as_deref());
    if let Some(tag) = tag {
        buzz_sdk::nip_oa::verify_auth_tag(tag, &keys.public_key())
            .map_err(|_| HostError::Invalid("auth_tag does not authorize this agent".into()))?;
        env.insert("BUZZ_AUTH_TAG".into(), tag.to_string());
    }
    let owner = nonempty(launch.and_then(|l| l.owner_pubkey.as_deref())).unwrap_or(owner_pubkey);
    if !is_pubkey_hex(owner) {
        return Err(HostError::Invalid("launch.owner_pubkey is not hex".into()));
    }
    env.insert("BUZZ_ACP_AGENT_OWNER".into(), owner.to_string());

    if let Some(command) = nonempty(launch.and_then(|l| l.command.as_deref())) {
        // A desktop path is meaningless here; resolve by name.
        let name = command.rsplit('/').next().unwrap_or(command);
        if crate::tools::which(name, path).is_none() {
            return Err(HostError::Invalid(format!(
                "agent command {name:?} is not installed on this host"
            )));
        }
        env.insert("BUZZ_ACP_AGENT_COMMAND".into(), name.to_string());
    }
    if let Some(args) = launch.map(|l| &l.args).filter(|a| !a.is_empty()) {
        env.insert("BUZZ_ACP_AGENT_ARGS".into(), args.join(","));
    }

    if let Some(respond_to) = nonempty(req.respond_to.as_deref()) {
        if respond_to == "allowlist" && req.respond_to_allowlist.is_empty() {
            return Err(HostError::Invalid(
                "respond_to=allowlist requires a non-empty allowlist".into(),
            ));
        }
        env.insert("BUZZ_ACP_RESPOND_TO".into(), respond_to.to_string());
    }
    if !req.respond_to_allowlist.is_empty() {
        env.insert(
            "BUZZ_ACP_RESPOND_TO_ALLOWLIST".into(),
            req.respond_to_allowlist.join(","),
        );
    }

    if crate::tools::which("buzz-acp", path).is_none() {
        return Err(HostError::Invalid(
            "buzz-acp is not installed on this host".into(),
        ));
    }
    let mcp = crate::tools::which("buzz-dev-mcp", path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    env.insert("BUZZ_ACP_MCP_COMMAND".into(), mcp);
    env.entry("PATH".into()).or_insert_with(|| path.to_string());
    env.entry("RUST_LOG".into())
        .or_insert_with(|| "info".into());

    Ok(AgentRecord {
        agent_pubkey: req.agent_pubkey.clone(),
        workdir: match nonempty(req.workdir.as_deref()) {
            Some(dir) => expand_workdir(Some(dir), home),
            None => default_workdir(home, &req.agent_pubkey),
        },
        env,
        deployed_at: now_secs(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::Launch;

    /// A deploy request for `agent` with a launch block, as a test fixture.
    pub(crate) fn deploy_request(agent: &Keys, owner: &Keys, request_id: &str) -> DeployRequest {
        use nostr::ToBech32;
        let tag = buzz_sdk::nip_oa::compute_auth_tag(owner, &agent.public_key(), "").expect("tag");
        DeployRequest {
            request_id: request_id.into(),
            agent_pubkey: agent.public_key().to_hex(),
            agent_nsec: agent.secret_key().to_bech32().expect("nsec"),
            auth_tag: Some(tag),
            relay_url: "wss://relay.example".into(),
            workdir: Some("~/work".into()),
            env: BTreeMap::new(),
            launch: Some(Launch {
                command: Some("/Applications/x/sh".into()),
                args: vec!["--acp".into()],
                env: BTreeMap::from([
                    ("USER_KEY".into(), "user".into()),
                    ("BUZZ_PRIVATE_KEY".into(), "spoofed".into()),
                ]),
                policy_env: BTreeMap::from([
                    ("USER_KEY".into(), "policy".into()),
                    ("BUZZ_ACP_LAZY_POOL".into(), "true".into()),
                ]),
                owner_pubkey: Some(owner.public_key().to_hex()),
            }),
            respond_to: None,
            respond_to_allowlist: Vec::new(),
        }
    }

    /// A search path containing fake `buzz-acp`, `buzz-dev-mcp` and `sh`.
    #[cfg(unix)]
    pub(crate) fn fake_tool_path(dir: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        for name in ["buzz-acp", "buzz-dev-mcp", "sh"] {
            let p = dir.join(name);
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").expect("write tool");
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        dir.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn env_precedence_and_authoritative_keys() {
        let tools = tempfile::tempdir().expect("tools");
        let path = fake_tool_path(tools.path());
        let (agent, owner) = (Keys::generate(), Keys::generate());
        let req = deploy_request(&agent, &owner, "r1");
        let home = std::path::Path::new("/home/u");
        let rec =
            build_agent_record(&req, &owner.public_key().to_hex(), home, &path).expect("record");
        assert_eq!(rec.env["USER_KEY"], "user", "launch.env beats policy_env");
        assert_eq!(rec.env["BUZZ_ACP_LAZY_POOL"], "true");
        assert_eq!(rec.env["BUZZ_PRIVATE_KEY"], req.agent_nsec, "spoof dropped");
        assert_eq!(rec.env["BUZZ_ACP_AGENT_COMMAND"], "sh", "basename only");
        assert_eq!(rec.env["BUZZ_ACP_AGENT_ARGS"], "--acp");
        assert_eq!(rec.env["BUZZ_ACP_AGENT_OWNER"], owner.public_key().to_hex());
        assert_eq!(rec.workdir, "/home/u/work");

        let mut null_workdir = deploy_request(&agent, &owner, "r2");
        null_workdir.workdir = None;
        let rec = build_agent_record(&null_workdir, &owner.public_key().to_hex(), home, &path)
            .expect("record");
        assert_eq!(
            rec.workdir,
            format!("/home/u/buzz-agents/{}", agent.public_key().to_hex()),
            "null workdir => per-agent default"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_invalid_deploys_without_echoing_secrets() {
        let tools = tempfile::tempdir().expect("tools");
        let path = fake_tool_path(tools.path());
        let (agent, owner) = (Keys::generate(), Keys::generate());
        let owner_hex = owner.public_key().to_hex();
        let home = std::path::Path::new("/home/u");

        let mut wrong_key = deploy_request(&agent, &owner, "r");
        wrong_key.agent_pubkey = Keys::generate().public_key().to_hex();
        let mut traversal = deploy_request(&agent, &owner, "r");
        traversal.agent_pubkey = "../../etc/passwd".into();
        let mut no_presence = deploy_request(&agent, &owner, "r");
        if let Some(l) = no_presence.launch.as_mut() {
            l.env.insert("BUZZ_ACP_NO_PRESENCE".into(), "1".into());
        }
        let mut bad_tag = deploy_request(&agent, &owner, "r");
        bad_tag.auth_tag = Some(
            buzz_sdk::nip_oa::compute_auth_tag(&owner, &Keys::generate().public_key(), "")
                .expect("tag"),
        );
        for req in [wrong_key, traversal, no_presence, bad_tag] {
            let err = build_agent_record(&req, &owner_hex, home, &path)
                .expect_err("must refuse")
                .to_string();
            assert!(!err.contains(&req.agent_nsec), "{err}");
        }
        let missing = build_agent_record(
            &deploy_request(&agent, &owner, "r"),
            &owner_hex,
            home,
            "/nonexistent",
        );
        assert!(
            missing.is_err(),
            "missing buzz-acp / agent command is refused"
        );
    }
}
