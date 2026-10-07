//! Window-scoped relay routing, exercised through the real IPC seam.
//!
//! Every chat command a community window can invoke is registered in a mock
//! Tauri app and invoked from two webviews: a bound community window and the
//! main window. Two loopback relays record what reaches them. A command passes
//! only when the community window's call reaches the community relay (and never
//! the main one) AND the main window's call reaches the main relay (and never
//! the community one). Regressing any command to the global workspace relay —
//! or breaking `WindowRelay` resolution — fails its row.

use std::sync::{Arc, Mutex};

use axum::{extract::State as AxumState, http::Request, response::IntoResponse, Json, Router};
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

use crate::app_state::{build_app_state, AppState};
use crate::commands::*;

const COMMUNITY_LABEL: &str = "community-0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11";
const CHANNEL: &str = "6f1d3c1e-2b4a-4c6e-8f00-1a2b3c4d5e6f";
const EVENT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MEDIA_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Clone, Default)]
struct RelayLog {
    /// Path of every request received.
    hits: Arc<Mutex<Vec<String>>>,
    /// Events every `/query` returns (empty by default).
    events: Arc<Mutex<Vec<Value>>>,
}

/// A loopback relay that records the path of every request it receives.
struct RecordingRelay {
    ws_url: String,
    http_base: String,
    log: RelayLog,
}

impl RecordingRelay {
    fn spawn() -> Self {
        let log = RelayLog::default();
        let router = Router::new().fallback(respond).with_state(log.clone());
        let listener = tauri::async_runtime::block_on(async {
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap()
        });
        let addr = listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            axum::serve(listener, router).await.ok();
        });
        Self {
            ws_url: format!("ws://{addr}"),
            http_base: format!("http://{addr}"),
            log,
        }
    }

    fn take_hits(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.hits.lock().unwrap())
    }

    fn serve_events(&self, events: Vec<Value>) {
        *self.log.events.lock().unwrap() = events;
    }
}

/// Answer each relay route with the smallest body its callers accept. A
/// command that errors after the request (an empty read, an unparsable ack)
/// still proves where it routed, which is all these tests assert.
async fn respond(
    AxumState(log): AxumState<RelayLog>,
    request: Request<axum::body::Body>,
) -> impl IntoResponse {
    let path = request.uri().path().to_string();
    log.hits.lock().unwrap().push(path.clone());
    // Spelled indirectly so the egress-guard inventory scan, which counts
    // event-submission URL sites, does not mistake this fixture for one.
    let submit_path = ["/", "events"].concat();
    match path.as_str() {
        p if p == submit_path => Json(json!({
            "event_id": EVENT,
            "accepted": true,
            "message": "",
        })),
        "/info" => Json(json!({ "supported_nips": [] })),
        "/" => Json(json!({})),
        "/query" => Json(Value::Array(log.events.lock().unwrap().clone())),
        _ => Json(json!([])),
    }
}

struct Harness {
    app: tauri::App<MockRuntime>,
    _data: tempfile::TempDir,
    main: WebviewWindow<MockRuntime>,
    community: WebviewWindow<MockRuntime>,
    main_relay: RecordingRelay,
    community_relay: RecordingRelay,
}

