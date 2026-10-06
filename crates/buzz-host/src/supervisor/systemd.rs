//! systemd user-unit supervision (Linux).
//!
//! Ported from `scripts/buzz-backend-ssh`: one template unit,
//! `buzz-host-agent@.service`, instantiated per agent pubkey. The unit runs
//! `buzz host exec-agent <agent_pubkey>`, which execs `buzz-acp` with the
//! env from `agents/<agent_pubkey>.json`. A clean exit (the relay
//! `!shutdown`) keeps the unit down; crashes restart with a start rate limit.
//!
//! The template is named `buzz-host-agent@` rather than the SSH provider's
//! `buzz-agent@` so both can coexist on one machine.

use std::path::PathBuf;

use crate::error::{HostError, Result};
use crate::protocol::{AgentRunState, AgentStatus};
use crate::store::{self, HostPaths};

use super::{rotate_log, CommandRunner};

/// Template unit file name.
pub const TEMPLATE_NAME: &str = "buzz-host-agent@.service";

/// Instance unit name for one agent.
pub fn unit_name(agent_pubkey: &str) -> String {
    format!("buzz-host-agent@{agent_pubkey}.service")
}

/// Quote one `ExecStart=` argument for systemd.
fn systemd_quote(arg: &str) -> String {
    let escaped = arg
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

/// Render the template unit. `exec_prefix` re-invokes `buzz host`.
pub fn render_template(exec_prefix: &[String], logs_dir: &std::path::Path) -> String {
    let exec: Vec<String> = exec_prefix.iter().map(|a| systemd_quote(a)).collect();
    let logs = logs_dir.to_string_lossy().replace('%', "%%");
    format!(
        "[Unit]\n\
Description=Buzz agent %i (buzz host)\n\
After=network-online.target\n\
StartLimitIntervalSec=600\n\
StartLimitBurst=5\n\
\n\
[Service]\n\
ExecStart={exec} exec-agent %i\n\
StandardOutput=append:{logs}/%i.log\n\
StandardError=append:{logs}/%i.log\n\
Restart=on-failure\n\
RestartPreventExitStatus=0\n\
RestartSec=10\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        exec = exec.join(" "),
    )
}

/// Default systemd user unit directory.
pub fn user_unit_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .map_or_else(|| store::home_dir().map(|h| h.join(".config")), Ok)?;
    Ok(base.join("systemd").join("user"))
}

/// Supervises agents as systemd user units.
pub struct SystemdSupervisor {
    paths: HostPaths,
    unit_dir: PathBuf,
    exec_prefix: Vec<String>,
    runner: Box<dyn CommandRunner>,
}

impl SystemdSupervisor {
    /// Supervisor writing units to the user's unit directory.
    pub fn new(paths: HostPaths, runner: Box<dyn CommandRunner>) -> Result<Self> {
        Ok(Self::with_unit_dir(
            paths,
            user_unit_dir()?,
            crate::tools::self_command(),
            runner,
        ))
    }

    /// Supervisor with explicit unit directory and exec prefix (tests).
    pub fn with_unit_dir(
        paths: HostPaths,
        unit_dir: PathBuf,
        exec_prefix: Vec<String>,
        runner: Box<dyn CommandRunner>,
    ) -> Self {
        Self {
            paths,
            unit_dir,
            exec_prefix,
            runner,
        }
    }

    async fn systemctl(&self, args: &[&str]) -> Result<String> {
        let mut full = vec!["--user".to_string()];
        full.extend(args.iter().map(|a| a.to_string()));
        let out = self.runner.run("systemctl", full).await?;
        if out.success {
            Ok(out.stdout)
        } else {
            Err(HostError::Supervisor(format!(
                "systemctl {} failed: {}",
                args.join(" "),
                out.stderr
            )))
        }
    }

    /// Write the template unit if it changed; returns whether it changed.
    fn write_template(&self) -> Result<bool> {
        let content = render_template(&self.exec_prefix, &self.paths.logs_dir());
        let path = self.unit_dir.join(TEMPLATE_NAME);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(content.as_str()) {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.unit_dir)
            .map_err(|e| HostError::io(format!("create {}", self.unit_dir.display()), e))?;
        std::fs::write(&path, content)
            .map_err(|e| HostError::io(format!("write {}", path.display()), e))?;
        Ok(true)
    }

    /// Install the template, then enable and (re)start the agent's unit.
    /// `restart` starts a stopped unit and restarts a running one, so a
    /// repeated deploy never creates a second instance.
    pub async fn apply(&mut self, agent_pubkey: &str) -> Result<()> {
        store::ensure_private_dir(&self.paths.logs_dir())?;
        rotate_log(&self.paths.agent_log(agent_pubkey));
        if self.write_template()? {
            self.systemctl(&["daemon-reload"]).await?;
        }
        let unit = unit_name(agent_pubkey);
        // A unit that hit its start limit refuses `restart` until reset.
        let _ = self.systemctl(&["reset-failed", &unit]).await;
        self.systemctl(&["enable", &unit]).await?;
        self.systemctl(&["restart", &unit]).await?;
        Ok(())
    }

