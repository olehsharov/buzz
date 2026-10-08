//! Agent supervision: systemd user units on Linux, child processes elsewhere.
//!
//! The caller owns `agents/<agent_pubkey>.json`; a supervisor only starts,
//! restarts, stops and reports the process that runs it.

pub mod process;
pub mod systemd;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::error::{HostError, Result};
use crate::protocol::AgentStatus;
use crate::store::HostPaths;

/// A boxed, sendable future (dyn-compatible async trait methods).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Captured result of an external command.
#[derive(Debug, Clone, Default)]
pub struct CmdOutput {
    /// Whether the command exited 0.
    pub success: bool,
    /// Captured stdout (bounded).
    pub stdout: String,
    /// Captured stderr (bounded).
    pub stderr: String,
}

/// Runs an external program; swappable in tests.
pub trait CommandRunner: Send + Sync {
    /// Run `program` with `args` and capture its output.
    fn run<'a>(&'a self, program: &'a str, args: Vec<String>) -> BoxFuture<'a, Result<CmdOutput>>;
}

/// Upper bound on one supervisor command (systemctl, launchctl, kill).
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// Captured output is truncated to this many bytes per stream.
pub const MAX_CAPTURE: usize = 8 * 1024;

/// The production [`CommandRunner`]: `tokio::process` with a timeout.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

fn truncate(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_CAPTURE)]);
    s.trim().to_string()
}

impl CommandRunner for SystemRunner {
    fn run<'a>(&'a self, program: &'a str, args: Vec<String>) -> BoxFuture<'a, Result<CmdOutput>> {
        Box::pin(async move {
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(&args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true);
            let out = tokio::time::timeout(COMMAND_TIMEOUT, cmd.output())
                .await
                .map_err(|_| HostError::Supervisor(format!("{program} timed out")))?
                .map_err(|e| HostError::Supervisor(format!("could not run {program}: {e}")))?;
            Ok(CmdOutput {
                success: out.status.success(),
                stdout: truncate(&out.stdout),
                stderr: truncate(&out.stderr),
            })
        })
    }
}

/// Rotate `log` to `<log>.1` once it exceeds this size, keeping disk use
/// bounded to about two files per agent.
pub const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Rotate an agent log if it grew past [`MAX_LOG_BYTES`].
pub fn rotate_log(log: &std::path::Path) {
    let big = std::fs::metadata(log)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    if big {
        let mut rotated = log.as_os_str().to_owned();
        rotated.push(".1");
        if let Err(e) = std::fs::rename(log, std::path::PathBuf::from(rotated)) {
            tracing::warn!("could not rotate {}: {e}", log.display());
        }
    }
}

/// Whether `systemctl --user is-system-running` output means the user
/// manager can run units.
pub fn user_manager_usable(state: &str) -> bool {
    matches!(
        state.trim(),
        "running" | "degraded" | "starting" | "initializing" | "maintenance"
    )
}

/// The supervisor backing this host.
pub enum Supervisor {
    /// systemd user units (Linux).
    Systemd(systemd::SystemdSupervisor),
    /// In-process child supervision (macOS, or Linux without systemd).
    Process(process::ProcessSupervisor),
}

impl Supervisor {
    /// The platform default: systemd on Linux when `systemctl --user` works,
    /// otherwise child-process supervision.
    pub async fn detect(paths: &HostPaths) -> Result<Self> {
        if cfg!(target_os = "linux") {
            let probe = SystemRunner
                .run(
                    "systemctl",
                    vec!["--user".into(), "is-system-running".into()],
                )
                .await;
            // `is-system-running` exits non-zero for "degraded" too, so judge
            // by the reported state rather than the exit code.
            if probe
                .map(|o| user_manager_usable(&o.stdout))
                .unwrap_or(false)
            {
                return Ok(Self::Systemd(systemd::SystemdSupervisor::new(
                    paths.clone(),
                    Box::new(SystemRunner),
                )?));
            }
            tracing::warn!("systemd user manager unavailable; supervising agents in-process");
        }
        Ok(Self::Process(process::ProcessSupervisor::new(
            paths.clone(),
            Box::new(SystemRunner),
        )))
    }

    /// Start the agent from its config, or restart it if it is running.
    pub async fn apply(&mut self, agent_pubkey: &str) -> Result<()> {
        match self {
            Self::Systemd(s) => s.apply(agent_pubkey).await,
            Self::Process(p) => p.apply(agent_pubkey).await,
        }
    }

    /// Stop the agent and remove its service.
    pub async fn remove(&mut self, agent_pubkey: &str) -> Result<()> {
        match self {
            Self::Systemd(s) => s.remove(agent_pubkey).await,
            Self::Process(p) => p.remove(agent_pubkey).await,
        }
    }

    /// Current state of each listed agent.
    pub async fn states(&mut self, agents: &[String]) -> Vec<AgentStatus> {
        match self {
            Self::Systemd(s) => s.states(agents).await,
            Self::Process(p) => p.states(agents),
        }
    }

    /// Periodic supervision (restart crashed children). No-op for systemd.
    pub async fn tick(&mut self) {
        if let Self::Process(p) = self {
            p.tick().await;
        }
    }

    /// Start every configured agent (daemon startup). systemd units start
    /// themselves, so only child supervision needs this.
    pub async fn start_all(&mut self, agents: &[String]) {
        if let Self::Process(p) = self {
            p.start_all(agents).await;
        }
    }

    /// Stop supervised children on daemon exit. systemd units keep running.
    pub async fn shutdown(&mut self) {
        if let Self::Process(p) = self {
            p.shutdown().await;
        }
    }
}
