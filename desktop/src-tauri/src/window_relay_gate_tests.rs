//! Static gate: the chat command files a community window can reach must
//! never resolve the app-wide workspace relay. Every relay call in them takes
//! an explicit base threaded from the invoking window's `WindowRelay`.
//!
//! A new `query_relay(&state, …)` (or any other implicit-workspace helper) in
//! one of these files fails this test with the file and line. Test modules
//! (`#[cfg(test)] mod …` and below, and `*_tests.rs`) are exempt: fixtures may
//! set the override deliberately.
//!
//! Two sanctioned uses stay allowed, each visible at the call site:
//! - the unassigned-record fallback of `effective_agent_relay_url(record,
//!   workspace)` — an agent's own community wins; the workspace only names
//!   the community of a legacy record that has none;
//! - a line (or the line after a comment) carrying a `window-relay:` marker
//!   that states why the main window's community is the right one there
//!   (main-window-only flows such as identity pairing and archive sync).

/// The MVP chat command files a community window invokes.
const SCOPED_FILES: &[(&str, &str)] = &[
    ("commands/messages.rs", include_str!("commands/messages.rs")),
    (
        "commands/messages/forum.rs",
        include_str!("commands/messages/forum.rs"),
    ),
    (
        "commands/messages/event_batch.rs",
        include_str!("commands/messages/event_batch.rs"),
    ),
    (
        "commands/messages/thread_ref.rs",
        include_str!("commands/messages/thread_ref.rs"),
    ),
    ("commands/channels.rs", include_str!("commands/channels.rs")),
    (
        "commands/channels/fetch.rs",
        include_str!("commands/channels/fetch.rs"),
    ),
    ("commands/media.rs", include_str!("commands/media.rs")),
    (
        "commands/media_raw.rs",
        include_str!("commands/media_raw.rs"),
    ),
    (
        "commands/media_download.rs",
        include_str!("commands/media_download.rs"),
    ),
    (
        "commands/media_fetch_cancellation.rs",
        include_str!("commands/media_fetch_cancellation.rs"),
    ),
    ("commands/dms.rs", include_str!("commands/dms.rs")),
    (
        "commands/relay_members.rs",
        include_str!("commands/relay_members.rs"),
    ),
    ("commands/profile.rs", include_str!("commands/profile.rs")),
    ("commands/social.rs", include_str!("commands/social.rs")),
    ("commands/canvas.rs", include_str!("commands/canvas.rs")),
    ("commands/engrams.rs", include_str!("commands/engrams.rs")),
    (
        "commands/channel_window.rs",
        include_str!("commands/channel_window.rs"),
    ),
    (
        "commands/channel_reconnect_repair.rs",
        include_str!("commands/channel_reconnect_repair.rs"),
    ),
    ("commands/identity.rs", include_str!("commands/identity.rs")),
    ("unread_catch_up.rs", include_str!("unread_catch_up.rs")),
    // Agents, definitions, teams, machines, projects, and workflows run for
    // the invoking window's community too.
    (
        "commands/workflows.rs",
        include_str!("commands/workflows.rs"),
    ),
    (
        "commands/project_git_exec.rs",
        include_str!("commands/project_git_exec.rs"),
    ),
    ("commands/hosts.rs", include_str!("commands/hosts.rs")),
    ("commands/bestie.rs", include_str!("commands/bestie.rs")),
    ("archive/mod.rs", include_str!("archive/mod.rs")),
    ("persona_catalog.rs", include_str!("persona_catalog.rs")),
    ("team_catalog.rs", include_str!("team_catalog.rs")),
    ("commands/agents.rs", include_str!("commands/agents.rs")),
    (
        "commands/agents_pending.rs",
        include_str!("commands/agents_pending.rs"),
    ),
    (
        "commands/agents_profile.rs",
        include_str!("commands/agents_profile.rs"),
    ),
    (
        "commands/agents_deploy.rs",
        include_str!("commands/agents_deploy.rs"),
    ),
    (
        "commands/agent_config.rs",
        include_str!("commands/agent_config.rs"),
    ),
    (
        "commands/agent_models.rs",
        include_str!("commands/agent_models.rs"),
    ),
    // Agent defaults are per community: the settings edit the window's.
    (
        "commands/global_agent_config.rs",
        include_str!("commands/global_agent_config.rs"),
    ),
    (
        "commands/agent_models_update.rs",
        include_str!("commands/agent_models_update.rs"),
    ),
    (
        "commands/agent_discovery/relay_directory.rs",
        include_str!("commands/agent_discovery/relay_directory.rs"),
    ),
    (
        "commands/identity_archive.rs",
        include_str!("commands/identity_archive.rs"),
    ),
    (
        "commands/personas/mod.rs",
        include_str!("commands/personas/mod.rs"),
    ),
    (
        "commands/personas/create.rs",
        include_str!("commands/personas/create.rs"),
    ),
    (
        "commands/personas/update.rs",
        include_str!("commands/personas/update.rs"),
    ),
    (
        "commands/personas/sharing.rs",
        include_str!("commands/personas/sharing.rs"),
    ),
    (
        "commands/personas/pending.rs",
        include_str!("commands/personas/pending.rs"),
    ),
    (
        "commands/personas/card.rs",
        include_str!("commands/personas/card.rs"),
    ),
    (
        "commands/personas/snapshot.rs",
        include_str!("commands/personas/snapshot.rs"),
    ),
    (
        "commands/personas/snapshot/import.rs",
        include_str!("commands/personas/snapshot/import.rs"),
    ),
    (
        "commands/personas/inbound.rs",
        include_str!("commands/personas/inbound.rs"),
    ),
    (
        "commands/teams/mod.rs",
        include_str!("commands/teams/mod.rs"),
    ),
    (
        "commands/teams/pending.rs",
        include_str!("commands/teams/pending.rs"),
    ),
    (
        "commands/teams/sharing.rs",
        include_str!("commands/teams/sharing.rs"),
    ),
    (
        "commands/teams/adopt.rs",
        include_str!("commands/teams/adopt.rs"),
    ),
    (
        "commands/teams/adopt/apply.rs",
        include_str!("commands/teams/adopt/apply.rs"),
    ),
    (
        "commands/team_snapshot.rs",
        include_str!("commands/team_snapshot.rs"),
    ),
];