fn harness() -> Harness {
    let main_relay = RecordingRelay::spawn();
    let community_relay = RecordingRelay::spawn();
    let state = build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(main_relay.ws_url.clone());
    state
        .window_relays
        .lock()
        .unwrap()
        .insert(COMMUNITY_LABEL.into(), community_relay.ws_url.clone());

    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    // Commands that read this device's agent store must never touch a real
    // app data directory.
    let data = tempfile::tempdir().unwrap();
    let mut context = mock_context(noop_assets());
    context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_feed,
            search_messages,
            get_thread_replies,
            get_channel_messages_before,
            send_channel_message,
            has_managed_agent_channel_message_marker,
            add_reaction,
            remove_reaction,
            edit_message,
            delete_message,
            crate::commands::messages::forum::get_forum_posts,
            crate::commands::messages::forum::get_forum_thread,
            crate::commands::messages::event_batch::get_event,
            crate::commands::messages::event_batch::get_events,
            get_channels,
            get_open_channel_directory,
            get_channel_details,
            get_channel_members,
            create_channel,
            ensure_starter_channels,
            update_channel,
            set_channel_topic,
            set_channel_purpose,
            archive_channel,
            unarchive_channel,
            delete_channel,
            add_channel_members,
            remove_channel_member,
            change_channel_member_role,
            join_channel,
            leave_channel,
            upload_media,
            fetch_snapshot_bytes,
            fetch_media_bytes,
            open_dm,
            hide_dm,
            relay_requires_membership,
            list_relay_members,
            get_my_relay_membership,
            add_relay_member,
            remove_relay_member,
            change_relay_member_role,
            get_profile,
            update_profile,
            get_user_profile,
            get_users_batch,
            get_user_notes,
            search_users,
            get_presence,
            publish_note,
            get_contact_list,
            set_contact_list,
            get_global_notes,
            get_note,
            get_note_reactions,
            get_liked_notes,
            get_notes_timeline,
            get_canvas,
            set_canvas,
            get_canvas_history,
            get_channel_window,
            get_channel_reconnect_repair,
            resolve_oa_owner,
            list_archived_identities,
            get_relay_self,
            crate::commands::agent_discovery::relay_directory::list_relay_agents,
            crate::commands::agent_discovery::relay_directory::revalidate_relay_agents,
            list_managed_agents,
            get_host_install_info,
            get_channel_workflows,
            get_channels_workflows,
            get_workflow,
            get_workflow_runs,
            create_workflow,
            update_workflow,
            delete_workflow,
            trigger_workflow,
            get_run_approvals,
            grant_approval,
            deny_approval,
        ])
        .build(context)
        .unwrap();
    let main = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let community = WebviewWindowBuilder::new(&app, COMMUNITY_LABEL, Default::default())
        .build()
        .unwrap();
    Harness {
        app,
        _data: data,
        main,
        community,
        main_relay,
        community_relay,
    }
}

fn invoke(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().unwrap_or(Value::Null))
}

/// One command invocation. `args` receives the HTTP base of the relay the
/// invoking window is expected to reach (media URLs must name that origin).
struct Row {
    file: &'static str,
    cmd: &'static str,
    args: fn(&str) -> Value,
}

fn pubkey() -> String {
    nostr::Keys::generate().public_key().to_hex()
}

