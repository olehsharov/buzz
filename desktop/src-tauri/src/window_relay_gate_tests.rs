//! Static gate: the chat command files a community window can reach must
//! never resolve the app-wide workspace relay. Every relay call in them takes
//! an explicit base threaded from the invoking window's `WindowRelay`.
//!
//! A new `query_relay(&state, …)` (or any other implicit-workspace helper) in
//! one of these files fails this test with the file and line. Test modules
//! (`#[cfg(test)]` and below, and `*_tests.rs`) are exempt: fixtures may set
//! the override deliberately.

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
    let production = source.split("\n#[cfg(test)]").next().unwrap_or(source);
    production
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
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
}
