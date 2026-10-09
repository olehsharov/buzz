//! Owner-reviewed agent draft requests published through Buzz observer frames.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use buzz_core::observer::{encrypt_observer_payload, OBSERVER_FRAME_TELEMETRY};
use nostr::{Event, FromBech32, Keys, PublicKey};
use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

use crate::error::CliError;

const AGENT_REQUEST_KIND: &str = "agent_management_request";
const PROJECT_CHANNEL_REQUEST_KIND: &str = "project_channel_request";
const MAX_NAME_CHARS: usize = 120;
const MAX_PROMPT_CHARS: usize = 20_000;
const MAX_VALUE_CHARS: usize = 300;
const MAX_ID_CHARS: usize = 64;
const MAX_PROVIDER_CONFIG_ENTRIES: usize = 20;
const MAX_EMOJI_BYTES: usize = 16;
/// The only environment variables an agent may propose; Desktop rejects any other key.
const ALLOWED_ENV_KEYS: [&str; 3] = [
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "BUZZ_ACP_RESUME_SESSION",
];
const RESUME_SESSION_ENV_KEY: &str = "BUZZ_ACP_RESUME_SESSION";
const RESPOND_TO_VALUES: [&str; 4] = ["owner-only", "allowlist", "anyone", "nobody"];
/// `--run-on host:<ref>` targets one of the owner's paired machines.
const HOST_RUN_ON_PREFIX: &str = "host:";
/// Longest machine name Desktop accepts from a machine's hello frame.
const MAX_HOST_REFERENCE_CHARS: usize = 128;
/// Longest machine folder Desktop accepts (mirrors its `MAX_HOST_WORKDIR_CHARS`).
const MAX_WORKDIR_CHARS: usize = 300;
/// Provider-config key that carries a compute provider's working directory.
const PROVIDER_WORKDIR_KEY: &str = "workdir";
/// Claude Code shortens longer project directory names, so they cannot be predicted.
const MAX_CLAUDE_PROJECT_DIR_CHARS: usize = 200;
/// Upper bound on `~/.claude/projects` entries scanned for a resumed session.
const MAX_PROJECT_DIRS_SCANNED: usize = 10_000;

/// Unvalidated `draft-create` input as received from the command line.
///
/// `env_vars` and `provider_config` hold raw `KEY=VALUE` strings; [`build_create`]
/// parses and validates every field against the Desktop create-draft contract.
#[derive(Debug, Clone, Default)]
pub struct CreateAgentDraft {
    pub channel_id: String,
    pub display_name: String,
    pub system_prompt: String,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub respond_to: Option<String>,
    pub env_vars: Vec<String>,
    pub avatar_emoji: Option<String>,
    pub avatar_color: Option<String>,
    /// A compute provider id, or `host:<pubkey|npub|name>` for a paired machine.
    pub run_on: Option<String>,
    pub provider_config: Vec<String>,
    /// Folder the agent runs in on the chosen `run_on` target.
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CreateAgentAvatar {
    emoji: String,
    color: String,
}

/// Wire shape of the decrypted `request` object for action `create`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentRequest {
    channel_id: String,
    display_name: String,
    system_prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    respond_to: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env_vars: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar: Option<CreateAgentAvatar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_on: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    provider_config: BTreeMap<String, String>,
    /// A paired machine as a lowercase hex pubkey or its name. Desktop
    /// resolves it against the owner's approved machines. Exclusive with
    /// `run_on`, so a provider draft keeps the shape older Desktops accept.
    #[serde(skip_serializing_if = "Option::is_none")]
    run_on_host: Option<String>,
    /// Folder on that machine; only sent with `run_on_host`.
    #[serde(skip_serializing_if = "Option::is_none")]
    host_workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentDraft {
    pub channel_id: String,
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub respond_to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectChannelDraft {
    pub home_channel_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRequest<T> {
    #[serde(rename = "type")]
    request_type: &'static str,
    action: &'static str,
    request_id: String,
    request: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ObserverEvent<T> {
    seq: u64,
    timestamp: String,
    kind: &'static str,
    agent_index: Option<usize>,
    channel_id: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    payload: ManagementRequest<T>,
}

#[derive(Debug)]
pub struct BuiltDraftRequest {
    pub event: Event,
    pub request_id: String,
    pub action: &'static str,
    /// Non-fatal problems with the draft that the agent should relay.
    pub warnings: Vec<String>,
}

fn required(value: String, label: &str, max: usize) -> Result<String, CliError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CliError::Usage(format!("{label} is required")));
    }
    if value.chars().count() > max {
        return Err(CliError::Usage(format!(
            "{label} is too long (max {max} characters)"
        )));
    }
    Ok(value.to_owned())
}

fn optional(value: Option<String>, label: &str) -> Result<Option<String>, CliError> {
    value
        .map(|value| required(value, label, MAX_VALUE_CHARS))
        .transpose()
}

/// `[a-z0-9][a-z0-9_-]*`, at most 64 characters (runtime and provider ids).
fn optional_id(value: Option<String>, label: &str) -> Result<Option<String>, CliError> {
    let Some(value) = optional(value, label)? else {
        return Ok(None);
    };
    let mut chars = value.chars();
    let valid = value.len() <= MAX_ID_CHARS
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        return Err(CliError::Usage(format!(
            "invalid {label} '{value}': use lowercase letters, digits, '_' or '-', \
             starting with a letter or digit (max {MAX_ID_CHARS} characters)"
        )));
    }
    Ok(Some(value))
}

