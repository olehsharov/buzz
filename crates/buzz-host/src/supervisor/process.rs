//! In-process child supervision (macOS, or Linux without a systemd user
//! manager).
//!
//! Each agent runs `buzz-acp` in its own process group. A clean exit (the
//! relay `!shutdown`) leaves it stopped; a crash restarts it after a capped
//! exponential backoff, up to [`MAX_CONSECUTIVE_FAILURES`] in a row, after
//! which it stays `failed` until the next deploy. Process-group ids are
//! recorded in `pids.json` so a restarted daemon reaps orphans before
//! starting fresh instances (one live instance per agent).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::error::{HostError, Result};
use crate::protocol::{AgentRunState, AgentStatus};
use crate::store::{self, now_secs, HostPaths};

use super::{rotate_log, CommandRunner};

/// First restart delay after a crash.
pub const RESTART_BASE: Duration = Duration::from_secs(2);
/// Restart delay cap.
pub const RESTART_CAP: Duration = Duration::from_secs(300);
/// A child that ran at least this long resets its failure count.
pub const HEALTHY_RUN: Duration = Duration::from_secs(600);
/// Consecutive crashes after which the agent stays failed.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 10;
/// Grace period between SIGTERM and SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);

/// Delay before restart number `failures` (1-based), capped at [`RESTART_CAP`].
pub fn restart_delay(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(16);
    RESTART_BASE.saturating_mul(1u32 << exp).min(RESTART_CAP)
}

struct Entry {
    child: Option<tokio::process::Child>,
    pgid: Option<u32>,
    state: AgentRunState,
    since: u64,
    started: Instant,
    failures: u32,
    restart_at: Option<Instant>,
}

impl Entry {
    fn new() -> Self {
        Self {
            child: None,
            pgid: None,
            state: AgentRunState::Stopped,
            since: now_secs(),
            started: Instant::now(),
            failures: 0,
            restart_at: None,
        }
    }

    fn set_state(&mut self, state: AgentRunState) {
        if self.state != state {
            self.state = state;
            self.since = now_secs();
        }
    }
}

/// Supervises agents as child processes of the daemon.
pub struct ProcessSupervisor {
    paths: HostPaths,
    runner: Box<dyn CommandRunner>,
    entries: BTreeMap<String, Entry>,
}

impl ProcessSupervisor {
    /// New supervisor; reaps orphans recorded by a previous daemon.
    pub fn new(paths: HostPaths, runner: Box<dyn CommandRunner>) -> Self {
        Self {
            paths,
            runner,
            entries: BTreeMap::new(),
        }
    }

    fn pids_file(&self) -> std::path::PathBuf {
        self.paths.root.join("pids.json")
    }

    fn save_pids(&self) {
        let pids: BTreeMap<&str, u32> = self
            .entries
            .iter()
            .filter_map(|(k, e)| e.pgid.map(|p| (k.as_str(), p)))
            .collect();
        if let Err(e) = store::write_json(&self.pids_file(), &pids) {
            tracing::warn!("could not record agent pids: {e}");
        }
    }

    /// Kill process groups recorded by a previous daemon whose leader is
    /// still a `buzz-acp`.
    pub async fn reap_orphans(&mut self) {
        let recorded: BTreeMap<String, u32> = store::read_json(&self.pids_file())
            .ok()
            .flatten()
            .unwrap_or_default();
        for (agent, pgid) in recorded {
            if self.entries.contains_key(&agent) {
                continue;
            }
            let ps = self
                .runner
                .run(
                    "ps",
                    vec![
                        "-o".into(),
                        "command=".into(),
                        "-p".into(),
                        pgid.to_string(),
                    ],
                )
                .await;
            if ps
                .map(|o| o.success && o.stdout.contains("buzz-acp"))
                .unwrap_or(false)
            {
                tracing::info!(agent = %agent, "stopping orphaned agent process group");
                self.kill_group(pgid, "TERM").await;
            }
        }
        self.save_pids();
    }

    async fn kill_group(&self, pgid: u32, signal: &str) {
        let args = vec![format!("-{signal}"), "--".into(), format!("-{pgid}")];
        match self.runner.run("kill", args).await {
            Ok(o) if !o.success => tracing::debug!(pgid, "kill -{signal}: {}", o.stderr),
            Err(e) => tracing::warn!(pgid, "kill -{signal} failed: {e}"),
            _ => {}
        }
    }

    async fn stop(&mut self, agent: &str) {
        let Some(entry) = self.entries.get_mut(agent) else {
            return;
        };
        entry.restart_at = None;
        let pgid = entry.pgid.take();
        let child = entry.child.take();
        if let (Some(pgid), Some(mut child)) = (pgid, child) {
            self.kill_group(pgid, "TERM").await;
            if tokio::time::timeout(STOP_GRACE, child.wait())
                .await
                .is_err()
            {
                self.kill_group(pgid, "KILL").await;
                let _ = child.wait().await;
            }
        }
        if let Some(entry) = self.entries.get_mut(agent) {
            entry.set_state(AgentRunState::Stopped);
        }
    }