/// Helpers that silently read the app-wide workspace relay.
const FORBIDDEN: &[&str] = &[
    "relay_ws_url_with_override",
    "relay_api_base_url_with_override",
    "workspace_relay_override",
    "relay_url_override",
    "capture_relay_target",
    "query_relay(",
    "submit_event(",
    "submit_event_with_keys(",
    "submit_signed_event_with_keys(",
    "get_relay_json(",
    "fetch_relay_self(",
    "fetch_archived_pubkeys(",
];

/// `query_relay(` must not match `query_relay_at(` etc.: require that the
/// character before the needle is not an identifier character.
fn violations(source: &str, needle: &str) -> Vec<usize> {
    let production = source
        .split("\n#[cfg(test)]\nmod ")
        .next()
        .unwrap_or(source);
    let lines: Vec<&str> = production.lines().collect();
    let sanctioned = |index: usize| {
        let window = &lines[index.saturating_sub(3)..=index];
        window
            .iter()
            .any(|line| line.contains("effective_agent_relay_url("))
            || lines[index.saturating_sub(1)..=index]
                .iter()
                .any(|line| line.contains("window-relay:"))
            || test_only_item(&lines, index)
    };
    // Imports name a helper; only calls read the relay.
    let mut in_use = false;
    let imports: Vec<bool> = lines
        .iter()
        .map(|line| {
            let starts = line.starts_with("use ") || line.starts_with("pub use ");
            let is_import = in_use || starts;
            in_use = is_import && !line.contains(';');
            is_import
        })
        .collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(index, _)| !imports[*index])
        .filter(|(index, _)| !sanctioned(*index))
        .filter(|(_, line)| {
            line.match_indices(needle).any(|(index, _)| {
                line[..index]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            })
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// Whether line `index` sits inside an item annotated `#[cfg(test)]` (a
/// test-only helper outside a test module): walk up to the enclosing
/// top-level item and check its attributes.
fn test_only_item(lines: &[&str], index: usize) -> bool {
    let Some(start) = (0..=index).rev().find(|&i| {
        let line = lines[i];
        !line.starts_with(' ')
            && !line.starts_with('}')
            && !line.starts_with(')')
            && !line.trim().is_empty()
            && !line.starts_with("#[")
            && !line.starts_with("//")
    }) else {
        return false;
    };
    (0..start)
        .rev()
        .take_while(|&i| lines[i].starts_with("#[") || lines[i].starts_with("///"))
        .any(|i| lines[i].starts_with("#[cfg(test)]"))
}

#[test]
fn scoped_command_files_never_read_the_workspace_relay() {
    let mut found = Vec::new();
    for (file, source) in SCOPED_FILES {
        for needle in FORBIDDEN {
            for line in violations(source, needle) {
                found.push(format!("src/{file}:{line} uses `{needle}`"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "window-scoped command files must take the relay from `WindowRelay`:\n{}",
        found.join("\n")
    );
}

#[test]
fn gate_detects_a_regression() {
    let regressed = "fn x() {\n    query_relay(&state, &[f]).await;\n}\n#[cfg(test)]\nquery_relay(";
    assert_eq!(violations(regressed, "query_relay("), vec![2]);
    assert!(violations("query_relay_at(&state, base, &[f])", "query_relay(").is_empty());
    assert!(violations("// query_relay(&state)", "query_relay(").is_empty());
    // The unassigned-record fallback and marked main-window flows are allowed;
    // the same read anywhere else is not.
    let fallback = "let r = effective_agent_relay_url(\n    &record.relay_url,\n    &relay_ws_url_with_override(state),\n);";
    assert!(violations(fallback, "relay_ws_url_with_override").is_empty());
    let marked = "// window-relay: identity pairing is main-window settings work.\nlet r = relay_ws_url_with_override(&state);";
    assert!(violations(marked, "relay_ws_url_with_override").is_empty());
    let bare = "fn f() {\n    let r = relay_ws_url_with_override(&state);\n}";
    assert_eq!(violations(bare, "relay_ws_url_with_override"), vec![2]);
    let test_only = "#[cfg(test)]\nfn helper() {\n    query_relay(&state, &[f]);\n}\nfn prod() {\n    query_relay(&state, &[f]);\n}";
    assert_eq!(violations(test_only, "query_relay("), vec![6]);
}