fn is_lowercase_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
        })
}

fn is_provider_config_key(key: &str) -> bool {
    let mut chars = key.chars();
    key.len() <= MAX_ID_CHARS
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Parses repeated `KEY=VALUE` flags, rejecting malformed entries and duplicate keys.
fn key_values(entries: Vec<String>, flag: &str) -> Result<BTreeMap<String, String>, CliError> {
    let mut map = BTreeMap::new();
    for entry in entries {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(CliError::Usage(format!("{flag} expects KEY=VALUE")));
        };
        if value.chars().count() > MAX_VALUE_CHARS {
            return Err(CliError::Usage(format!(
                "{flag} value for {key} is too long (max {MAX_VALUE_CHARS} characters)"
            )));
        }
        if map.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(CliError::Usage(format!(
                "{flag} {key} is given more than once"
            )));
        }
    }
    Ok(map)
}

fn env_vars(entries: Vec<String>) -> Result<BTreeMap<String, String>, CliError> {
    let map = key_values(entries, "--env")?;
    for (key, value) in &map {
        if !ALLOWED_ENV_KEYS.contains(&key.as_str()) {
            return Err(CliError::Usage(format!(
                "--env {key} is not allowed; allowed keys: {}",
                ALLOWED_ENV_KEYS.join(", ")
            )));
        }
        if key == RESUME_SESSION_ENV_KEY && !is_lowercase_uuid(value) {
            return Err(CliError::Usage(format!(
                "--env {RESUME_SESSION_ENV_KEY} must be a lowercase hyphenated session UUID"
            )));
        }
    }
    Ok(map)
}

/// Where a draft asks the agent to run.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunOn {
    /// A compute provider id discovered by the owner's Desktop.
    Provider(String),
    /// A paired machine as a lowercase hex pubkey or its name. Only Desktop
    /// knows the owner's approved machines, so it resolves the reference.
    Host(String),
}

/// Parses `--run-on`: a provider id, or `host:<hex pubkey|npub|name>`.
fn run_on_target(value: Option<String>) -> Result<Option<RunOn>, CliError> {
    let Some(value) = optional(value, "run-on")? else {
        return Ok(None);
    };
    match value.strip_prefix(HOST_RUN_ON_PREFIX) {
        Some(reference) => Ok(Some(RunOn::Host(host_reference(reference)?))),
        None => Ok(optional_id(Some(value), "run-on")?.map(RunOn::Provider)),
    }
}

/// Normalizes a machine reference: a hex pubkey is lowercased and an npub is
/// decoded to hex; anything else is passed through as a machine name.
fn host_reference(reference: &str) -> Result<String, CliError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(CliError::Usage(
            "--run-on host: needs a machine pubkey, npub or name, e.g. host:devbox".into(),
        ));
    }
    if reference.len() == 64 && reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(reference.to_ascii_lowercase());
    }
    if reference.starts_with("npub1") {
        return PublicKey::from_bech32(reference)
            .map(|pubkey| pubkey.to_hex())
            .map_err(|_| CliError::Usage(format!("invalid machine npub '{reference}'")));
    }
    if reference.chars().count() > MAX_HOST_REFERENCE_CHARS
        || reference.chars().any(char::is_control)
    {
        return Err(CliError::Usage(format!(
            "invalid machine name in --run-on: use at most {MAX_HOST_REFERENCE_CHARS} \
             characters and no control characters"
        )));
    }
    Ok(reference.to_owned())
}