    fn spawn(&self, agent: &str) -> Result<tokio::process::Child> {
        let rec = store::load_agent(&self.paths, agent)?
            .ok_or_else(|| HostError::Supervisor(format!("agent {agent} has no config")))?;
        let path = rec
            .env
            .get("PATH")
            .cloned()
            .unwrap_or_else(crate::tools::agent_path);
        let acp = crate::tools::which("buzz-acp", &path).ok_or_else(|| {
            HostError::Supervisor("buzz-acp is not installed on this host".into())
        })?;
        std::fs::create_dir_all(&rec.workdir)
            .map_err(|e| HostError::io("create agent workdir", e))?;
        store::ensure_private_dir(&self.paths.logs_dir())?;
        let log_path = self.paths.agent_log(agent);
        rotate_log(&log_path);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| HostError::io("open agent log", e))?;
        let log_err = log
            .try_clone()
            .map_err(|e| HostError::io("open agent log", e))?;
        let mut cmd = tokio::process::Command::new(acp);
        cmd.envs(&rec.env)
            .current_dir(&rec.workdir)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .kill_on_drop(false);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.spawn()
            .map_err(|e| HostError::Supervisor(format!("could not start buzz-acp: {e}")))
    }

    fn start(&mut self, agent: &str) -> Result<()> {
        let result = self.spawn(agent);
        let entry = self
            .entries
            .entry(agent.to_string())
            .or_insert_with(Entry::new);
        match result {
            Ok(child) => {
                entry.pgid = child.id();
                entry.child = Some(child);
                entry.started = Instant::now();
                entry.restart_at = None;
                entry.set_state(AgentRunState::Running);
                self.save_pids();
                Ok(())
            }
            Err(e) => {
                entry.set_state(AgentRunState::Failed);
                Err(e)
            }
        }
    }

    /// Start, or stop and start again, one agent. Resets its failure count.
    pub async fn apply(&mut self, agent: &str) -> Result<()> {
        self.stop(agent).await;
        if let Some(e) = self.entries.get_mut(agent) {
            e.failures = 0;
        }
        self.start(agent)
    }

    /// Stop one agent and forget it.
    pub async fn remove(&mut self, agent: &str) -> Result<()> {
        self.stop(agent).await;
        self.entries.remove(agent);
        self.save_pids();
        Ok(())
    }

    /// Start every listed agent (daemon startup).
    pub async fn start_all(&mut self, agents: &[String]) {
        self.reap_orphans().await;
        for agent in agents {
            if let Err(e) = self.start(agent) {
                tracing::warn!(agent = %agent, "agent did not start: {e}");
                self.schedule_restart(agent);
            }
        }
    }

    fn schedule_restart(&mut self, agent: &str) {
        let Some(entry) = self.entries.get_mut(agent) else {
            return;
        };
        if entry.started.elapsed() >= HEALTHY_RUN {
            entry.failures = 0;
        }
        entry.failures = entry.failures.saturating_add(1);
        entry.set_state(AgentRunState::Failed);
        entry.restart_at = (entry.failures < MAX_CONSECUTIVE_FAILURES)
            .then(|| Instant::now() + restart_delay(entry.failures));
        if entry.restart_at.is_none() {
            tracing::warn!(agent = %agent, "agent keeps crashing; giving up until the next deploy");
        }
    }

    /// Reap exited children and restart crashed ones whose delay elapsed.
    pub async fn tick(&mut self) {
        let agents: Vec<String> = self.entries.keys().cloned().collect();
        for agent in agents {
            let exited = self.entries.get_mut(&agent).and_then(|e| {
                let status = e.child.as_mut()?.try_wait().ok()??;
                e.child = None;
                e.pgid = None;
                Some(status)
            });
            if let Some(status) = exited {
                if status.success() {
                    tracing::info!(agent = %agent, "agent exited cleanly; leaving it stopped");
                    if let Some(e) = self.entries.get_mut(&agent) {
                        e.failures = 0;
                        e.set_state(AgentRunState::Stopped);
                    }
                } else {
                    tracing::warn!(agent = %agent, "agent exited with {status}");
                    self.schedule_restart(&agent);
                }
                self.save_pids();
            }
            let due = self
                .entries
                .get(&agent)
                .and_then(|e| e.restart_at)
                .is_some_and(|at| at <= Instant::now());
            if due {
                if let Err(e) = self.start(&agent) {
                    tracing::warn!(agent = %agent, "agent restart failed: {e}");
                    self.schedule_restart(&agent);
                }
            }
        }
    }

    /// Current state of each listed agent.
    pub fn states(&self, agents: &[String]) -> Vec<AgentStatus> {
        agents
            .iter()
            .map(|a| {
                let (state, since) = self
                    .entries
                    .get(a)
                    .map(|e| (e.state, e.since))
                    .unwrap_or((AgentRunState::Stopped, 0));
                AgentStatus {
                    agent_pubkey: a.clone(),
                    state,
                    since,
                }
            })
            .collect()
    }

    /// Number of live child processes (tests and diagnostics).
    pub fn live_children(&self) -> usize {
        self.entries.values().filter(|e| e.child.is_some()).count()
    }

    /// Stop every child (daemon exit).
    pub async fn shutdown(&mut self) {
        let agents: Vec<String> = self.entries.keys().cloned().collect();
        for agent in agents {
            self.stop(&agent).await;
        }
        self.save_pids();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::AgentRecord;

    #[cfg(unix)]
    fn setup(script: &str) -> (tempfile::TempDir, ProcessSupervisor, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let acp = bin.join("buzz-acp");
        std::fs::write(&acp, script).expect("script");
        std::fs::set_permissions(&acp, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let paths = HostPaths::at(dir.path().join("host"));
        let agent = "ab".repeat(32);
        let rec = AgentRecord {
            agent_pubkey: agent.clone(),
            workdir: dir.path().join("work").to_string_lossy().into_owned(),
            env: BTreeMap::from([(
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", bin.display()),
            )]),
            deployed_at: 0,
        };
        store::write_json(&paths.agent_file(&agent), &rec).expect("record");
        let sup = ProcessSupervisor::new(paths, Box::new(super::super::SystemRunner));
        (dir, sup, agent)
    }

    /// Re-applying a running agent replaces its child: one live process.
    ///
    /// Mutation: drop `self.stop(agent)` from `apply` → two children → RED.
    #[cfg(unix)]
    #[tokio::test]
    async fn apply_twice_keeps_one_child() {
        let (_dir, mut sup, agent) = setup("#!/bin/sh\nexec sleep 30\n");
        sup.apply(&agent).await.expect("first");
        let first = sup.entries.get(&agent).and_then(|e| e.pgid).expect("pid");
        sup.apply(&agent).await.expect("second");
        let second = sup.entries.get(&agent).and_then(|e| e.pgid).expect("pid");
        assert_ne!(first, second, "restarted");
        assert_eq!(sup.live_children(), 1);
        let alive = std::process::Command::new("kill")
            .args(["-0", &first.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(!alive, "old child was stopped");
        sup.remove(&agent).await.expect("remove");
        assert_eq!(sup.live_children(), 0);
    }

    /// A crash is restarted after backoff; a clean exit stays stopped.
    #[cfg(unix)]
    #[tokio::test]
    async fn crash_schedules_restart_clean_exit_stays_down() {
        let (_dir, mut sup, agent) = setup("#!/bin/sh\nexit 3\n");
        sup.apply(&agent).await.expect("start");
        tokio::time::sleep(Duration::from_millis(300)).await;
        sup.tick().await;
        let e = sup.entries.get(&agent).expect("entry");
        assert_eq!(e.state, AgentRunState::Failed);
        assert_eq!(e.failures, 1);
        assert!(e.restart_at.is_some());

        let (_dir2, mut sup2, agent2) = setup("#!/bin/sh\nexit 0\n");
        sup2.apply(&agent2).await.expect("start");
        tokio::time::sleep(Duration::from_millis(300)).await;
        sup2.tick().await;
        let e = sup2.entries.get(&agent2).expect("entry");
        assert_eq!(e.state, AgentRunState::Stopped);
        assert!(e.restart_at.is_none());
    }

    /// After MAX_CONSECUTIVE_FAILURES the agent stops being restarted.
    #[test]
    fn crash_loop_reaches_terminal_state() {
        let paths = HostPaths::at(std::env::temp_dir().join("buzz-host-unused"));
        let mut sup = ProcessSupervisor::new(paths, Box::new(super::super::SystemRunner));
        sup.entries.insert("a".into(), Entry::new());
        for _ in 0..MAX_CONSECUTIVE_FAILURES {
            sup.schedule_restart("a");
        }
        let e = sup.entries.get("a").expect("entry");
        assert!(e.restart_at.is_none(), "gave up");
        assert_eq!(e.state, AgentRunState::Failed);
    }

    /// Restart delays grow and never exceed the cap, even for huge counts.
    ///
    /// Mutation: drop `.min(RESTART_CAP)` → RED.
    #[test]
    fn restart_delay_is_capped() {
        assert_eq!(restart_delay(1), RESTART_BASE);
        assert!(restart_delay(2) > restart_delay(1));
        for n in [8, 9, 20, 1000, u32::MAX] {
            assert!(restart_delay(n) <= RESTART_CAP, "n={n}");
        }
        assert_eq!(restart_delay(1000), RESTART_CAP);
    }
}
