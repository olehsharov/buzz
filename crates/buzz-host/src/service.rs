//! `buzz host install-service`: run the daemon at login/boot.
//!
//! * Linux: systemd user unit `buzz-host.service` plus `loginctl enable-linger`.
//! * macOS: `~/Library/LaunchAgents/xyz.buzz.host.plist`, `KeepAlive` on
//!   failure only, so a forgotten host (clean exit) is not restarted.

use std::path::PathBuf;

use crate::error::{HostError, Result};
use crate::store;
use crate::supervisor::{CommandRunner, SystemRunner};

/// systemd user unit name for the daemon.
pub const SYSTEMD_UNIT: &str = "buzz-host.service";
/// launchd label for the daemon.
pub const LAUNCHD_LABEL: &str = "xyz.buzz.host";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render the daemon's systemd user unit.
pub fn render_systemd_unit(exec_prefix: &[String], path: &str, home_env: &str) -> String {
    let exec: Vec<String> = exec_prefix
        .iter()
        .map(|a| {
            format!(
                "\"{}\"",
                a.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('%', "%%")
            )
        })
        .collect();
    format!(
        "[Unit]\n\
Description=Buzz agent host\n\
After=network-online.target\n\
Wants=network-online.target\n\
StartLimitIntervalSec=600\n\
StartLimitBurst=10\n\
\n\
[Service]\n\
Environment=\"PATH={path}\"\n\
{home_env}\
ExecStart={exec} run\n\
Restart=on-failure\n\
RestartSec=10\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        path = path.replace('%', "%%"),
        exec = exec.join(" "),
    )
}

/// Render the daemon's launchd plist.
pub fn render_plist(
    exec_prefix: &[String],
    path: &str,
    log: &str,
    home_env: Option<&str>,
) -> String {
    let mut args: String = exec_prefix
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml_escape(a)))
        .collect();
    args.push_str("    <string>run</string>\n");
    let extra_env = home_env
        .map(|h| {
            format!(
                "    <key>{}</key><string>{}</string>\n",
                store::HOST_HOME_ENV,
                xml_escape(h)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>{path}</string>
{extra_env}  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key><false/>
  </dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        path = xml_escape(path),
        log = xml_escape(log),
    )
}

fn plist_path() -> Result<PathBuf> {
    Ok(store::home_dir()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

fn custom_home() -> Option<String> {
    std::env::var(store::HOST_HOME_ENV)
        .ok()
        .filter(|v| !v.is_empty())
}

async fn must(runner: &SystemRunner, program: &str, args: &[&str]) -> Result<()> {
    let out = runner
        .run(program, args.iter().map(|s| s.to_string()).collect())
        .await?;
    if out.success {
        Ok(())
    } else {
        Err(HostError::Supervisor(format!(
            "{program} {} failed: {}",
            args.join(" "),
            out.stderr
        )))
    }
}

/// Install and start the daemon service for the current user.
pub async fn install() -> Result<String> {
    let exec = crate::tools::self_command();
    let path = crate::tools::agent_path();
    let runner = SystemRunner;
    if cfg!(target_os = "macos") {
        let plist = plist_path()?;
        let paths = store::HostPaths::from_env()?;
        store::ensure_private_dir(&paths.logs_dir())?;
        let log = paths.logs_dir().join("daemon.log");
        let content = render_plist(
            &exec,
            &path,
            &log.to_string_lossy(),
            custom_home().as_deref(),
        );
        if let Some(dir) = plist.parent() {
            std::fs::create_dir_all(dir).map_err(|e| HostError::io("create LaunchAgents", e))?;
        }
        std::fs::write(&plist, content).map_err(|e| HostError::io("write plist", e))?;
        let uid = current_uid().await?;
        let domain = format!("gui/{uid}");
        let target = format!("{domain}/{LAUNCHD_LABEL}");
        // bootout fails when not loaded; that is fine.
        let _ = runner
            .run("launchctl", vec!["bootout".into(), target.clone()])
            .await;
        must(
            &runner,
            "launchctl",
            &["bootstrap", &domain, &plist.to_string_lossy()],
        )
        .await?;
        must(&runner, "launchctl", &["kickstart", "-k", &target]).await?;
        Ok(format!("installed {}", plist.display()))
    } else {
        let dir = crate::supervisor::systemd::user_unit_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| HostError::io("create unit dir", e))?;
        let home_env = custom_home()
            .map(|h| {
                format!(
                    "Environment=\"{}={}\"\n",
                    store::HOST_HOME_ENV,
                    h.replace('%', "%%")
                )
            })
            .unwrap_or_default();
        let unit = dir.join(SYSTEMD_UNIT);
        std::fs::write(&unit, render_systemd_unit(&exec, &path, &home_env))
            .map_err(|e| HostError::io("write unit", e))?;
        let user = std::env::var("USER").unwrap_or_default();
        let linger = if user.is_empty() {
            Err(HostError::Supervisor("USER is not set".into()))
        } else {
            must(&runner, "loginctl", &["enable-linger", &user]).await
        };
        if let Err(e) = &linger {
            eprintln!("warning: could not enable linger ({e}); agents stop when you log out");
        }
        must(&runner, "systemctl", &["--user", "daemon-reload"]).await?;
        must(&runner, "systemctl", &["--user", "enable", SYSTEMD_UNIT]).await?;
        must(&runner, "systemctl", &["--user", "restart", SYSTEMD_UNIT]).await?;
        Ok(format!("installed {}", unit.display()))
    }
}

/// Stop and remove the daemon service. Missing services are fine.
pub async fn uninstall() -> Result<()> {
    let runner = SystemRunner;
    if cfg!(target_os = "macos") {
        let plist = plist_path()?;
        if let Ok(uid) = current_uid().await {
            let _ = runner
                .run(
                    "launchctl",
                    vec!["bootout".into(), format!("gui/{uid}/{LAUNCHD_LABEL}")],
                )
                .await;
        }
        store::remove_file(&plist)
    } else {
        let _ = runner
            .run(
                "systemctl",
                vec![
                    "--user".into(),
                    "disable".into(),
                    "--now".into(),
                    SYSTEMD_UNIT.into(),
                ],
            )
            .await;
        let unit = crate::supervisor::systemd::user_unit_dir()?.join(SYSTEMD_UNIT);
        store::remove_file(&unit)
    }
}

async fn current_uid() -> Result<String> {
    let out = SystemRunner.run("id", vec!["-u".into()]).await?;
    if out.success && !out.stdout.is_empty() {
        Ok(out.stdout)
    } else {
        Err(HostError::Supervisor("could not determine uid".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_keeps_alive_only_on_failure() {
        let p = render_plist(
            &["/Users/u/.local/bin/buzz".into(), "host".into()],
            "/usr/bin",
            "/tmp/l.log",
            None,
        );
        assert!(p.contains("<key>SuccessfulExit</key><false/>"));
        assert!(p.contains("<string>/Users/u/.local/bin/buzz</string>\n    <string>host</string>\n    <string>run</string>"));
        assert!(p.contains(LAUNCHD_LABEL));
    }

    #[test]
    fn systemd_unit_restarts_on_failure() {
        let u = render_systemd_unit(
            &["/home/u/.local/bin/buzz".into(), "host".into()],
            "/usr/bin",
            "",
        );
        assert!(u.contains("Restart=on-failure"));
        assert!(u.contains("ExecStart=\"/home/u/.local/bin/buzz\" \"host\" run"));
    }
}
