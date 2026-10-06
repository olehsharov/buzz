#![deny(unsafe_code)]
//! `buzz host`: turn a machine into a Buzz agent host.
//!
//! A host pairs once with its owner's Buzz desktop over NIP-AB, then stays
//! online through the relay as a NIP-OA agent of the owner: it publishes
//! kind:20001 presence, answers NIP-44 encrypted kind:24200 control frames
//! (`host.deploy`, `host.undeploy`, `host.status`, `host.forget`), and runs
//! agents (`buzz-acp`) under systemd user units (Linux) or as supervised
//! children (macOS). See `docs/agent-hosts.md`.

pub mod control;
pub mod daemon;
pub mod env;
pub mod error;
pub mod pairing;
pub mod protocol;
pub mod service;
pub mod store;
pub mod supervisor;
pub mod tools;

use clap::Subcommand;

pub use error::{HostError, Result};

/// `buzz host` subcommands.
#[derive(Debug, Subcommand)]
pub enum HostCmd {
    /// Pair with your Buzz desktop if needed, then run as a background service
    Up {
        /// Relay to pair through (overrides the relay in the pairing URI)
        #[arg(long)]
        relay: Option<String>,
        /// Pairing URI from Buzz desktop (prompted for when omitted)
        #[arg(long)]
        uri: Option<String>,
        /// Machine name shown in Buzz desktop (default: hostname)
        #[arg(long)]
        name: Option<String>,
        /// Do not ask to confirm the pairing code on this machine
        #[arg(long)]
        yes: bool,
        /// Run the daemon in this terminal instead of installing a service
        #[arg(long)]
        foreground: bool,
    },
    /// Pair with your Buzz desktop using its pairing URI (pairing only)
    Pair {
        /// Pairing URI shown by Buzz desktop ("Add machine")
        uri: String,
        /// Relay to pair through (overrides the relay in the URI)
        #[arg(long)]
        relay: Option<String>,
        /// Machine name shown in Buzz desktop (default: hostname)
        #[arg(long)]
        name: Option<String>,
        /// Do not ask to confirm the pairing code on this machine
        #[arg(long)]
        yes: bool,
    },
    /// Run the host daemon in the foreground
    Run,
    /// Show pairing, agents and service state
    Status {
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Stop all agents, remove the service and wipe all local host state
    Forget {
        /// Do not ask for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Install the daemon as a login service (systemd user unit / LaunchAgent)
    InstallService,
    /// Remove the daemon login service
    UninstallService,
    /// Internal: exec buzz-acp for one deployed agent (used by unit files)
    #[command(hide = true)]
    ExecAgent {
        /// Agent pubkey (hex)
        agent_pubkey: String,
    },
}

fn init_logging() {
    let filter = daemon::log_filter(std::env::var("RUST_LOG").ok().as_deref());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

/// Run a `buzz host` subcommand.
pub async fn run(cmd: HostCmd) -> Result<()> {
    let paths = store::HostPaths::from_env()?;
    match cmd {
        HostCmd::Up {
            relay,
            uri,
            name,
            yes,
            foreground,
        } => {
            if store::load_owner(&paths)?.is_none() {
                let uri = match uri {
                    Some(u) => u,
                    None => {
                        println!("In Buzz desktop, open Settings → Machines → Add machine.");
                        pairing::prompt_line("Paste the pairing URI here: ")?
                    }
                };
                pair(&paths, &uri, relay.as_deref(), name, yes).await?;
            } else {
                println!("Already paired.");
            }
            if foreground {
                init_logging();
                daemon::run(paths).await
            } else {
                println!("{}", service::install().await?);
                println!("buzz host is running in the background. `buzz host status` shows it.");
                Ok(())
            }
        }
        HostCmd::Pair {
            uri,
            relay,
            name,
            yes,
        } => pair(&paths, &uri, relay.as_deref(), name, yes).await,
        HostCmd::Run => {
            init_logging();
            match daemon::run(paths).await {
                // Unpaired is a clean exit so service managers do not restart-loop.
                Err(HostError::NotPaired) => {
                    tracing::warn!("not paired; exiting");
                    Ok(())
                }
                other => other,
            }
        }
        HostCmd::Status { json } => status(&paths, json),
        HostCmd::Forget { yes } => {
            if !yes && !pairing::ask_yes_no("Stop all agents and forget this machine? [y/N]: ")? {
                return Err(HostError::Invalid("cancelled".into()));
            }
            forget(&paths).await
        }
        HostCmd::InstallService => {
            println!("{}", service::install().await?);
            Ok(())
        }
        HostCmd::UninstallService => service::uninstall().await,
        HostCmd::ExecAgent { agent_pubkey } => exec_agent(&paths, &agent_pubkey),
    }
}

async fn pair(
    paths: &store::HostPaths,
    uri: &str,
    relay: Option<&str>,
    name: Option<String>,
    yes: bool,
) -> Result<()> {
    if let Some(relay) = relay {
        protocol::validate_relay_url(relay)?;
    }
    let name = name.unwrap_or_else(tools::machine_name);
    let confirm = if yes {
        pairing::Confirm::Assume
    } else {
        pairing::Confirm::Prompt
    };
    let owner = pairing::pair(paths, uri, relay, &name, confirm).await?;
    println!(
        "Paired as {name:?} with owner {} on {}.",
        owner.owner_pubkey, owner.relay_url
    );
    Ok(())
}

fn status(paths: &store::HostPaths, json: bool) -> Result<()> {
    let key = store::load_host_key(paths)?;
    let owner = store::load_owner(paths)?;
    let agents = store::list_agents(paths)?;
    let snapshot: Option<protocol::HostStatus> = store::read_json(&paths.status_file())?;
    let snapshot_age = std::fs::metadata(paths.status_file())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());
    let states: std::collections::BTreeMap<String, protocol::AgentStatus> = snapshot
        .as_ref()
        .map(|s| {
            s.agents
                .iter()
                .map(|a| (a.agent_pubkey.clone(), a.clone()))
                .collect()
        })
        .unwrap_or_default();
    let (claude, tools) = tools::tools_status(&tools::agent_path());
    if json {
        let value = serde_json::json!({
            "paired": owner.is_some(),
            "host_pubkey": key.map(|k| k.public_key().to_hex()),
            "owner_pubkey": owner.as_ref().map(|o| &o.owner_pubkey),
            "relay_url": owner.as_ref().map(|o| &o.relay_url),
            "name": owner.as_ref().map(|o| &o.name),
            "agents": agents.iter().map(|a| serde_json::json!({
                "agent_pubkey": a,
                "state": states.get(a).map(|s| s.state),
            })).collect::<Vec<_>>(),
            "status_age_secs": snapshot_age,
            "claude": claude,
            "tools": tools,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    match (&owner, &key) {
        (Some(o), Some(k)) => {
            println!("Paired:  yes ({})", o.name);
            println!("Host:    {}", k.public_key().to_hex());
            println!("Owner:   {}", o.owner_pubkey);
            println!("Relay:   {}", o.relay_url);
        }
        _ => println!("Paired:  no (run `buzz host up`)"),
    }
    match snapshot_age {
        Some(age) => println!("Daemon:  last status {age}s ago"),
        None => println!("Daemon:  no status yet"),
    }
    println!(
        "Tools:   buzz-acp={} node={} claude-agent-acp={} claude={}",
        tools.buzz_acp, tools.node, tools.claude_agent_acp, claude.installed
    );
    println!("Agents:  {}", agents.len());
    for a in &agents {
        let state = states
            .get(a)
            .map(|s| format!("{:?}", s.state).to_lowercase())
            .unwrap_or_else(|| "unknown".into());
        println!("  {a}  {state}");
    }
    Ok(())
}

/// Local wipe: stop agents, remove the service, delete every host file.
async fn forget(paths: &store::HostPaths) -> Result<()> {
    if let Err(e) = service::uninstall().await {
        eprintln!("warning: could not remove the service: {e}");
    }
    let agents = store::list_agents(paths)?;
    match supervisor::Supervisor::detect(paths).await? {
        supervisor::Supervisor::Systemd(mut s) => {
            for a in &agents {
                s.remove(a).await?;
            }
        }
        supervisor::Supervisor::Process(mut p) => p.reap_orphans().await,
    }
    control::wipe_except_key(paths)?;
    store::remove_file(&paths.key_file())?;
    println!("Forgot this machine. Remove it in Buzz desktop too if it is still listed.");
    Ok(())
}

/// Replace this process with `buzz-acp` for one agent (unit-file entry point).
fn exec_agent(paths: &store::HostPaths, agent_pubkey: &str) -> Result<()> {
    if !protocol::is_pubkey_hex(agent_pubkey) {
        return Err(HostError::Invalid("agent pubkey must be hex".into()));
    }
    let rec = store::load_agent(paths, agent_pubkey)?
        .ok_or_else(|| HostError::Invalid(format!("agent {agent_pubkey} is not deployed")))?;
    let path = rec
        .env
        .get("PATH")
        .cloned()
        .unwrap_or_else(tools::agent_path);
    let acp = tools::which("buzz-acp", &path)
        .ok_or_else(|| HostError::Supervisor("buzz-acp is not installed on this host".into()))?;
    std::fs::create_dir_all(&rec.workdir).map_err(|e| HostError::io("create workdir", e))?;
    let mut cmd = std::process::Command::new(acp);
    cmd.envs(&rec.env).current_dir(&rec.workdir);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(HostError::Supervisor(format!(
            "exec buzz-acp failed: {err}"
        )))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|e| HostError::Supervisor(format!("run buzz-acp failed: {e}")))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
