//! Per-window relay binding for community windows.
//!
//! The main window drives the app-wide community through `apply_workspace`,
//! which installs `AppState::relay_url_override`. A community window
//! (`community-<community id>`, opened with Cmd/Ctrl-click on the community
//! rail) runs a *different* community side by side, so every chat command it
//! invokes must target that window's relay rather than the global one.
//!
//! The binding lives in `AppState::window_relays`, keyed by window label, and
//! is resolved per command through the [`WindowRelay`] command argument. The
//! value is resolved once, at command entry, and threaded explicitly into every
//! relay call — deliberately not a task-local, which spawned or blocking work
//! would silently lose. Windows without a binding (main, pop-outs, huddles, and
//! background tasks, which have no window at all) keep using the global relay.
//! A community window that has not bound yet fails closed instead of leaking
//! into the main window's community.

use std::collections::HashSet;

use tauri::ipc::{CommandArg, CommandItem, InvokeError};
use tauri::{Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::app_state::AppState;
use crate::relay::{relay_http_base_url, relay_ws_url_with_override};

/// Prefix of every community window label (`community-<community id>`).
pub(crate) const COMMUNITY_WINDOW_LABEL_PREFIX: &str = "community-";
/// Upper bound on a community id embedded in a window label.
const MAX_COMMUNITY_ID_LEN: usize = 128;
/// Upper bound on a community window title, in characters.
const MAX_TITLE_CHARS: usize = 200;
/// Error returned when a community window invokes a relay command before it
/// has bound its community.
pub(crate) const UNBOUND_COMMUNITY_WINDOW: &str =
    "this community window has not connected to its community yet";

/// Whether `id` is a community id that may be embedded in a window label.
/// Tauri labels accept only `[A-Za-z0-9-/:_]`; community ids are UUIDs.
fn is_safe_community_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_COMMUNITY_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Whether `label` names a community window.
pub(crate) fn is_community_window_label(label: &str) -> bool {
    label
        .strip_prefix(COMMUNITY_WINDOW_LABEL_PREFIX)
        .is_some_and(is_safe_community_id)
}

/// The one window label a community may own.
pub(crate) fn community_window_label(community_id: &str) -> Result<String, String> {
    if !is_safe_community_id(community_id) {
        return Err("invalid community id".into());
    }
    Ok(format!("{COMMUNITY_WINDOW_LABEL_PREFIX}{community_id}"))
}

/// The relay (ws/wss URL) a command invoked from `window_label` must target.
///
/// A bound window gets its own relay. A community window without a binding is
/// an error (fail closed). Every other caller — the main window, pop-outs,
/// huddle companions — uses the app-wide workspace relay.
pub fn relay_ws_url_for_window(state: &AppState, window_label: &str) -> Result<String, String> {
    let bound = state
        .window_relays
        .lock()
        .map_err(|error| error.to_string())?
        .get(window_label)
        .cloned();
    match bound {
        Some(relay_url) => Ok(relay_url),
        None if is_community_window_label(window_label) => {
            Err(UNBOUND_COMMUNITY_WINDOW.to_string())
        }
        None => Ok(relay_ws_url_with_override(state)),
    }
}

/// The relay a command targets, resolved once from the invoking window.
///
/// Taken as a command argument (`relay: WindowRelay`): Tauri resolves it from
/// the calling webview before the command body runs, so a command can never
/// forget to scope itself. Pass `api_base()`/`ws_url()` explicitly into every
/// relay helper; never re-read the global override inside the command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRelay {
    label: String,
    ws_url: String,
}

impl WindowRelay {
    /// Resolve the relay for `window_label` (see [`relay_ws_url_for_window`]).
    pub fn resolve(state: &AppState, window_label: &str) -> Result<Self, String> {
        Ok(Self {
            label: window_label.to_string(),
            ws_url: relay_ws_url_for_window(state, window_label)?,
        })
    }

    /// The invoking window's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The relay WebSocket URL (`ws://` / `wss://`).
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// The relay HTTP API base URL.
    pub fn api_base(&self) -> String {
        relay_http_base_url(&self.ws_url)
    }