/// Validates `--workdir` the way Desktop validates a machine folder.
fn workdir(value: Option<String>) -> Result<Option<String>, CliError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(CliError::Usage("--workdir needs a folder path".into()));
    }
    if value.chars().count() > MAX_WORKDIR_CHARS {
        return Err(CliError::Usage(format!(
            "--workdir is too long (max {MAX_WORKDIR_CHARS} characters)"
        )));
    }
    if value.contains(['\0', '\n', '\r']) {
        return Err(CliError::Usage(
            "--workdir must be one line without NUL characters".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

/// The wire fields for a run target: `(runOn, runOnHost, hostWorkdir)`.
/// A provider's `--workdir` travels as `providerConfig.workdir`, exactly as
/// `--provider-config workdir=…` always has.
type RunOnFields = (Option<String>, Option<String>, Option<String>);

fn run_on_fields(
    run_on: Option<RunOn>,
    workdir: Option<String>,
    provider_config: &mut BTreeMap<String, String>,
) -> Result<RunOnFields, CliError> {
    match run_on {
        None if workdir.is_some() => Err(CliError::Usage(
            "--workdir requires --run-on (a provider id or host:<machine>)".into(),
        )),
        None => Ok((None, None, None)),
        Some(RunOn::Host(reference)) => Ok((None, Some(reference), workdir)),
        Some(RunOn::Provider(id)) => {
            if let Some(workdir) = workdir {
                match provider_config.get(PROVIDER_WORKDIR_KEY) {
                    Some(existing) if *existing != workdir => {
                        return Err(CliError::Usage(format!(
                            "--workdir conflicts with --provider-config \
                             {PROVIDER_WORKDIR_KEY}={existing}; give the folder once"
                        )));
                    }
                    _ => {
                        provider_config.insert(PROVIDER_WORKDIR_KEY.to_owned(), workdir);
                    }
                }
                if provider_config.len() > MAX_PROVIDER_CONFIG_ENTRIES {
                    return Err(CliError::Usage(format!(
                        "--provider-config accepts at most {MAX_PROVIDER_CONFIG_ENTRIES} entries, \
                         including --workdir"
                    )));
                }
            }
            Ok((Some(id), None, None))
        }
    }
}

/// Claude Code stores a session under `~/.claude/projects/<cwd with every
/// non-alphanumeric character replaced by '-'>/<session id>.jsonl`.
fn claude_project_dir_name(folder: &str) -> String {
    folder
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn expand_home(folder: &str, home: &Path) -> Option<PathBuf> {
    let folder = if folder.len() > 1 {
        folder.trim_end_matches('/')
    } else {
        folder
    };
    if folder == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = folder.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    Path::new(folder)
        .is_absolute()
        .then(|| PathBuf::from(folder))
}

/// Warnings for a draft that resumes a Claude Code session on a remote target.
///
/// Claude Code resumes a session only from the folder it ran in. Without a
/// folder the agent starts in the target's default folder. With one, the
/// check can only run when the session file is on this computer (the CLI
/// cannot see another machine's disk): it warns when the session sits under
/// a different project folder than `workdir`, and stays silent when the
/// session is not here at all.
fn resume_session_warnings(
    session: Option<&str>,
    remote_target: bool,
    workdir: Option<&str>,
    home: Option<&Path>,
) -> Vec<String> {
    let Some(session) = session else {
        return Vec::new();
    };
    if !remote_target {
        return Vec::new();
    }
    let Some(workdir) = workdir else {
        return vec![format!(
            "{RESUME_SESSION_ENV_KEY} is set without --workdir: the agent starts in the \
             target's default folder, where Claude Code cannot find session {session}. \
             Pass --workdir with the folder the session ran in."
        )];
    };
    let Some(home) = home else {
        return Vec::new();
    };
    let Some(folder) = expand_home(workdir, home) else {
        return Vec::new();
    };
    let expected = claude_project_dir_name(&folder.to_string_lossy());
    if expected.len() > MAX_CLAUDE_PROJECT_DIR_CHARS {
        return Vec::new();
    }
    let projects = home.join(".claude").join("projects");
    let file_name = format!("{session}.jsonl");
    if projects.join(&expected).join(&file_name).is_file() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    let found = entries
        .take(MAX_PROJECT_DIRS_SCANNED)
        .filter_map(Result::ok)
        .find(|entry| entry.path().join(&file_name).is_file());
    match found {
        Some(entry) => vec![format!(
            "Session {session} is in ~/.claude/projects/{}/ on this computer, but --workdir \
             {workdir} maps to ~/.claude/projects/{expected}/. Claude Code resumes a session \
             only from the folder it ran in, so check --workdir.",
            entry.file_name().to_string_lossy()
        )],
        None => Vec::new(),
    }
}

fn provider_config(
    entries: Vec<String>,
    run_on: Option<&RunOn>,
) -> Result<BTreeMap<String, String>, CliError> {
    if entries.is_empty() {
        return Ok(BTreeMap::new());
    }
    match run_on {
        None => {
            return Err(CliError::Usage(
                "--provider-config requires --run-on".into(),
            ))
        }
        Some(RunOn::Host(_)) => {
            return Err(CliError::Usage(
                "--provider-config applies only to a compute provider; use --workdir \
                 for the folder on a machine"
                    .into(),
            ))
        }
        Some(RunOn::Provider(_)) => {}
    }
    if entries.len() > MAX_PROVIDER_CONFIG_ENTRIES {
        return Err(CliError::Usage(format!(
            "--provider-config accepts at most {MAX_PROVIDER_CONFIG_ENTRIES} entries"
        )));
    }
    let map = key_values(entries, "--provider-config")?;
    for (key, value) in &map {
        if !is_provider_config_key(key) {
            return Err(CliError::Usage(format!(
                "invalid --provider-config key '{key}': use lowercase letters, digits and '_', \
                 not starting with a digit (max {MAX_ID_CHARS} characters)"
            )));
        }
        if value.trim().is_empty() {
            return Err(CliError::Usage(format!(
                "--provider-config {key} needs a value"
            )));
        }
    }
    Ok(map)
}

fn avatar(
    emoji: Option<String>,
    color: Option<String>,
) -> Result<Option<CreateAgentAvatar>, CliError> {
    let (emoji, color) = match (emoji, color) {
        (None, None) => return Ok(None),
        (Some(emoji), Some(color)) => (emoji.trim().to_owned(), color.trim().to_owned()),
        _ => {
            return Err(CliError::Usage(
                "--avatar-emoji and --avatar-color must be given together".into(),
            ))
        }
    };
    if emoji.graphemes(true).count() != 1 || emoji.len() > MAX_EMOJI_BYTES {
        return Err(CliError::Usage(format!(
            "--avatar-emoji must be a single emoji (max {MAX_EMOJI_BYTES} bytes)"
        )));
    }
    let color_valid = color.len() == 7
        && color.starts_with('#')
        && color.bytes().skip(1).all(|byte| byte.is_ascii_hexdigit());
    if !color_valid {
        return Err(CliError::Usage(format!(
            "--avatar-color must be a #RRGGBB hex color, got '{color}'"
        )));
    }
    Ok(Some(CreateAgentAvatar { emoji, color }))
}

fn build<T: Serialize>(
    keys: &Keys,
    owner: &PublicKey,
    channel_id: String,
    request_kind: &'static str,
    action: &'static str,
    request: T,
) -> Result<BuiltDraftRequest, CliError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let payload = ObserverEvent {
        seq: 0,
        timestamp: chrono::Utc::now().to_rfc3339(),
        kind: request_kind,
        agent_index: None,
        channel_id: Some(channel_id),
        session_id: None,
        turn_id: None,
        payload: ManagementRequest {
            request_type: request_kind,
            action,
            request_id: request_id.clone(),
            request,
        },
    };
    let encrypted = encrypt_observer_payload(keys, owner, &payload)
        .map_err(|error| CliError::Other(format!("could not encrypt draft request: {error}")))?;
    let event = buzz_sdk::build_agent_observer_frame(
        &owner.to_hex(),
        &keys.public_key().to_hex(),
        OBSERVER_FRAME_TELEMETRY,
        &encrypted,
    )
    .map_err(|error| CliError::Other(format!("could not build draft request: {error}")))?
    .sign_with_keys(keys)
    .map_err(|error| CliError::Other(format!("could not sign draft request: {error}")))?;
    Ok(BuiltDraftRequest {
        event,
        request_id,
        action,
        warnings: Vec::new(),
    })
}

pub fn build_create(
    keys: &Keys,
    owner: &PublicKey,
    draft: CreateAgentDraft,
) -> Result<BuiltDraftRequest, CliError> {
    let channel_id = required(draft.channel_id, "channel", 128)?;
    uuid::Uuid::parse_str(&channel_id)
        .map_err(|_| CliError::Usage(format!("invalid channel UUID: {channel_id}")))?;
    // The prompt may be empty: the owner can write it in the Desktop dialog.
    let system_prompt = draft.system_prompt.trim().to_owned();
    if system_prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(CliError::Usage(format!(
            "system prompt is too long (max {MAX_PROMPT_CHARS} characters)"
        )));
    }
    let respond_to = optional(draft.respond_to, "respond-to")?;
    if let Some(value) = respond_to.as_deref() {
        if !RESPOND_TO_VALUES.contains(&value) {
            return Err(CliError::Usage(format!(
                "respond-to must be one of: {}",
                RESPOND_TO_VALUES.join(", ")
            )));
        }
    }
    let run_on = run_on_target(draft.run_on)?;
    let mut provider_config = provider_config(draft.provider_config, run_on.as_ref())?;
    let (run_on, run_on_host, host_workdir) =
        run_on_fields(run_on, workdir(draft.workdir)?, &mut provider_config)?;
    let env_vars = env_vars(draft.env_vars)?;
    let warnings = resume_session_warnings(
        env_vars.get(RESUME_SESSION_ENV_KEY).map(String::as_str),
        run_on.is_some() || run_on_host.is_some(),
        host_workdir.as_deref().or_else(|| {
            provider_config
                .get(PROVIDER_WORKDIR_KEY)
                .map(String::as_str)
        }),
        dirs::home_dir().as_deref(),
    );
    let request = CreateAgentRequest {
        channel_id: channel_id.clone(),
        display_name: required(draft.display_name, "display name", MAX_NAME_CHARS)?,
        system_prompt,
        runtime: optional_id(draft.runtime, "runtime")?,
        model: optional(draft.model, "model")?,
        respond_to,
        env_vars,
        avatar: avatar(draft.avatar_emoji, draft.avatar_color)?,
        run_on,
        provider_config,
        run_on_host,
        host_workdir,
    };
    let mut built = build(
        keys,
        owner,
        channel_id,
        AGENT_REQUEST_KIND,
        "create",
        request,
    )?;
    built.warnings = warnings;
    Ok(built)
}

pub fn build_update(
    keys: &Keys,
    owner: &PublicKey,
    draft: UpdateAgentDraft,
) -> Result<BuiltDraftRequest, CliError> {
    let channel_id = required(draft.channel_id, "channel", 128)?;
    uuid::Uuid::parse_str(&channel_id)
        .map_err(|_| CliError::Usage(format!("invalid channel UUID: {channel_id}")))?;
    let respond_to = optional(draft.respond_to, "respond-to")?;
    if respond_to
        .as_deref()
        .is_some_and(|value| value != "owner-only" && value != "anyone")
    {
        return Err(CliError::Usage(
            "respond-to must be owner-only or anyone".into(),
        ));
    }
    let request = UpdateAgentDraft {
        channel_id: channel_id.clone(),
        agent_name: required(draft.agent_name, "agent name", MAX_NAME_CHARS)?,
        display_name: optional(draft.display_name, "display name")?,
        system_prompt: draft
            .system_prompt
            .map(|value| required(value, "system prompt", MAX_PROMPT_CHARS))
            .transpose()?,
        runtime: optional(draft.runtime, "runtime")?,
        provider: optional(draft.provider, "provider")?,
        model: optional(draft.model, "model")?,
        respond_to,
    };
    if request.display_name.is_none()
        && request.system_prompt.is_none()
        && request.runtime.is_none()
        && request.provider.is_none()
        && request.model.is_none()
        && request.respond_to.is_none()
    {
        return Err(CliError::Usage(
            "include at least one field to update".into(),
        ));
    }
    build(
        keys,
        owner,
        channel_id,
        AGENT_REQUEST_KIND,
        "update",
        request,
    )
}

pub fn build_project_channel(
    keys: &Keys,
    owner: &PublicKey,
    draft: CreateProjectChannelDraft,
) -> Result<BuiltDraftRequest, CliError> {
    let home_channel_id = required(draft.home_channel_id, "home channel", 128)?;
    uuid::Uuid::parse_str(&home_channel_id)
        .map_err(|_| CliError::Usage(format!("invalid channel UUID: {home_channel_id}")))?;
    let visibility = required(draft.visibility, "visibility", 16)?;
    if visibility != "open" && visibility != "private" {
        return Err(CliError::Usage("visibility must be open or private".into()));
    }
    if draft.ttl_seconds == Some(0) {
        return Err(CliError::Usage("ttl must be greater than zero".into()));
    }
    let request = CreateProjectChannelDraft {
        home_channel_id: home_channel_id.clone(),
        name: required(draft.name, "name", MAX_NAME_CHARS)?,
        description: draft
            .description
            .map(|value| required(value, "description", 2_048))
            .transpose()?,
        visibility,
        ttl_seconds: draft.ttl_seconds,
        template_name: optional(draft.template_name, "template")?,
    };
    build(
        keys,
        owner,
        home_channel_id,
        PROJECT_CHANNEL_REQUEST_KIND,
        "create",
        request,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::observer::{decrypt_observer_payload, OBSERVER_AGENT_TAG, OBSERVER_FRAME_TAG};

    const CHANNEL: &str = "7c07e659-3610-42f4-9a5e-1e9973c09da9";

    #[test]
    fn create_is_owner_encrypted_and_matches_desktop_contract() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let built = build_create(
            &agent,
            &owner.public_key(),
            CreateAgentDraft {
                channel_id: CHANNEL.into(),
                display_name: "Research helper".into(),
                system_prompt: "Find sources.".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(built.event.kind.as_u16(), 24_200);
        let tags: Vec<Vec<String>> = built
            .event
            .tags
            .iter()
            .map(|tag| tag.as_slice().to_vec())
            .collect();
        assert!(tags
            .iter()
            .any(|tag| tag == &["p", &owner.public_key().to_hex()]));
        assert!(tags
            .iter()
            .any(|tag| tag == &[OBSERVER_AGENT_TAG, &agent.public_key().to_hex()]));
        assert!(tags
            .iter()
            .any(|tag| tag == &[OBSERVER_FRAME_TAG, OBSERVER_FRAME_TELEMETRY]));
        assert!(!tags
            .iter()
            .any(|tag| tag.first().map(String::as_str) == Some("h")));

        let payload: serde_json::Value = decrypt_observer_payload(&owner, &built.event).unwrap();
        assert_eq!(payload["kind"], AGENT_REQUEST_KIND);
        assert_eq!(payload["channelId"], CHANNEL);
        assert_eq!(payload["payload"]["type"], AGENT_REQUEST_KIND);
        assert_eq!(payload["payload"]["action"], "create");
        assert_eq!(
            payload["payload"]["request"]["displayName"],
            "Research helper"
        );
        // Unset optional fields are omitted entirely (never null): Desktop rejects extra keys.
        let request = payload["payload"]["request"].as_object().unwrap();
        let mut keys: Vec<&str> = request.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["channelId", "displayName", "systemPrompt"]);
        assert_eq!(request["systemPrompt"], "Find sources.");
    }

    const RESUME: &str = "0f3c2a9e-6b1d-4e7a-9c55-2d8f1a3b4c6e";

    fn full_draft() -> CreateAgentDraft {
        CreateAgentDraft {
            channel_id: CHANNEL.into(),
            display_name: "Session resumer".into(),
            system_prompt: String::new(),
            runtime: Some("claude".into()),
            model: Some("claude-opus-4".into()),
            respond_to: Some("allowlist".into()),
            env_vars: vec![
                "ANTHROPIC_AUTH_TOKEN=".into(),
                "ANTHROPIC_BASE_URL=https://gateway.example/v1".into(),
                format!("BUZZ_ACP_RESUME_SESSION={RESUME}"),
            ],
            avatar_emoji: Some("🐝".into()),
            avatar_color: Some("#1A2b3C".into()),
            run_on: Some("remote-host".into()),
            provider_config: vec!["host=box-7.internal".into(), "workdir=/srv/repo".into()],
            workdir: None,
        }
    }

    fn host_draft(run_on: &str) -> CreateAgentDraft {
        CreateAgentDraft {
            run_on: Some(run_on.into()),
            provider_config: Vec::new(),
            workdir: Some("~/code/app".into()),
            ..full_draft()
        }
    }

    #[test]
    fn create_carries_a_machine_target_by_name_with_its_folder() {
        let request = decrypted_request(host_draft("host: Dev Box "));
        assert_eq!(request["runOnHost"], "Dev Box");
        assert_eq!(request["hostWorkdir"], "~/code/app");
        // A machine draft never sends provider fields.
        assert!(request.get("runOn").is_none());
        assert!(request.get("providerConfig").is_none());
        // The resumed session and the folder travel together.
        assert_eq!(request["envVars"][RESUME_SESSION_ENV_KEY], RESUME);
    }

    #[test]
    fn create_normalizes_a_machine_pubkey_or_npub_to_lowercase_hex() {
        let machine = Keys::generate().public_key();
        let hex = machine.to_hex();
        let npub = nostr::ToBech32::to_bech32(&machine).unwrap();
        for reference in [hex.to_uppercase(), npub] {
            let request = decrypted_request(CreateAgentDraft {
                workdir: None,
                ..host_draft(&format!("host:{reference}"))
            });
            assert_eq!(request["runOnHost"], hex.as_str(), "{reference}");
            assert!(request.get("hostWorkdir").is_none());
        }
    }

    #[test]
    fn create_sends_a_provider_workdir_as_provider_config() {
        let request = decrypted_request(CreateAgentDraft {
            provider_config: vec!["host=box-7.internal".into()],
            workdir: Some("/srv/repo".into()),
            ..full_draft()
        });
        assert_eq!(request["runOn"], "remote-host");
        assert_eq!(
            request["providerConfig"],
            serde_json::json!({ "host": "box-7.internal", "workdir": "/srv/repo" })
        );
        assert!(request.get("runOnHost").is_none());
        assert!(request.get("hostWorkdir").is_none());

        // Giving the same folder both ways is not a conflict.
        let request = decrypted_request(CreateAgentDraft {
            workdir: Some("/srv/repo".into()),
            ..full_draft()
        });
        assert_eq!(request["providerConfig"]["workdir"], "/srv/repo");
    }

    #[test]
    fn create_rejects_invalid_machine_targets_and_folders() {
        type Mutate = fn(&mut CreateAgentDraft);
        let cases: &[(&str, Mutate, &str)] = &[
            (
                "empty machine",
                |d| d.run_on = Some("host:  ".into()),
                "needs a machine pubkey, npub or name",
            ),
            (
                "bad npub",
                |d| d.run_on = Some("host:npub1notakey".into()),
                "invalid machine npub",
            ),
            (
                "machine name too long",
                |d| d.run_on = Some(format!("host:{}", "m".repeat(129))),
                "invalid machine name",
            ),
            (
                "machine name with a control character",
                |d| d.run_on = Some("host:dev\u{7}box".into()),
                "invalid machine name",
            ),
            (
                "provider config on a machine",
                |d| d.provider_config = vec!["host=x".into()],
                "applies only to a compute provider",
            ),
            (
                "workdir without run-on",
                |d| d.run_on = None,
                "--workdir requires --run-on",
            ),
            (
                "empty workdir",
                |d| d.workdir = Some("  ".into()),
                "--workdir needs a folder path",
            ),
            (
                "workdir too long",
                |d| d.workdir = Some(format!("/{}", "a".repeat(300))),
                "--workdir is too long",
            ),
            (
                "workdir with a newline",
                |d| d.workdir = Some("/srv/a\nb".into()),
                "one line without NUL",
            ),
            (
                "workdir with a carriage return",
                |d| d.workdir = Some("/srv/a\rb".into()),
                "one line without NUL",
            ),
            (
                "workdir with NUL",
                |d| d.workdir = Some("/srv/a\0b".into()),
                "one line without NUL",
            ),
            (
                "provider workdir conflicts with provider config",
                |d| {
                    d.run_on = Some("remote-host".into());
                    d.provider_config = vec!["workdir=/srv/other".into()];
                },
                "--workdir conflicts with --provider-config workdir=/srv/other",
            ),
            (
                "provider workdir pushes config past the cap",
                |d| {
                    d.run_on = Some("remote-host".into());
                    d.provider_config = (0..20).map(|i| format!("k{i}=v")).collect();
                },
                "at most 20 entries, including --workdir",
            ),
        ];
        for (name, mutate, expected) in cases {
            let mut draft = host_draft("host:devbox");
            mutate(&mut draft);
            let error = build_create(&Keys::generate(), &Keys::generate().public_key(), draft)
                .expect_err(name);
            assert!(
                matches!(error, CliError::Usage(_)),
                "{name}: expected a usage error, got {error:?}"
            );
            assert!(
                error.to_string().contains(expected),
                "{name}: '{error}' does not contain '{expected}'"
            );
        }
    }

    #[test]
    fn create_warns_when_a_remote_resume_has_no_folder() {
        let built = build_create(
            &Keys::generate(),
            &Keys::generate().public_key(),
            CreateAgentDraft {
                workdir: None,
                ..host_draft("host:devbox")
            },
        )
        .unwrap();
        assert_eq!(built.warnings.len(), 1, "{:?}", built.warnings);
        assert!(built.warnings[0].contains("without --workdir"));
        assert!(built.warnings[0].contains(RESUME));

        // A local agent cannot take a folder, so there is nothing to warn about.
        let built = build_create(
            &Keys::generate(),
            &Keys::generate().public_key(),
            CreateAgentDraft {
                run_on: None,
                workdir: None,
                ..host_draft("host:devbox")
            },
        )
        .unwrap();
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    }

    fn place_session(home: &Path, project_dir: &str) {
        let dir = home.join(".claude").join("projects").join(project_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{RESUME}.jsonl")), "{}\n").unwrap();
    }

    #[test]
    fn resume_check_accepts_the_session_in_the_folders_project_dir() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join("code").join("my_app.v2");
        let dir = claude_project_dir_name(&folder.to_string_lossy());
        assert!(dir.ends_with("-code-my-app-v2"), "{dir}");
        place_session(home.path(), &dir);
        for workdir in [
            folder.to_string_lossy().into_owned(),
            format!("{}/", folder.to_string_lossy()),
            "~/code/my_app.v2".to_owned(),
        ] {
            let warnings =
                resume_session_warnings(Some(RESUME), true, Some(&workdir), Some(home.path()));
            assert!(warnings.is_empty(), "{workdir}: {warnings:?}");
        }
    }

    #[test]
    fn resume_check_warns_when_the_session_belongs_to_another_folder() {
        let home = tempfile::tempdir().unwrap();
        place_session(home.path(), "-elsewhere-repo");
        let warnings =
            resume_session_warnings(Some(RESUME), true, Some("~/code/app"), Some(home.path()));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("~/.claude/projects/-elsewhere-repo/"));
        assert!(warnings[0].contains("--workdir ~/code/app"));
    }

    #[test]
    fn resume_check_is_silent_when_the_session_is_not_on_this_computer() {
        let home = tempfile::tempdir().unwrap();
        place_session(home.path(), "-unrelated");
        std::fs::remove_file(
            home.path()
                .join(".claude/projects/-unrelated")
                .join(format!("{RESUME}.jsonl")),
        )
        .unwrap();
        for workdir in ["~/code/app", "/srv/app", "relative/app"] {
            let warnings =
                resume_session_warnings(Some(RESUME), true, Some(workdir), Some(home.path()));
            assert!(warnings.is_empty(), "{workdir}: {warnings:?}");
        }
        assert!(resume_session_warnings(None, true, None, Some(home.path())).is_empty());
    }

    fn decrypted_request(draft: CreateAgentDraft) -> serde_json::Value {
        let owner = Keys::generate();
        let built = build_create(&Keys::generate(), &owner.public_key(), draft).unwrap();
        let payload: serde_json::Value = decrypt_observer_payload(&owner, &built.event).unwrap();
        payload["payload"]["request"].clone()
    }

    #[test]
    fn create_carries_every_contract_field_when_set() {
        let request = decrypted_request(full_draft());
        assert_eq!(
            request,
            serde_json::json!({
                "channelId": CHANNEL,
                "displayName": "Session resumer",
                "systemPrompt": "",
                "runtime": "claude",
                "model": "claude-opus-4",
                "respondTo": "allowlist",
                "envVars": {
                    "ANTHROPIC_AUTH_TOKEN": "",
                    "ANTHROPIC_BASE_URL": "https://gateway.example/v1",
                    "BUZZ_ACP_RESUME_SESSION": RESUME,
                },
                "avatar": { "emoji": "🐝", "color": "#1A2b3C" },
                "runOn": "remote-host",
                "providerConfig": { "host": "box-7.internal", "workdir": "/srv/repo" },
            })
        );
    }

    #[test]
    fn create_accepts_every_respond_to_mode_and_run_on_without_config() {
        for mode in ["owner-only", "allowlist", "anyone", "nobody"] {
            let request = decrypted_request(CreateAgentDraft {
                respond_to: Some(mode.into()),
                provider_config: Vec::new(),
                ..full_draft()
            });
            assert_eq!(request["respondTo"], mode);
            assert_eq!(request["runOn"], "remote-host");
            assert!(request.get("providerConfig").is_none());
        }
    }

    #[test]
    fn create_rejects_contract_violations() {
        type Mutate = fn(&mut CreateAgentDraft);
        let cases: &[(&str, Mutate, &str)] = &[
            (
                "disallowed env key",
                |d| d.env_vars.push("OPENAI_API_KEY=x".into()),
                "allowed keys: ANTHROPIC_AUTH_TOKEN, ANTHROPIC_BASE_URL, BUZZ_ACP_RESUME_SESSION",
            ),
            (
                "env entry without '='",
                |d| d.env_vars = vec!["ANTHROPIC_BASE_URL".into()],
                "expects KEY=VALUE",
            ),
            (
                "duplicate env key",
                |d| d.env_vars.push("ANTHROPIC_AUTH_TOKEN=".into()),
                "more than once",
            ),
            (
                "env value too long",
                |d| d.env_vars = vec![format!("ANTHROPIC_BASE_URL={}", "a".repeat(301))],
                "too long",
            ),
            (
                "resume id not a uuid",
                |d| d.env_vars = vec!["BUZZ_ACP_RESUME_SESSION=latest".into()],
                "session UUID",
            ),
            (
                "resume id uppercase",
                |d| d.env_vars = vec![format!("BUZZ_ACP_RESUME_SESSION={}", RESUME.to_uppercase())],
                "session UUID",
            ),
            (
                "resume id without hyphens",
                |d| {
                    d.env_vars = vec![format!(
                        "BUZZ_ACP_RESUME_SESSION={}",
                        RESUME.replace('-', "")
                    )]
                },
                "session UUID",
            ),
            (
                "color without #",
                |d| d.avatar_color = Some("1a2b3c".into()),
                "#RRGGBB",
            ),
            (
                "color short form",
                |d| d.avatar_color = Some("#abc".into()),
                "#RRGGBB",
            ),
            (
                "color non-hex",
                |d| d.avatar_color = Some("#12345g".into()),
                "#RRGGBB",
            ),
            (
                "emoji without color",
                |d| d.avatar_color = None,
                "given together",
            ),
            (
                "color without emoji",
                |d| d.avatar_emoji = None,
                "given together",
            ),
            (
                "two emoji",
                |d| d.avatar_emoji = Some("🐝🐝".into()),
                "single emoji",
            ),
            (
                "emoji over 16 bytes",
                |d| d.avatar_emoji = Some("👩‍👩‍👧‍👦".into()),
                "single emoji",
            ),
            (
                "provider config without run-on",
                |d| d.run_on = None,
                "--provider-config requires --run-on",
            ),
            (
                "provider config key with uppercase",
                |d| d.provider_config = vec!["Host=x".into()],
                "invalid --provider-config key",
            ),
            (
                "provider config key starting with digit",
                |d| d.provider_config = vec!["1host=x".into()],
                "invalid --provider-config key",
            ),
            (
                "provider config empty value",
                |d| d.provider_config = vec!["host=".into()],
                "needs a value",
            ),
            (
                "too many provider config entries",
                |d| d.provider_config = (0..21).map(|i| format!("k{i}=v")).collect(),
                "at most 20",
            ),
            (
                "runtime uppercase",
                |d| d.runtime = Some("Claude".into()),
                "invalid runtime",
            ),
            (
                "runtime leading dash",
                |d| d.runtime = Some("-claude".into()),
                "invalid runtime",
            ),
            (
                "runtime too long",
                |d| d.runtime = Some("a".repeat(65)),
                "invalid runtime",
            ),
            (
                "run-on with space",
                |d| d.run_on = Some("render illa".into()),
                "invalid run-on",
            ),
            (
                "run-on with dot",
                |d| d.run_on = Some("remote-host.v2".into()),
                "invalid run-on",
            ),
            (
                "unknown respond-to",
                |d| d.respond_to = Some("everyone".into()),
                "respond-to must be one of",
            ),
            (
                "model too long",
                |d| d.model = Some("m".repeat(301)),
                "model is too long",
            ),
            (
                "system prompt too long",
                |d| d.system_prompt = "p".repeat(MAX_PROMPT_CHARS + 1),
                "system prompt is too long",
            ),
            (
                "display name too long",
                |d| d.display_name = "n".repeat(MAX_NAME_CHARS + 1),
                "display name is too long",
            ),
        ];
        for (name, mutate, expected) in cases {
            let mut draft = full_draft();
            mutate(&mut draft);
            let error = build_create(&Keys::generate(), &Keys::generate().public_key(), draft)
                .expect_err(name);
            assert!(
                matches!(error, CliError::Usage(_)),
                "{name}: expected a usage error, got {error:?}"
            );
            assert!(
                error.to_string().contains(expected),
                "{name}: '{error}' does not contain '{expected}'"
            );
        }
    }

    #[test]
    fn update_requires_a_change() {
        let error = build_update(
            &Keys::generate(),
            &Keys::generate().public_key(),
            UpdateAgentDraft {
                channel_id: CHANNEL.into(),
                agent_name: "Scout".into(),
                display_name: None,
                system_prompt: None,
                runtime: None,
                provider: None,
                model: None,
                respond_to: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("at least one field"));
    }

    #[test]
    fn create_rejects_invalid_channel() {
        let error = build_create(
            &Keys::generate(),
            &Keys::generate().public_key(),
            CreateAgentDraft {
                channel_id: "general".into(),
                display_name: "Scout".into(),
                system_prompt: "Help".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("invalid channel UUID"));
    }

    #[test]
    fn project_channel_request_is_owner_encrypted() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let built = build_project_channel(
            &agent,
            &owner.public_key(),
            CreateProjectChannelDraft {
                home_channel_id: CHANNEL.into(),
                name: "release-planning".into(),
                description: Some("Coordinate the next release.".into()),
                visibility: "open".into(),
                ttl_seconds: None,
                template_name: Some("Release team".into()),
            },
        )
        .unwrap();

        let payload: serde_json::Value = decrypt_observer_payload(&owner, &built.event).unwrap();
        assert_eq!(payload["kind"], PROJECT_CHANNEL_REQUEST_KIND);
        assert_eq!(payload["channelId"], CHANNEL);
        assert_eq!(payload["payload"]["type"], PROJECT_CHANNEL_REQUEST_KIND);
        assert_eq!(payload["payload"]["action"], "create");
        assert_eq!(payload["payload"]["request"]["homeChannelId"], CHANNEL);
        assert_eq!(
            payload["payload"]["request"]["templateName"],
            "Release team"
        );
    }
}
