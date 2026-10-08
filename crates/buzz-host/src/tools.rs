//! Tool discovery on the host (`buzz-acp`, `node`, `claude`, ...).
//!
//! Service managers start the daemon with a minimal `PATH`, so lookups use a
//! search path that also covers the usual per-user install locations.

use std::path::{Path, PathBuf};

use crate::protocol::{ClaudeStatus, ToolsStatus};

/// The search `PATH` for agents and tool checks: the user's install dirs,
/// the directory of the running binary, then the inherited `PATH`, then
/// system defaults. Duplicates are removed, order is preserved.
pub fn agent_path() -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(home) = crate::store::home_dir() {
        for rel in [".local/bin", ".npm-global/bin", ".cargo/bin", ".bun/bin"] {
            dirs.push(home.join(rel));
        }
    }
    if let Some(dir) = self_dir() {
        dirs.push(dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    for sys in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        dirs.push(PathBuf::from(sys));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    std::env::join_paths(dirs)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into())
}

/// Directory containing the invoked binary, without resolving symlinks.
fn self_dir() -> Option<PathBuf> {
    let argv0 = std::env::args_os().next()?;
    let p = PathBuf::from(argv0);
    if p.components().count() > 1 {
        std::path::absolute(&p)
            .ok()?
            .parent()
            .map(Path::to_path_buf)
    } else {
        None
    }
}

/// Find `name` on `path` (a `PATH`-style string). Names containing a `/`
/// are checked as-is.
pub fn which(name: &str, path: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_executable(&p).then_some(p);
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Current tool availability.
pub fn tools_status(path: &str) -> (ClaudeStatus, ToolsStatus) {
    let claude = ClaudeStatus {
        installed: which("claude", path).is_some(),
        // Checking Claude's login would mean running it; report unknown.
        auth_ok: None,
    };
    let tools = ToolsStatus {
        node: which("node", path).is_some(),
        claude_agent_acp: which("claude-agent-acp", path).is_some(),
        buzz_acp: which("buzz-acp", path).is_some(),
    };
    (claude, tools)
}

/// The command prefix that re-invokes `buzz host` (for unit files and
/// launchd plists). Uses `argv[0]` without resolving symlinks, because the
/// static Sprig binary dispatches on the name it was invoked as.
pub fn self_command() -> Vec<String> {
    let argv0 = std::env::args().next().unwrap_or_default();
    let p = PathBuf::from(&argv0);
    let resolved = if p.components().count() > 1 {
        std::path::absolute(&p).ok()
    } else {
        which(&argv0, &agent_path())
    };
    let exe = resolved
        .or_else(|| std::env::current_exe().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "buzz".into());
    let personality = Path::new(&exe)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if personality == "buzz-host" {
        vec![exe]
    } else {
        vec![exe, "host".into()]
    }
}

/// A best-effort machine name: `$BUZZ_HOST_NAME`, else `hostname`, minus a
/// trailing `.local`.
pub fn machine_name() -> String {
    if let Ok(name) = std::env::var("BUZZ_HOST_NAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    let raw = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let name = raw.strip_suffix(".local").unwrap_or(&raw).to_string();
    if name.is_empty() {
        "buzz-host".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn which_finds_only_executables() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").expect("write");
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "").expect("write");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path = dir.path().to_string_lossy().into_owned();
        assert_eq!(which("tool", &path), Some(exe));
        assert_eq!(which("plain", &path), None);
        assert_eq!(which("missing", &path), None);
    }
}