    /// Stale-result fence: whether the invoking window still targets the relay
    /// this value was resolved for. A main-window workspace switch or a
    /// community-window rebind during an await makes it false.
    pub fn is_current(&self, state: &AppState) -> bool {
        relay_ws_url_for_window(state, &self.label).is_ok_and(|current| current == self.ws_url)
    }
}

impl<'de, R: Runtime> CommandArg<'de, R> for WindowRelay {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        let webview = command.message.webview_ref();
        let state = webview
            .try_state::<AppState>()
            .ok_or_else(|| InvokeError::from("app state is unavailable"))?;
        WindowRelay::resolve(&state, webview.label()).map_err(InvokeError::from)
    }
}

/// The `scheme://host[:port]` origin of a community relay URL, normalized the
/// way the frontend's saved-community list (`communityRelaySetKey`) is: only a
/// bare http(s)/ws(s) origin with no credentials, path, query, or fragment.
pub(crate) fn relay_origin(relay_url: &str) -> Option<String> {
    let url = url::Url::parse(&relay_http_base_url(relay_url)).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// The `host[:port]` authority of a relay URL, lowercased, matching what the
/// browser's `URL.host` reports (default ports omitted).
pub(crate) fn relay_authority(relay_url: &str) -> Option<String> {
    let url = url::Url::parse(&relay_http_base_url(relay_url)).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// Validate and install `relay_url` as the relay of community window `label`.
///
/// Only community windows may bind, and only to a relay in the user's saved
/// community list (the list the main window publishes through
/// `set_agent_avatar_communities`), so a window can never be pointed at an
/// arbitrary origin.
pub(crate) fn bind_window_relay(
    state: &AppState,
    label: &str,
    relay_url: &str,
) -> Result<(), String> {
    if !is_community_window_label(label) {
        return Err("only a community window can bind a community".into());
    }
    let relay_url = relay_url.trim();
    let origin = relay_origin(relay_url).ok_or("community relay URL is not a bare relay origin")?;
    let saved = state
        .agent_avatar_communities
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .filter_map(|entry| relay_origin(entry))
        .collect::<HashSet<_>>();
    if !saved.contains(&origin) {
        return Err("community relay is not one of this device's saved communities".into());
    }
    state
        .window_relays
        .lock()
        .map_err(|error| error.to_string())?
        .insert(label.to_string(), relay_url.to_string());
    Ok(())
}

/// Forget the binding of window `label` (called when the window is destroyed).
pub(crate) fn release_window_relay(state: &AppState, label: &str) {
    match state.window_relays.lock() {
        Ok(mut relays) => {
            relays.remove(label);
        }
        Err(poisoned) => {
            poisoned.into_inner().remove(label);
        }
    }
}

/// Every relay the app is currently bound to: the main workspace relay plus
/// each community window's relay. The media proxy resolves path-encoded relay
/// hosts against this set so it never mints media auth for any other origin.
pub(crate) fn bound_relay_urls(state: &AppState) -> Vec<String> {
    let mut urls = vec![relay_ws_url_with_override(state)];
    if let Ok(relays) = state.window_relays.lock() {
        urls.extend(relays.values().cloned());
    }
    urls
}

/// The HTTP API base of the bound relay whose authority is `authority`.
pub(crate) fn bound_relay_base_for_authority(state: &AppState, authority: &str) -> Option<String> {
    let wanted = authority.to_ascii_lowercase();
    bound_relay_urls(state)
        .into_iter()
        .find(|url| relay_authority(url).is_some_and(|candidate| candidate == wanted))
        .map(|url| relay_http_base_url(&url))
}

/// Bind the calling community window to `relay_url` (validated against the
/// saved community list). Re-binding replaces the previous relay.
///
/// Then reconcile this community's scoped event store, exactly as
/// `apply_workspace` does for the main window's community, and before the
/// window exposes its community: local agent, definition, and team records
/// of this community are retained (and flushed by the shared publisher) so
/// its catalogs and agent records are current while the window owns it. The
/// two-writer guard keeps the main window off this community meanwhile.
#[tauri::command]
pub async fn bind_window_community(
    relay_url: String,
    webview: tauri::Webview,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    bind_window_relay(&state, webview.label(), &relay_url)?;
    let scope =
        crate::managed_agents::retention::retention_scope_for(&app, &state, relay_url.trim())?;
    crate::event_sync::run_event_sync_blocking(
        app.clone(),
        scope.owner_keys,
        scope.db_path,
        scope.relay_url,
    )
    .await
    .map_err(|error| format!("community event sync failed: {error}"))
}

fn validate_title(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("title exceeds {MAX_TITLE_CHARS} characters"));
    }
    if title.chars().any(char::is_control) {
        return Err("title must not contain control characters".into());
    }
    Ok(if title.is_empty() {
        "Buzz".to_string()
    } else {
        title.to_string()
    })
}

