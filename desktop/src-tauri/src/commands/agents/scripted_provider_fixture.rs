//! Test fixture: a mock Tauri app with a scripted `buzz-backend-accesstest`
//! provider on `PATH`, so tests drive the production deploy path
//! (`deploy_to_provider`) end to end without a real provider.

use std::{ffi::OsString, path::PathBuf, sync::MutexGuard};

use crate::{
    app_state::AppState,
    managed_agents::{
        load_managed_agents, managed_agents_store_path, save_managed_agents, BackendKind,
        ManagedAgentRecord,
    },
};

/// The workspace relay the fixture app runs with.
pub(crate) const FIXTURE_RELAY: &str = "wss://relay.example";

struct ScopedEnv {
    key: &'static str,
    prior: Option<OsString>,
}

impl ScopedEnv {
    /// The caller holds the crate-wide process-env lock (`lock_path_mutex`).
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let prior = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prior }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// A mock app whose provider `accesstest` answers every deploy with the reply
/// given to [`ScriptedProvider::new`] and logs the last deploy request.
///
/// Field order is drop order: the app goes first and the process-env lock is
/// released last, after `PATH` is restored.
pub(crate) struct ScriptedProvider {
    pub(crate) app: tauri::App<tauri::test::MockRuntime>,
    request_log: PathBuf,
    _path: ScopedEnv,
    _temp: tempfile::TempDir,
    _env_guard: MutexGuard<'static, ()>,
}

impl ScriptedProvider {
    pub(crate) fn new(deploy_reply: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let env_guard = crate::managed_agents::lock_path_mutex();
        #[cfg(feature = "system-keyring")]
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let request_log = temp.path().join("deploy-request.json");
        let provider = bin.join("buzz-backend-accesstest");
        let script = format!(
            r#"#!/bin/sh
set -eu
read request
case "$request" in
  *\"op\":\"info\"*) printf '%s\n' '{{"ok":true,"name":"t","version":"1","protocol_version":1,"description":"t","config_schema":{{}}}}' ;;
  *\"op\":\"deploy\"*) printf '%s' "$request" > '{log}'; printf '%s\n' '{reply}' ;;
esac
"#,
            log = request_log.display(),
            reply = deploy_reply,
        );
        std::fs::write(&provider, script).unwrap();
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Prepend (never replace) so concurrently running tests keep their
        // tools; the provider id is unique to this fixture.
        let mut search = vec![bin];
        search.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = ScopedEnv::set("PATH", &std::env::join_paths(search).unwrap());

        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier = home.to_str().unwrap().to_owned();
        let state = crate::app_state::build_app_state();
        *state.relay_url_override.lock().unwrap() = Some(FIXTURE_RELAY.into());
        let app = tauri::test::mock_builder()
            .manage(state)
            .build(context)
            .unwrap();
        Self {
            app,
            request_log,
            _path: path,
            _temp: temp,
            _env_guard: env_guard,
        }
    }

    pub(crate) fn state(&self) -> tauri::State<'_, AppState> {
        tauri::Manager::state::<AppState>(&self.app)
    }

    /// A new agent deployed on the scripted provider, with its secret key.
    pub(crate) fn deployed_agent(&self) -> (ManagedAgentRecord, String) {
        let keys = nostr::Keys::generate();
        let nsec = nostr::ToBech32::to_bech32(keys.secret_key()).unwrap();
        let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
            "pubkey": keys.public_key().to_hex(),
            "name": "Remote Agent",
            "relay_url": FIXTURE_RELAY,
            "acp_command": "buzz-acp", "agent_command": "goose", "agent_args": ["acp"],
            "mcp_command": "", "turn_timeout_seconds": 0,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        record.backend = BackendKind::Provider {
            id: "accesstest".into(),
            config: serde_json::json!({}),
        };
        record.backend_agent_id = Some("deployed-before".into());
        (record, nsec)
    }

    /// Save `agents` with each secret key inline. The mock keyring keeps
    /// nothing across entries and is process-global, so the key rides in the
    /// keyringless-fallback shape `load` reads rather than racing other tests
    /// for the shared secret cache.
    pub(crate) fn save(&self, agents: &[(ManagedAgentRecord, String)]) {
        let records: Vec<ManagedAgentRecord> =
            agents.iter().map(|(record, _)| record.clone()).collect();
        save_managed_agents(self.app.handle(), &records).unwrap();
        let store_path = managed_agents_store_path(self.app.handle()).unwrap();
        let mut stored: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&store_path).unwrap()).unwrap();
        for entry in &mut stored {
            if let Some((_, nsec)) = agents
                .iter()
                .find(|(record, _)| entry["pubkey"] == record.pubkey.as_str())
            {
                entry["private_key_nsec"] = nsec.clone().into();
            }
        }
        std::fs::write(&store_path, serde_json::to_vec(&stored).unwrap()).unwrap();
    }

    pub(crate) fn load(&self, pubkey: &str) -> ManagedAgentRecord {
        load_managed_agents(self.app.handle())
            .unwrap()
            .into_iter()
            .find(|record| record.pubkey == pubkey)
            .unwrap()
    }

    /// The last deploy request the provider received.
    pub(crate) fn delivered(&self) -> Option<serde_json::Value> {
        std::fs::read_to_string(&self.request_log)
            .ok()
            .map(|raw| serde_json::from_str(&raw).unwrap())
    }
}
