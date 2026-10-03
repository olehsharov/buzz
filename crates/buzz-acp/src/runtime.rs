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
                apply_stream_mode(
                    config.stream_mode,
                    config.session_policy.append_session_model(base),
                )
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
        // Needs the live relay publisher; conversation startup attaches it.
        // Isolated tasks never stream.
        stream: None,
    })
}

/// Standing-prompt lines that tell the agent to publish with the CLI, and the
/// text that replaces each one when the harness autoposts the response text.
/// Left in place, they contradict Reply Delivery: an agent that follows them
/// answers with `buzz messages send` and its streamed draft is discarded.
///
/// Each original is a whole line of `base_prompt.md` (a test pins that every
/// one occurs exactly once). A custom base prompt without a line keeps its
/// own text; the appended Reply Delivery section still applies.
const AUTOPOST_BASE_SUBSTITUTIONS: &[(&str, &str)] = &[
    (
        "- **If your turn produced anything worth knowing, you MUST publish it.** Use `buzz messages send`. \
         Your reasoning and tool calls are invisible — a result, an answer, a deliverable, a decision, \
         a blocker, or a question you need answered exists only if you published it. Work or an answer \
         that someone asked you for always counts. Ending that kind of turn without a message is a silent failure.",
        "- **If your turn produced anything worth knowing, you MUST publish it**, as described in Reply \
         Delivery. A result, an answer, a deliverable, a decision, a blocker, or a question you need \
         answered exists only if you published it. Work or an answer that someone asked you for always \
         counts. Ending that kind of turn without a reply is a silent failure.",
    ),
    (
        "- After publishing a pickup message, keep working until you publish the result, handoff, \
         blocker, or key decision or information that needs to be surfaced.",
        "- Do not post pickup or progress messages to the conversation you are answering — people \
         watch your work live, and such a message replaces your reply. Keep working until your reply \
         holds the result, handoff, blocker, or key decision or information that needs to be surfaced.",
    ),
    (
        "- **Work in the open.** Your tool calls and reasoning are invisible to humans — narrate as you \
         go in brief messages, and never go dark between \"picked up\" and \"done.\" If you didn't post \
         it, it didn't happen.",
        "- **Work in the open.** People watch your reply being written and your tool activity live; \
         what gets posted is your final response text. If it isn't in your reply, it didn't happen.",
    ),
];

/// Apply the reply-delivery contract when the harness autoposts the turn's
/// response text: swap out the standing CLI-reply lines, then append Reply
/// Delivery. `off` and `draft` change nothing the agent must do, so their
/// prompt is returned untouched.
fn apply_stream_mode(mode: crate::stream_draft::StreamMode, base: String) -> String {
    match mode {
        crate::stream_draft::StreamMode::DraftAutopost => {
            let base = AUTOPOST_BASE_SUBSTITUTIONS
                .iter()
                .fold(base, |base, (cli, autopost)| {
                    base.replacen(cli, autopost, 1)
                });
            format!(
                "{}\n\n{}",
                base.trim_end(),
                include_str!("session_stream_mode.md").trim_end()
            )
        }
        crate::stream_draft::StreamMode::Off | crate::stream_draft::StreamMode::Draft => base,
    }
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

    fn config_with_stream(stream: Option<&str>) -> Config {
        let mut argv = vec![
            "buzz-acp".to_string(),
            "--private-key".into(),
            "0000000000000000000000000000000000000000000000000000000000000001".into(),
        ];
        if let Some(mode) = stream {
            argv.extend(["--stream".into(), mode.into()]);
        }
        Config::from_args(crate::config::CliArgs::try_parse_from(argv).expect("args"))
            .expect("config")
    }

    fn base_prompt_for(config: &Config, mode: SessionMode) -> String {
        let rest = relay::RestClient {
            http: reqwest::Client::new(),
            base_url: "http://127.0.0.1:0".into(),
            keys: config.keys.clone(),
            auth_tag_json: None,
        };
        let ctx = make_prompt_context(config, rest, HashMap::new(), mode).expect("context");
        assert!(
            ctx.stream.is_none(),
            "the relay publisher is attached by startup"
        );
        ctx.base_prompt.expect("base prompt")
    }

    #[test]
    fn stream_flag_parses_into_config() {
        use crate::stream_draft::StreamMode;
        assert_eq!(config_with_stream(None).stream_mode, StreamMode::Off);
        assert_eq!(
            config_with_stream(Some("draft")).stream_mode,
            StreamMode::Draft
        );
        assert_eq!(
            config_with_stream(Some("draft+autopost")).stream_mode,
            StreamMode::DraftAutopost
        );
        assert!(crate::config::CliArgs::try_parse_from([
            "buzz-acp",
            "--private-key",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "--stream",
            "on",
        ])
        .is_err());
    }

    const BASE: &str = include_str!("base_prompt.md");
    const REPLY_DELIVERY: &str = include_str!("session_stream_mode.md");

    #[test]
    fn autopost_substitutions_each_replace_one_whole_base_prompt_line() {
        for (cli, autopost) in AUTOPOST_BASE_SUBSTITUTIONS {
            assert_eq!(
                BASE.lines().filter(|line| line == cli).count(),
                1,
                "base_prompt.md must contain this line exactly once: {cli}"
            );
            assert!(!autopost.contains("buzz messages send"), "{autopost}");
        }
    }

    #[test]
    fn autopost_conversation_prompt_replaces_the_cli_reply_contract() {
        let config = config_with_stream(Some("draft+autopost"));
        let prompt = base_prompt_for(&config, SessionMode::Conversation);
        assert!(
            !prompt.contains("Use `buzz messages send`."),
            "the base CLI-reply instruction contradicts Reply Delivery"
        );
        for (cli, autopost) in AUTOPOST_BASE_SUBSTITUTIONS {
            assert!(!prompt.contains(cli), "left in place: {cli}");
            assert_eq!(prompt.matches(autopost).count(), 1, "{autopost}");
        }
        assert!(prompt.contains("Your reply is your response text."));
        assert!(prompt
            .contains("Do not use `buzz messages send` to answer the message you were given."));
        let expected_head = AUTOPOST_BASE_SUBSTITUTIONS.iter().fold(
            config.session_policy.append_session_model(BASE),
            |base, (cli, autopost)| base.replace(cli, autopost),
        );
        assert_eq!(
            prompt,
            format!(
                "{}\n\n{}",
                expected_head.trim_end(),
                REPLY_DELIVERY.trim_end()
            ),
            "only the substituted lines change; Reply Delivery follows the session model"
        );
    }

    #[test]
    fn off_and_draft_prompts_are_the_unmodified_base() {
        for mode in [None, Some("draft")] {
            let config = config_with_stream(mode);
            assert_eq!(
                base_prompt_for(&config, SessionMode::Conversation),
                config.session_policy.append_session_model(BASE),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn isolated_tasks_never_get_the_streaming_contract() {
        for mode in [None, Some("draft"), Some("draft+autopost")] {
            assert_eq!(
                base_prompt_for(&config_with_stream(mode), SessionMode::Task),
                format!("{BASE}\n\n{}", include_str!("session_model_task.md")),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn custom_base_prompt_keeps_its_text_and_gains_reply_delivery() {
        let mut config = config_with_stream(Some("draft+autopost"));
        config.base_prompt_content = Some("Use `buzz messages send`.".into());
        assert_eq!(
            base_prompt_for(&config, SessionMode::Conversation),
            format!(
                "{}\n\n{}",
                config
                    .session_policy
                    .append_session_model("Use `buzz messages send`."),
                REPLY_DELIVERY.trim_end()
            )
        );
    }
}
