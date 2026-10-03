use super::{command_search_dirs, common_binary_paths, find_nvm_default_bin, login_shell_path};
use std::path::{Path, PathBuf};

/// Ordered directories searched for Buzz's own sidecars (`buzz-acp`,
/// `buzz-agent`, `buzz-dev-mcp`, `buzz`, `git-credential-nostr`,
/// `buzz-backend-kubernetes`) before falling back to PATH-style discovery.
///
/// Release builds search only the running executable's directory
/// (`Contents/MacOS/` in a `.app` bundle), where Tauri places the sidecars it
/// bundled with this exact build. The compile-time workspace root and the
/// process cwd are meaningful only to a developer build: a release `.app`
/// built on a developer machine would otherwise launch whatever stale binaries
/// sit in that checkout's `target/`, silently diverging from the bundle.
///
/// Debug builds (`just dev`) keep the workspace lookup first so freshly built
/// `target/debug` sidecars win over the stubs/copies next to the dev
/// executable, preferring `target/debug` over possibly stale `target/release`.
pub(crate) fn sidecar_search_dirs(
    is_debug: bool,
    exe_dir: Option<PathBuf>,
    workspace_root: &Path,
    current_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if is_debug {
        let profile_dirs = |root: &Path| [root.join("target/debug"), root.join("target/release")];
        dirs.extend(profile_dirs(workspace_root));
        if let Some(current_dir) = current_dir {
            dirs.extend(profile_dirs(&current_dir));
        }
    }
    dirs.extend(exe_dir);
    merge_command_discovery_dirs([dirs])
}

/// First `dir/file_name` in `dirs` (in order) accepted by `is_candidate`.
pub(crate) fn first_candidate_in(
    dirs: &[PathBuf],
    file_name: &str,
    is_candidate: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(file_name))
        .find(|candidate| is_candidate(candidate))
}

pub(crate) fn merge_command_discovery_dirs(
    sources: impl IntoIterator<Item = Vec<PathBuf>>,
) -> Vec<PathBuf> {
    sources
        .into_iter()
        .flatten()
        .fold(Vec::new(), |mut unique, dir| {
            if !unique.contains(&dir) {
                unique.push(dir);
            }
            unique
        })
}

pub(crate) fn command_discovery_dirs() -> Vec<PathBuf> {
    let path_dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let login_shell_dirs = login_shell_path()
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let nvm_dirs = dirs::home_dir()
        .and_then(|home| find_nvm_default_bin(&home))
        .into_iter()
        .collect();

    merge_command_discovery_dirs([
        command_search_dirs(),
        path_dirs,
        common_binary_paths().to_vec(),
        login_shell_dirs,
        nvm_dirs,
    ])
}

#[cfg(test)]
#[path = "command_search_tests.rs"]
mod tests;