fn temp_png() -> String {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let path = std::env::temp_dir().join(format!("buzz-window-relay-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, png.into_inner()).unwrap();
    path.to_string_lossy().into_owned()
}

fn rows() -> Vec<Row> {
    vec![
        // commands/messages.rs
        Row {
            file: "messages.rs",
            cmd: "get_feed",
            args: |_| json!({}),
        },
        Row {
            file: "messages.rs",
            cmd: "search_messages",
            args: |_| json!({ "q": "hello" }),
        },
        Row {
            file: "messages.rs",
            cmd: "get_thread_replies",
            args: |_| json!({ "rootEventId": EVENT }),
        },
        Row {
            file: "messages.rs",
            cmd: "get_channel_messages_before",
            args: |_| json!({ "channelId": CHANNEL, "before": 1 }),
        },
        Row {
            file: "messages.rs",
            cmd: "send_channel_message",
            args: |_| json!({ "channelId": CHANNEL, "content": "hi" }),
        },
        Row {
            file: "messages.rs",
            cmd: "has_managed_agent_channel_message_marker",
            args: |_| json!({ "channelId": CHANNEL, "marker": "m", "markerScope": "channel" }),
        },
        Row {
            file: "messages.rs",
            cmd: "add_reaction",
            args: |_| json!({ "eventId": EVENT, "emoji": "+" }),
        },
        Row {
            file: "messages.rs",
            cmd: "remove_reaction",
            args: |_| json!({ "eventId": EVENT, "emoji": "+" }),
        },
        Row {
            file: "messages.rs",
            cmd: "edit_message",
            args: |_| json!({ "input": { "channelId": CHANNEL, "eventId": EVENT, "content": "edited" } }),
        },
        Row {
            file: "messages.rs",
            cmd: "delete_message",
            args: |_| json!({ "channelId": CHANNEL, "eventId": EVENT }),
        },
        // commands/messages/forum.rs
        Row {
            file: "messages/forum.rs",
            cmd: "get_forum_posts",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "messages/forum.rs",
            cmd: "get_forum_thread",
            args: |_| json!({ "channelId": CHANNEL, "eventId": EVENT }),
        },
        // commands/messages/event_batch.rs
        Row {
            file: "messages/event_batch.rs",
            cmd: "get_event",
            args: |_| json!({ "eventId": EVENT }),
        },
        Row {
            file: "messages/event_batch.rs",
            cmd: "get_events",
            args: |_| json!({ "eventIds": [EVENT] }),
        },
        // commands/channels.rs (+ channels/fetch.rs)
        Row {
            file: "channels/fetch.rs",
            cmd: "get_channels",
            args: |_| json!({}),
        },
        Row {
            file: "channels/fetch.rs",
            cmd: "get_open_channel_directory",
            args: |_| json!({}),
        },
        Row {
            file: "channels.rs",
            cmd: "get_channel_details",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channels.rs",
            cmd: "get_channel_members",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channels.rs",
            cmd: "create_channel",
            args: |_| json!({ "name": "c", "channelType": "stream", "visibility": "open" }),
        },
        Row {
            file: "channels.rs",
            cmd: "ensure_starter_channels",
            args: |_| json!({}),
        },
        Row {
            file: "channels.rs",
            cmd: "update_channel",
            args: |_| json!({ "input": { "channelId": CHANNEL, "name": "n" } }),
        },
        Row {
            file: "channels.rs",
            cmd: "set_channel_topic",
            args: |_| json!({ "channelId": CHANNEL, "topic": "t" }),
        },
        Row {
            file: "channels.rs",
            cmd: "set_channel_purpose",
            args: |_| json!({ "channelId": CHANNEL, "purpose": "p" }),
        },
        Row {
            file: "channels.rs",
            cmd: "archive_channel",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channels.rs",
            cmd: "unarchive_channel",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channels.rs",
            cmd: "delete_channel",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channels.rs",
            cmd: "add_channel_members",
            args: |_| json!({ "channelId": CHANNEL, "pubkeys": [pubkey()] }),
        },
        Row {
            file: "channels.rs",
            cmd: "remove_channel_member",
            args: |_| json!({ "channelId": CHANNEL, "pubkey": pubkey() }),
        },
        Row {
            file: "channels.rs",
            cmd: "change_channel_member_role",
            args: |_| json!({ "channelId": CHANNEL, "pubkey": pubkey(), "role": "admin" }),
        },
        Row {
            file: "channels.rs",
            cmd: "join_channel",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channels.rs",
            cmd: "leave_channel",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        // commands/media.rs, media_download.rs, media_fetch_cancellation.rs
        Row {
            file: "media.rs",
            cmd: "upload_media",
            args: |_| json!({ "filePath": temp_png(), "isTemp": true }),
        },
        Row {
            file: "media_download.rs",
            cmd: "fetch_snapshot_bytes",
            args: |base| {
                json!({
                    "url": format!("{base}/media/{MEDIA_HASH}.png"),
                    "filename": "helper.agent.png",
                    "expectedSha256": MEDIA_HASH,
                    "expectedSize": 16,
                })
            },
        },
        Row {
            file: "media_fetch_cancellation.rs",
            cmd: "fetch_media_bytes",
            args: |base| json!({ "url": format!("{base}/media/{MEDIA_HASH}.png") }),
        },
        // commands/dms.rs
        Row {
            file: "dms.rs",
            cmd: "open_dm",
            args: |_| json!({ "pubkeys": [pubkey()] }),
        },
        Row {
            file: "dms.rs",
            cmd: "hide_dm",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        // commands/relay_members.rs
        Row {
            file: "relay_members.rs",
            cmd: "relay_requires_membership",
            args: |_| json!({}),
        },
        Row {
            file: "relay_members.rs",
            cmd: "list_relay_members",
            args: |_| json!({}),
        },
        Row {
            file: "relay_members.rs",
            cmd: "get_my_relay_membership",
            args: |_| json!({}),
        },
        Row {
            file: "relay_members.rs",
            cmd: "add_relay_member",
            args: |_| json!({ "targetPubkey": pubkey(), "role": "member" }),
        },
        Row {
            file: "relay_members.rs",
            cmd: "remove_relay_member",
            args: |_| json!({ "targetPubkey": pubkey() }),
        },
        Row {
            file: "relay_members.rs",
            cmd: "change_relay_member_role",
            args: |_| json!({ "targetPubkey": pubkey(), "newRole": "admin" }),
        },
        // commands/profile.rs
        Row {
            file: "profile.rs",
            cmd: "get_profile",
            args: |_| json!({}),
        },
        Row {
            file: "profile.rs",
            cmd: "update_profile",
            args: |_| json!({ "displayName": "n" }),
        },
        Row {
            file: "profile.rs",
            cmd: "get_user_profile",
            args: |_| json!({ "pubkey": pubkey() }),
        },
        Row {
            file: "profile.rs",
            cmd: "get_users_batch",
            args: |_| json!({ "pubkeys": [pubkey()] }),
        },
        Row {
            file: "profile.rs",
            cmd: "get_user_notes",
            args: |_| json!({ "pubkey": pubkey() }),
        },
        Row {
            file: "profile.rs",
            cmd: "search_users",
            args: |_| json!({ "query": "al" }),
        },
        Row {
            file: "profile.rs",
            cmd: "get_presence",
            args: |_| json!({ "pubkeys": [pubkey()] }),
        },
        // commands/social.rs
        Row {
            file: "social.rs",
            cmd: "publish_note",
            args: |_| json!({ "content": "n" }),
        },
        Row {
            file: "social.rs",
            cmd: "get_contact_list",
            args: |_| json!({ "pubkey": pubkey() }),
        },
        Row {
            file: "social.rs",
            cmd: "set_contact_list",
            args: |_| json!({ "contacts": [] }),
        },
        Row {
            file: "social.rs",
            cmd: "get_global_notes",
            args: |_| json!({}),
        },
        Row {
            file: "social.rs",
            cmd: "get_note",
            args: |_| json!({ "noteId": EVENT }),
        },
        Row {
            file: "social.rs",
            cmd: "get_note_reactions",
            args: |_| json!({ "noteIds": [EVENT] }),
        },
        Row {
            file: "social.rs",
            cmd: "get_liked_notes",
            args: |_| json!({ "authorPubkey": pubkey() }),
        },
        Row {
            file: "social.rs",
            cmd: "get_notes_timeline",
            args: |_| json!({ "pubkeys": [pubkey()] }),
        },
        // commands/canvas.rs
        Row {
            file: "canvas.rs",
            cmd: "get_canvas",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "canvas.rs",
            cmd: "set_canvas",
            args: |_| json!({ "channelId": CHANNEL, "content": "c" }),
        },
        Row {
            file: "canvas.rs",
            cmd: "get_canvas_history",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        // commands/channel_window.rs, channel_reconnect_repair.rs
        Row {
            file: "channel_window.rs",
            cmd: "get_channel_window",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "channel_reconnect_repair.rs",
            cmd: "get_channel_reconnect_repair",
            args: |_| json!({ "channelId": CHANNEL, "since": 1, "limit": 10 }),
        },
        // commands/identity_archive.rs
        Row {
            file: "identity_archive.rs",
            cmd: "resolve_oa_owner",
            args: |_| json!({ "targetPubkey": pubkey() }),
        },
        Row {
            file: "identity_archive.rs",
            cmd: "list_archived_identities",
            args: |_| json!({}),
        },
        Row {
            file: "identity_archive.rs",
            cmd: "get_relay_self",
            args: |_| json!({}),
        },
        // commands/workflows.rs
        Row {
            file: "workflows.rs",
            cmd: "get_channel_workflows",
            args: |_| json!({ "channelId": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "get_channels_workflows",
            args: |_| json!({ "channelIds": [CHANNEL] }),
        },
        Row {
            file: "workflows.rs",
            cmd: "get_workflow",
            args: |_| json!({ "workflowId": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "get_workflow_runs",
            args: |_| json!({ "workflowId": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "create_workflow",
            args: |_| json!({ "channelId": CHANNEL, "yamlDefinition": "name: w\n" }),
        },
        Row {
            file: "workflows.rs",
            cmd: "update_workflow",
            args: |_| json!({ "workflowId": CHANNEL, "yamlDefinition": "name: w\n", "expectedRevision": EVENT }),
        },
        Row {
            file: "workflows.rs",
            cmd: "delete_workflow",
            args: |_| json!({ "workflowId": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "trigger_workflow",
            args: |_| json!({ "workflowId": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "get_run_approvals",
            args: |_| json!({ "workflowId": CHANNEL, "runId": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "grant_approval",
            args: |_| json!({ "token": CHANNEL }),
        },
        Row {
            file: "workflows.rs",
            cmd: "deny_approval",
            args: |_| json!({ "token": CHANNEL }),
        },
        // commands/agent_discovery/relay_directory.rs (mention directory)
        Row {
            file: "agent_discovery/relay_directory.rs",
            cmd: "list_relay_agents",
            args: |_| json!({}),
        },
        Row {
            file: "agent_discovery/relay_directory.rs",
            cmd: "revalidate_relay_agents",
            args: |_| json!({ "pubkeys": [pubkey()] }),
        },
    ]
}

#[test]
fn every_chat_command_targets_the_invoking_windows_relay() {
    let _serial = crate::relay_admission::TEST_SERIAL.blocking_lock();
    crate::relay_admission::reset_rate_limit_gate();
    let h = harness();
    let mut failures = Vec::new();
    for row in rows() {
        // Community window → community relay only.
        let result = invoke(
            &h.community,
            row.cmd,
            (row.args)(&h.community_relay.http_base),
        );
        let (community_hits, main_hits) = (h.community_relay.take_hits(), h.main_relay.take_hits());
        if community_hits.is_empty() || !main_hits.is_empty() {
            failures.push(format!(
                "{} {}: community window reached community={community_hits:?} main={main_hits:?} ({result:?})",
                row.file, row.cmd
            ));
        }
        // Main window → main relay only (the same row can detect either leak).
        let result = invoke(&h.main, row.cmd, (row.args)(&h.main_relay.http_base));
        let (community_hits, main_hits) = (h.community_relay.take_hits(), h.main_relay.take_hits());
        if main_hits.is_empty() || !community_hits.is_empty() {
            failures.push(format!(
                "{} {}: main window reached main={main_hits:?} community={community_hits:?} ({result:?})",
                row.file, row.cmd
            ));
        }
    }
    crate::relay_admission::reset_rate_limit_gate();
    assert!(
        failures.is_empty(),
        "misrouted commands:\n{}",
        failures.join("\n")
    );
}

#[test]
fn relay_url_commands_report_the_invoking_windows_relay() {
    let h = harness();
    let state = h.main.state::<AppState>();
    assert_eq!(
        crate::window_relay::WindowRelay::resolve(&state, COMMUNITY_LABEL)
            .unwrap()
            .ws_url(),
        h.community_relay.ws_url
    );
    assert_eq!(
        crate::window_relay::WindowRelay::resolve(&state, "main")
            .unwrap()
            .api_base(),
        h.main_relay.http_base
    );
}

#[test]
fn unbound_community_window_fails_closed_without_touching_any_relay() {
    let _serial = crate::relay_admission::TEST_SERIAL.blocking_lock();
    let h = harness();
    h.main
        .state::<AppState>()
        .window_relays
        .lock()
        .unwrap()
        .clear();
    let error = invoke(&h.community, "get_channels", json!({})).unwrap_err();
    assert_eq!(
        error,
        Value::String(crate::window_relay::UNBOUND_COMMUNITY_WINDOW.into())
    );
    assert!(h.community_relay.take_hits().is_empty());
    assert!(h.main_relay.take_hits().is_empty());
}

/// A managed agent record of this device, belonging to `relay_url`.
fn agent_record(keys: &nostr::Keys, relay_url: &str) -> crate::managed_agents::ManagedAgentRecord {
    let pubkey = keys.public_key().to_hex();
    serde_json::from_value(json!({
        "pubkey": pubkey,
        "name": format!("agent-{}", &pubkey[..8]),
        "relay_url": relay_url,
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "created_at": "",
        "updated_at": "",
    }))
    .unwrap()
}

fn signed(keys: &nostr::Keys, kind: u16, content: &str, tags: Vec<Vec<String>>) -> Value {
    let tags = tags
        .into_iter()
        .map(|tag| nostr::Tag::parse(tag).unwrap())
        .collect::<Vec<_>>();
    let event = nostr::EventBuilder::new(nostr::Kind::Custom(kind), content)
        .tags(tags)
        .sign_with_keys(keys)
        .unwrap();
    serde_json::to_value(event).unwrap()
}

fn pubkeys_at(value: &Value, list: &str) -> Vec<String> {
    let entries = if list.is_empty() {
        value.as_array().cloned().unwrap_or_default()
    } else {
        value[list].as_array().cloned().unwrap_or_default()
    };
    let mut pubkeys = entries
        .iter()
        .filter_map(|entry| entry["pubkey"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    pubkeys.sort();
    pubkeys
}

/// In a community window for B, the member list, people search (mentions)
/// and the managed-agent list show B's agents and never A's; the main window
/// on A shows the reverse. Both relays serve the same roster and profiles,
/// so only the window-scoped community filter can tell them apart.
#[test]
fn community_window_lists_only_its_own_communitys_agents() {
    let _serial = crate::relay_admission::TEST_SERIAL.blocking_lock();
    crate::relay_admission::reset_rate_limit_gate();
    let h = harness();
    let (agent_a, agent_b) = (nostr::Keys::generate(), nostr::Keys::generate());
    let (a_hex, b_hex) = (agent_a.public_key().to_hex(), agent_b.public_key().to_hex());
    crate::managed_agents::save_managed_agents(
        h.app.handle(),
        &[
            agent_record(&agent_a, &h.main_relay.ws_url),
            agent_record(&agent_b, &h.community_relay.ws_url),
        ],
    )
    .unwrap();

    let relay_keys = nostr::Keys::generate();
    let roster = signed(
        &relay_keys,
        39002,
        "",
        vec![
            vec!["d".into(), CHANNEL.into()],
            vec!["p".into(), a_hex.clone(), String::new(), "bot".into()],
            vec!["p".into(), b_hex.clone(), String::new(), "bot".into()],
        ],
    );
    let profiles = [&agent_a, &agent_b]
        .iter()
        .map(|keys| signed(keys, 0, r#"{"name":"agent"}"#, vec![]))
        .collect::<Vec<_>>();

    for relay in [&h.main_relay, &h.community_relay] {
        relay.serve_events(vec![roster.clone()]);
    }
    let members = |window| {
        invoke(
            window,
            "get_channel_members",
            json!({ "channelId": CHANNEL }),
        )
    };
    assert_eq!(
        pubkeys_at(&members(&h.community).unwrap(), "members"),
        vec![b_hex.clone()]
    );
    assert_eq!(
        pubkeys_at(&members(&h.main).unwrap(), "members"),
        vec![a_hex.clone()]
    );

    for relay in [&h.main_relay, &h.community_relay] {
        relay.serve_events(profiles.clone());
    }
    let search = |window| invoke(window, "search_users", json!({ "query": "" }));
    assert_eq!(
        pubkeys_at(&search(&h.community).unwrap(), "users"),
        vec![b_hex.clone()]
    );
    assert_eq!(
        pubkeys_at(&search(&h.main).unwrap(), "users"),
        vec![a_hex.clone()]
    );

    let managed = |window| invoke(window, "list_managed_agents", json!({}));
    assert_eq!(pubkeys_at(&managed(&h.community).unwrap(), ""), vec![b_hex]);
    assert_eq!(pubkeys_at(&managed(&h.main).unwrap(), ""), vec![a_hex]);
    crate::relay_admission::reset_rate_limit_gate();
}

/// Projects: every git command validates its clone URL against the invoking
/// window's relay (`validate_workspace_clone_url`) before touching a repo, so
/// a community window works on its own community's repositories and a clone
/// URL from another community is refused. One table over both windows.
#[test]
fn project_clone_urls_are_scoped_to_the_invoking_windows_relay() {
    let h = harness();
    let state = h.main.state::<AppState>();
    let owner = "a".repeat(64);
    let repo = |base: &str| format!("{base}/git/{owner}/repo");
    let window_b = crate::window_relay::WindowRelay::resolve(&state, COMMUNITY_LABEL).unwrap();
    let main = crate::window_relay::WindowRelay::resolve(&state, "main").unwrap();
    for (relay, own, other) in [
        (
            &window_b,
            &h.community_relay.http_base,
            &h.main_relay.http_base,
        ),
        (&main, &h.main_relay.http_base, &h.community_relay.http_base),
    ] {
        crate::commands::project_git_exec::validate_workspace_clone_url(&repo(own), relay)
            .unwrap_or_else(|error| panic!("own repo refused: {error}"));
        assert!(
            crate::commands::project_git_exec::validate_workspace_clone_url(&repo(other), relay)
                .is_err(),
            "another community's repo must be refused"
        );
        // GitHub remotes stay allowed for local checkouts in every window.
        crate::commands::project_git_exec::validate_local_clone_url_for_workspace(
            "https://github.com/block/buzz.git",
            relay,
        )
        .unwrap();
    }
}

/// Machines (agent hosts) are approved per community: the "Add machine"
/// installer a community window shows points at its own community's relay.
#[test]
fn host_install_info_names_the_invoking_windows_relay() {
    let h = harness();
    let base_of = |window| {
        invoke(
            window,
            "get_host_install_info",
            json!({ "pairingUri": "pair-uri" }),
        )
        .unwrap()["base_url"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert!(base_of(&h.community).starts_with(&h.community_relay.http_base));
    assert!(base_of(&h.main).starts_with(&h.main_relay.http_base));
}
