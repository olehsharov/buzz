//! Shared agent capabilities. Intake and scheduling belong to each entry point;
//! environment, credentials, adapter settings and prompt equipment belong here.
use crate::{
    build_mcp_servers, config::Config, current_working_directory, git, observer, pool, relay,
    resolve_agent_owner, PromptContext,
};
use anyhow::Result;
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

pub(crate) enum SessionMode {
    Conversation,
    Task,
}

/// Own shared launch resources until all adapter work has been drained.
/// Add new runtime capabilities here, not separately to the CLI entry points.
pub(crate) struct AgentRuntime {
    config: Config,
    _git_environment: git::GitEnvironment,
}

impl AgentRuntime {
    pub(crate) fn prepare(mut config: Config) -> Result<Self> {
        let git_environment = git::GitEnvironment::for_config(&mut config)?;
        Ok(Self {
            config,
            _git_environment: git_environment,
        })
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn startup(&self, observer: Option<observer::ObserverHandle>) -> PoolStartup {
        PoolStartup::from_config(&self.config, observer)
    }

    pub(crate) fn prompt_context(
        &self,
        rest: relay::RestClient,
        channels: HashMap<Uuid, relay::ChannelInfo>,
        mode: SessionMode,
    ) -> Result<PromptContext> {
        make_prompt_context(&self.config, rest, channels, mode)
    }
}

#[derive(Clone)]
pub(crate) struct PoolStartup {
    pub(crate) agents: u32,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) extra_env: Vec<(String, String)>,
    pub(crate) has_generated_codex_config: bool,
    pub(crate) model: Option<String>,
    pub(crate) effort_level: Option<String>,
    pub(crate) observer: Option<observer::ObserverHandle>,
}

impl PoolStartup {
    fn from_config(config: &Config, observer: Option<observer::ObserverHandle>) -> Self {
        Self {
            agents: config.agents,
            command: config.agent_command.clone(),
            args: config.agent_args.clone(),
            extra_env: config.persona_env_vars.clone(),
            has_generated_codex_config: config.has_generated_codex_config,
            model: config.model.clone(),
            effort_level: config.effort_level.clone(),
            observer,
        }
    }
}

fn make_prompt_context(
    config: &Config,
    rest_client: relay::RestClient,
    channels: HashMap<Uuid, relay::ChannelInfo>,
    mode: SessionMode,
) -> Result<PromptContext> {
    let base_prompt_content = config.base_prompt_content.as_ref();
    let cwd = current_working_directory()?;
    Ok(PromptContext {
        mcp_servers: build_mcp_servers(config),
        initial_message: config.initial_message.clone(),
        idle_timeout: Duration::from_secs(config.idle_timeout_secs),
        max_turn_duration: Duration::from_secs(config.max_turn_duration_secs),
        turn_liveness_interval: Duration::from_secs(config.turn_liveness_secs),
        dedup_mode: config.dedup_mode,
        system_prompt: config.system_prompt.clone(),
        session_title: config.session_title.clone(),
        team_instructions: config.team_instructions.clone(),
        base_prompt: if config.no_base_prompt {
            None
        } else {
            // Build standing context once under the configured policy, before
            // any session/new. Both modern ACP and legacy first-turn framing
            // consume this same assembled base (including custom base files).
            let base = base_prompt_content
                .map(String::as_str)
                .unwrap_or(include_str!("base_prompt.md"));
            Some(if matches!(mode, SessionMode::Task) {
                format!("{base}\n\n{}", include_str!("session_model_task.md"))
            } else {
                config.session_policy.append_session_model(base)
            })
        },
        heartbeat_prompt: config.heartbeat_prompt.clone(),
        cwd,
        rest_client: rest_client.clone(),
        channel_info: pool::ChannelInfoResolver::new(channels, rest_client),
        context_message_limit: config.context_message_limit,
        max_turns_per_session: config.max_turns_per_session,
        permission_mode: config.permission_mode,
        agent_keys: config.keys.clone(),
        agent_owner_pubkey: resolve_agent_owner(config)
            .as_deref()
            .and_then(|hex| nostr::PublicKey::from_hex(hex).ok()),
        memory_enabled: config.memory_enabled,
        harness_name: crate::config::normalize_agent_command_identity(&config.agent_command),
        relay_url: config.relay_url.clone(),
        resume_session: std::sync::Arc::new(std::sync::Mutex::new(pending_resume(
            config,
            |key| std::env::var_os(key),
        )?)),
    })
}

/// The pending `BUZZ_ACP_RESUME_SESSION` continuation with the store that
/// records its fork for this agent. Errors when the session is configured
/// but no state directory can be resolved: without the record, every restart
/// would silently fork the source again.
fn pending_resume(
    config: &Config,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Option<pool::PendingResume>> {
    let Some(source) = config.resume_session.clone() else {
        return Ok(None);
    };
    let state_root = crate::resume_store::state_root_from_env(env).ok_or_else(|| {
        anyhow::anyhow!(
            "BUZZ_ACP_RESUME_SESSION is set, but no state directory to record its fork \
             (set an absolute XDG_STATE_HOME or HOME)"
        )
    })?;
    let store =
        crate::resume_store::ResumeForkStore::new(&state_root, &config.keys.public_key().to_hex());
    tracing::info!(
        "BUZZ_ACP_RESUME_SESSION={source}: fork record directory {}",
        store.dir().display()
    );
    Ok(Some(pool::PendingResume { source, store }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;
    use std::ffi::OsString;

    fn config(resume: Option<&str>) -> Config {
        let mut argv = vec![
            "buzz-acp".to_string(),
            "--private-key".into(),
            "0000000000000000000000000000000000000000000000000000000000000001".into(),
        ];
        if let Some(id) = resume {
            argv.extend(["--resume-session".into(), id.into()]);
        }
        Config::from_args(crate::config::CliArgs::try_parse_from(argv).expect("args"))
            .expect("config")
    }

    const SOURCE: &str = "0f7c1a52-3b9e-4d6a-9c1e-2a8b7d4e5f60";

    #[test]
    fn pending_resume_keys_store_by_agent_under_state_root() {
        let config = config(Some(SOURCE));
        let pending = pending_resume(&config, |key| {
            (key == "XDG_STATE_HOME").then(|| OsString::from("/state"))
        })
        .expect("resolves")
        .expect("pending");
        assert_eq!(pending.source, SOURCE);
        assert_eq!(
            pending.store.dir(),
            std::path::Path::new("/state/buzz-acp/resume-sessions")
                .join(config.keys.public_key().to_hex())
        );
    }

    #[test]
    fn pending_resume_requires_a_state_dir_only_when_configured() {
        let err = pending_resume(&config(Some(SOURCE)), |_| None)
            .expect_err("no state dir while resuming must fail at startup");
        assert!(err.to_string().contains("BUZZ_ACP_RESUME_SESSION"), "{err}");
        assert!(pending_resume(&config(None), |_| None)
            .expect("unset needs no state dir")
            .is_none());
    }
}