fn focus_window<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    if let Err(error) = window.unminimize() {
        eprintln!("buzz-desktop: failed to unminimize community window: {error}");
    }
    if let Err(error) = window.show() {
        eprintln!("buzz-desktop: failed to show community window: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("buzz-desktop: failed to focus community window: {error}");
    }
}

/// Focus the community window for `community_id` when one is open. Returns
/// whether a window was focused.
pub(crate) fn focus_existing_community_window<R: Runtime>(
    app: &tauri::AppHandle<R>,
    community_id: &str,
) -> Result<bool, String> {
    let label = community_window_label(community_id)?;
    Ok(match app.get_webview_window(&label) {
        Some(window) => {
            focus_window(&window);
            true
        }
        None => false,
    })
}

/// Open `community_id` in its own window, or focus the window it already has.
/// A community owns at most one window (label `community-<id>`). Async so the
/// window build never runs on the main thread's command dispatch.
#[tauri::command]
pub async fn open_community_window(
    community_id: String,
    title: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    open_or_focus_community_window(&app, &community_id, title.as_deref()).map(|opened| opened.label)
}

/// Result of [`open_or_focus_community_window`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OpenedCommunityWindow {
    pub label: String,
    /// False when the community already had a window, which was focused.
    pub created: bool,
}

pub(crate) fn open_or_focus_community_window<R: Runtime>(
    app: &tauri::AppHandle<R>,
    community_id: &str,
    title: Option<&str>,
) -> Result<OpenedCommunityWindow, String> {
    let label = community_window_label(community_id)?;
    let title = validate_title(title.unwrap_or_default())?;
    if focus_existing_community_window(app, community_id)? {
        return Ok(OpenedCommunityWindow {
            label,
            created: false,
        });
    }
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1100.0, 760.0)
        .min_inner_size(640.0, 480.0)
        // Match the main window: HTML5 file drops reach the composer.
        .disable_drag_drop_handler()
        .focused(true);
    // Match the main window's chrome (tauri.conf.json): an overlay title bar
    // with the traffic lights where the app's top chrome leaves room for
    // them. The top chrome is the drag region.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(16.0, 25.0));
    let built = builder.build();
    match built {
        Ok(window) => {
            focus_window(&window);
            Ok(OpenedCommunityWindow {
                label,
                created: true,
            })
        }
        // A concurrent open of the same community won the race: focus it.
        Err(error) => {
            if focus_existing_community_window(app, community_id)? {
                Ok(OpenedCommunityWindow {
                    label,
                    created: false,
                })
            } else {
                Err(error.to_string())
            }
        }
    }
}

/// Focus the community window of `community_id` if one is open. The main
/// window calls this before switching to a community so a community never
/// runs in two windows at once. Returns whether a window was focused.
#[tauri::command]
pub fn focus_community_window(community_id: String, app: tauri::AppHandle) -> Result<bool, String> {
    focus_existing_community_window(&app, &community_id)
}

#[cfg(test)]
#[path = "window_relay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "window_relay_command_tests.rs"]
mod command_tests;

#[cfg(test)]
#[path = "window_relay_gate_tests.rs"]
mod gate_tests;
