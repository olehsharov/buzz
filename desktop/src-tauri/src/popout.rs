//! Pop-out windows: secondary webview windows that load the same app entry as
//! the main window and open directly on one route (Cmd-click "open in new
//! window").
//!
//! The main window remains the single owner of app-wide effects (deep links,
//! tray actions, notification activation, toasts). A pop-out learns which
//! route and community to render from a one-time launch payload stored here,
//! keyed by its window label, and can hand navigation back to the main window
//! with `focus_main_window_route`.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// Label of the app's primary window, as configured in `tauri.conf.json`.
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
/// Prefix of every pop-out window label (`popout-<uuid v4>`).
pub(crate) const POPOUT_LABEL_PREFIX: &str = "popout-";
/// Upper bound on simultaneously open pop-out windows.
pub(crate) const MAX_POPOUT_WINDOWS: usize = 20;
/// Upper bound on an app-relative route, in bytes.
pub(crate) const MAX_ROUTE_BYTES: usize = 2048;
/// Upper bound on the serialized community descriptor, in bytes.
pub(crate) const MAX_COMMUNITY_BYTES: usize = 16 * 1024;
/// Event delivered to the main window when a pop-out asks it to navigate.
pub(crate) const NAVIGATE_MAIN_EVENT: &str = "popout:navigate-main";

/// One-time launch payload a pop-out reads on startup.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopoutLaunch {
    pub route: String,
    pub community: serde_json::Value,
    /// True when the pop-out was opened from a community window and inherits
    /// that window's relay binding: it then follows its parent window's
    /// community instead of pausing whenever the main window is elsewhere.
    pub bound: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NavigateMainPayload {
    route: String,
}

/// Whether `label` names a pop-out window.
pub(crate) fn is_popout_label(label: &str) -> bool {
    label.starts_with(POPOUT_LABEL_PREFIX)
}

/// Whether the window-state plugin should save and restore geometry for the
/// window with `label`. The main window and community windows are persisted:
/// a community window's label is stable per community (`community-<id>`), so
/// it reopens where the user left it and the state file grows by at most one
/// entry per community. Pop-out and huddle windows carry per-instance labels,
/// so persisting them would grow the state file by one entry per window ever
/// opened.
pub(crate) fn persists_window_state(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || crate::window_relay::is_community_window_label(label)
}

/// Validate an app-relative route (path plus optional query/fragment).
///
/// Accepts only a single leading `/` (never `//`, which a URL parser reads as
/// a network-path reference), no `:` in the first path segment (so nothing can
/// be read as a scheme), no backslashes (WHATWG parsers treat `\` like `/`), no
/// control characters, and at most [`MAX_ROUTE_BYTES`] bytes.
pub(crate) fn validate_route(route: &str) -> Result<(), String> {
    if route.is_empty() {
        return Err("route must not be empty".into());
    }
    if route.len() > MAX_ROUTE_BYTES {
        return Err(format!("route exceeds {MAX_ROUTE_BYTES} bytes"));
    }
    let Some(rest) = route.strip_prefix('/') else {
        return Err("route must start with '/'".into());
    };
    if rest.starts_with('/') {
        return Err("route must not start with '//'".into());
    }
    if route.contains('\\') {
        return Err("route must not contain backslashes".into());
    }
    if route.chars().any(char::is_control) {
        return Err("route must not contain control characters".into());
    }
    let first_segment = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if first_segment.contains(':') {
        return Err("route must not contain a scheme".into());
    }
    Ok(())
}

fn validate_community(community: &serde_json::Value) -> Result<(), String> {
    let size = serde_json::to_vec(community)
        .map_err(|error| format!("community is not serializable: {error}"))?
        .len();
    if size > MAX_COMMUNITY_BYTES {
        return Err(format!("community exceeds {MAX_COMMUNITY_BYTES} bytes"));
    }
    Ok(())
}

#[derive(Default)]
struct RegistryInner {
    /// Labels of pop-outs reserved or open; bounded by `MAX_POPOUT_WINDOWS`.
    open: HashSet<String>,
    /// Launch payloads not yet taken by their window.
    launches: HashMap<String, PopoutLaunch>,
    /// Parent community window of a pop-out that inherited its binding;
    /// "Open in main window" hands the route to that window instead.
    parents: HashMap<String, String>,
}

/// Tracks open pop-out windows and their one-time launch payloads.
#[derive(Default)]
pub struct PopoutRegistry {
    inner: Mutex<RegistryInner>,
}