    /// Disable and stop the agent's unit. A unit that does not exist is
    /// already removed.
    pub async fn remove(&mut self, agent_pubkey: &str) -> Result<()> {
        let unit = unit_name(agent_pubkey);
        if let Err(e) = self.systemctl(&["disable", "--now", &unit]).await {
            let msg = e.to_string();
            if !(msg.contains("not loaded") || msg.contains("does not exist")) {
                return Err(e);
            }
        }
        let _ = self.systemctl(&["reset-failed", &unit]).await;
        Ok(())
    }

    /// Query each agent's unit state.
    pub async fn states(&mut self, agents: &[String]) -> Vec<AgentStatus> {
        let mut out = Vec::with_capacity(agents.len());
        for agent in agents {
            let unit = unit_name(agent);
            let shown = self
                .systemctl(&[
                    "show",
                    "--timestamp=unix",
                    "-p",
                    "ActiveState",
                    "-p",
                    "SubState",
                    "-p",
                    "StateChangeTimestamp",
                    &unit,
                ])
                .await
                .unwrap_or_default();
            let (state, since) = parse_show(&shown);
            let since = since.unwrap_or_else(|| {
                store::load_agent(&self.paths, agent)
                    .ok()
                    .flatten()
                    .map(|r| r.deployed_at)
                    .unwrap_or(0)
            });
            out.push(AgentStatus {
                agent_pubkey: agent.clone(),
                state,
                since,
            });
        }
        out
    }
}

/// Map `systemctl show` output to a run state and change timestamp.
pub fn parse_show(text: &str) -> (AgentRunState, Option<u64>) {
    let mut active = "";
    let mut sub = "";
    let mut since = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("ActiveState=") {
            active = v.trim();
        } else if let Some(v) = line.strip_prefix("SubState=") {
            sub = v.trim();
        } else if let Some(v) = line.strip_prefix("StateChangeTimestamp=") {
            since = v.trim().trim_start_matches('@').parse::<u64>().ok();
        }
    }
    let state = match (active, sub) {
        (_, "auto-restart") | ("failed", _) => AgentRunState::Failed,
        ("active", _) | ("activating", _) | ("reloading", _) => AgentRunState::Running,
        _ => AgentRunState::Stopped,
    };
    (state, since)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::supervisor::{BoxFuture, CmdOutput};
    use std::sync::{Arc, Mutex};

    /// Records every command and answers success.
    #[derive(Clone, Default)]
    pub(crate) struct RecordingRunner {
        pub calls: Arc<Mutex<Vec<String>>>,
    }

    impl CommandRunner for RecordingRunner {
        fn run<'a>(
            &'a self,
            program: &'a str,
            args: Vec<String>,
        ) -> BoxFuture<'a, Result<CmdOutput>> {
            let line = format!("{program} {}", args.join(" "));
            Box::pin(async move {
                if let Ok(mut calls) = self.calls.lock() {
                    calls.push(line);
                }
                Ok(CmdOutput {
                    success: true,
                    ..CmdOutput::default()
                })
            })
        }
    }

    impl RecordingRunner {
        pub(crate) fn take(&self) -> Vec<String> {
            self.calls
                .lock()
                .map(|mut c| std::mem::take(&mut *c))
                .unwrap_or_default()
        }
    }

    #[test]
    fn parse_show_table() {
        let cases = [
            (
                "ActiveState=active\nSubState=running\nStateChangeTimestamp=@17",
                AgentRunState::Running,
                Some(17),
            ),
            (
                "ActiveState=activating\nSubState=auto-restart\n",
                AgentRunState::Failed,
                None,
            ),
            (
                "ActiveState=failed\nSubState=failed\n",
                AgentRunState::Failed,
                None,
            ),
            (
                "ActiveState=inactive\nSubState=dead\n",
                AgentRunState::Stopped,
                None,
            ),
            ("", AgentRunState::Stopped, None),
        ];
        for (text, state, since) in cases {
            assert_eq!(parse_show(text), (state, since), "{text}");
        }
    }

    #[test]
    fn template_restarts_on_failure_only_with_rate_limit() {
        let t = render_template(
            &["/home/u/.local/bin/buzz".into(), "host".into()],
            std::path::Path::new("/home/u/.config/buzz/host/logs"),
        );
        for line in [
            "Restart=on-failure",
            "RestartPreventExitStatus=0",
            "StartLimitBurst=5",
            "ExecStart=\"/home/u/.local/bin/buzz\" \"host\" exec-agent %i",
        ] {
            assert!(t.contains(line), "missing {line:?} in\n{t}");
        }
    }
}