impl PopoutRegistry {
    fn lock(&self) -> MutexGuard<'_, RegistryInner> {
        // The maps hold no cross-field invariant a panicking writer could
        // break halfway, so a poisoned lock is still safe to use.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reserve a slot for `label` and store its launch payload. Fails when
    /// `MAX_POPOUT_WINDOWS` pop-outs are already open.
    pub(crate) fn reserve(&self, label: &str, launch: PopoutLaunch) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.open.len() >= MAX_POPOUT_WINDOWS {
            return Err(format!(
                "too many open windows (limit {MAX_POPOUT_WINDOWS}); close one and try again"
            ));
        }
        inner.open.insert(label.to_owned());
        inner.launches.insert(label.to_owned(), launch);
        Ok(())
    }

    /// Return and remove the launch payload for `label`. A second call
    /// returns `None`.
    pub(crate) fn take(&self, label: &str) -> Option<PopoutLaunch> {
        self.lock().launches.remove(label)
    }

    /// Forget `label`: frees its slot and drops any untaken payload. Called
    /// when its window is destroyed or failed to build.
    pub(crate) fn release(&self, label: &str) {
        let mut inner = self.lock();
        inner.open.remove(label);
        inner.launches.remove(label);
        inner.parents.remove(label);
    }

    /// Record that pop-out `label` follows community window `parent`.
    pub(crate) fn set_parent(&self, label: &str, parent: &str) {
        self.lock()
            .parents
            .insert(label.to_owned(), parent.to_owned());
    }

    /// The community window pop-out `label` follows, if any.
    pub(crate) fn parent(&self, label: &str) -> Option<String> {
        self.lock().parents.get(label).cloned()
    }

    #[cfg(test)]
    fn open_count(&self) -> usize {
        self.lock().open.len()
    }
}

/// Open `route` of `community` in a new pop-out window and return its label.
///
/// `route` must be an app-relative path (see [`validate_route`]). `community`
/// is opaque to the backend and handed back unchanged by
/// [`take_popout_launch`]. Async so window creation never runs on the main
/// thread's command dispatch (synchronous window builds deadlock on Windows).
#[tauri::command]
pub async fn open_popout_window(
    route: String,
    community: serde_json::Value,
    app: tauri::AppHandle,
    webview: tauri::Webview,
    registry: State<'_, PopoutRegistry>,
) -> Result<String, String> {
    validate_route(&route)?;
    validate_community(&community)?;

    let label = format!("{POPOUT_LABEL_PREFIX}{}", uuid::Uuid::new_v4());
    // A pop-out opened from a community window inherits that window's relay
    // binding, so its commands keep targeting the parent's community.
    let parent = webview.label().to_string();
    let inherited = crate::window_relay::is_community_window_label(&parent)
        .then(|| inherit_parent_relay(&app, &parent, &label))
        .transpose()?
        .flatten();
    let bound = inherited.is_some();
    registry.reserve(
        &label,
        PopoutLaunch {
            route,
            community,
            bound,
        },
    )?;
    if bound {
        registry.set_parent(&label, &parent);
    }

    let built = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .title("Buzz")
        .inner_size(900.0, 700.0)
        .min_inner_size(480.0, 400.0)
        // Match the main window: let the webview receive HTML5 file drops
        // (composer attachments) instead of Tauri's native drop handler.
        .disable_drag_drop_handler()
        .focused(true)
        .build();
    let window = match built {
        Ok(window) => window,
        Err(error) => {
            registry.release(&label);
            crate::window_relay::release_window_relay(
                &app.state::<crate::app_state::AppState>(),
                &label,
            );
            return Err(error.to_string());
        }
    };
    if let Err(error) = window.set_focus() {
        eprintln!("buzz-desktop: failed to focus pop-out window: {error}");
    }
    Ok(label)
}

/// Copy community window `parent`'s relay binding onto pop-out `label`.
/// `Ok(None)` when the parent is not bound (it then fails closed itself).
pub(crate) fn inherit_parent_relay<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    parent: &str,
    label: &str,
) -> Result<Option<String>, String> {
    let state = app.state::<crate::app_state::AppState>();
    let mut relays = state.window_relays.lock().map_err(|e| e.to_string())?;
    let Some(relay_url) = relays.get(parent).cloned() else {
        return Ok(None);
    };
    relays.insert(label.to_string(), relay_url.clone());
    Ok(Some(relay_url))
}

/// Return and remove the calling pop-out window's launch payload. Returns
/// `None` for any other window and on every call after the first.
#[tauri::command]
pub fn take_popout_launch(
    window: tauri::WebviewWindow,
    registry: State<'_, PopoutRegistry>,
) -> Option<PopoutLaunch> {
    registry.take(window.label())
}

/// Bring the main window forward and ask it (and only it) to navigate to
/// `route`. A pop-out that follows a community window hands the route to
/// that window instead: the main window is on another community.
#[tauri::command]
pub fn focus_main_window_route(
    route: String,
    app: tauri::AppHandle,
    webview: tauri::Webview,
    registry: State<'_, PopoutRegistry>,
) -> Result<(), String> {
    validate_route(&route)?;
    let target = registry
        .parent(webview.label())
        .unwrap_or_else(|| MAIN_WINDOW_LABEL.to_string());
    let window = app
        .get_webview_window(&target)
        .ok_or("main window is unavailable")?;
    if let Err(error) = window.unminimize() {
        eprintln!("buzz-desktop: failed to unminimize main window: {error}");
    }
    if let Err(error) = window.show() {
        eprintln!("buzz-desktop: failed to show main window: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("buzz-desktop: failed to focus main window: {error}");
    }
    app.emit_to(
        target.as_str(),
        NAVIGATE_MAIN_EVENT,
        NavigateMainPayload { route },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "popout_tests.rs"]
mod tests;
