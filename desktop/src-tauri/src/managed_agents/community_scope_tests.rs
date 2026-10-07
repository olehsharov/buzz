use super::*;

const RAILWAY: &str = "wss://first.example";
const AWS: &str = "wss://second.example";

fn raw(pubkey: &str, relay_url: Option<&str>, builtin: bool) -> serde_json::Value {
    let mut record = serde_json::json!({
        "pubkey": pubkey,
        "name": format!("agent-{pubkey}"),
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "is_builtin": builtin,
        "slug": if pubkey.is_empty() { Some(format!("slug-{builtin}")) } else { None },
        "created_at": "",
        "updated_at": ""
    });
    if let Some(relay_url) = relay_url {
        record["relay_url"] = relay_url.into();
    }
    record
}

fn write_store(dir: &Path, records: &[serde_json::Value]) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(records).unwrap();
    std::fs::write(dir.join("managed-agents.json"), &bytes).unwrap();
    bytes
}

fn read_store(dir: &Path) -> Vec<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(dir.join("managed-agents.json")).unwrap()).unwrap()
}

fn relay_of(record: &serde_json::Value) -> &str {
    record["relay_url"].as_str().unwrap_or("")
}

#[test]
fn legacy_assignment_moves_unassigned_records_to_the_first_community() {
    let dir = tempfile::tempdir().unwrap();
    let a = "aa".repeat(32);
    let b = "bb".repeat(32);
    let c = "cc".repeat(32);
    let original = write_store(
        dir.path(),
        &[
            raw(&a, Some(""), false),  // unpinned instance
            raw(&b, Some(AWS), false), // pinned elsewhere: kept
            raw(&c, None, false),      // field absent: unassigned
            raw("", Some(""), false),  // user definition: scoped
            raw("", Some(""), true),   // built-in definition: global
        ],
    );

    let assigned = assign_legacy_agent_communities(dir.path(), "WSS://First.Example:443/").unwrap();
    assert_eq!(assigned, 3);

    let store = read_store(dir.path());
    assert_eq!(relay_of(&store[0]), RAILWAY, "normalized first community");
    assert_eq!(relay_of(&store[1]), AWS, "existing pin kept");
    assert_eq!(relay_of(&store[2]), RAILWAY);
    assert_eq!(relay_of(&store[3]), RAILWAY, "user definition scoped");
    assert_eq!(relay_of(&store[4]), "", "built-in definition stays global");
    // Every other field survives.
    assert_eq!(store[0]["name"], format!("agent-{a}"));

    assert_eq!(std::fs::read(backup_path(dir.path())).unwrap(), original);
    assert!(legacy_assignment_done(dir.path()));
}

#[test]
fn legacy_assignment_is_idempotent_behind_its_marker() {
    let dir = tempfile::tempdir().unwrap();
    write_store(dir.path(), &[raw(&"aa".repeat(32), Some(""), false)]);
    assert_eq!(
        assign_legacy_agent_communities(dir.path(), RAILWAY).unwrap(),
        1
    );
    let backup = std::fs::read(backup_path(dir.path())).unwrap();

    // A record saved unassigned after the marker must NOT be swept into the
    // first community by a later apply — the write-time stamp owns it.
    let mut store = read_store(dir.path());
    store.push(raw(&"bb".repeat(32), Some(""), false));
    write_store(dir.path(), &store);

    assert_eq!(assign_legacy_agent_communities(dir.path(), AWS).unwrap(), 0);
    let store = read_store(dir.path());
    assert_eq!(relay_of(&store[0]), RAILWAY);
    assert_eq!(relay_of(&store[1]), "");
    assert_eq!(std::fs::read(backup_path(dir.path())).unwrap(), backup);
}

#[test]
fn legacy_assignment_with_nothing_to_assign_writes_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    write_store(dir.path(), &[raw(&"aa".repeat(32), Some(AWS), false)]);
    assert_eq!(
        assign_legacy_agent_communities(dir.path(), RAILWAY).unwrap(),
        0
    );
    assert!(!backup_path(dir.path()).exists());
    assert!(legacy_assignment_done(dir.path()));
}

#[test]
fn legacy_assignment_failure_leaves_no_marker_so_it_retries() {
    let dir = tempfile::tempdir().unwrap();
    write_store(dir.path(), &[raw(&"aa".repeat(32), Some(""), false)]);
    assert!(assign_legacy_agent_communities(dir.path(), "not a relay").is_err());
    assert!(!legacy_assignment_done(dir.path()));

    std::fs::write(dir.path().join("managed-agents.json"), b"{ broken").unwrap();
    assert!(assign_legacy_agent_communities(dir.path(), RAILWAY).is_err());
    assert!(!legacy_assignment_done(dir.path()));
}

fn record(pubkey: &str, relay_url: &str, builtin: bool) -> ManagedAgentRecord {
    serde_json::from_value(raw(pubkey, Some(relay_url), builtin)).unwrap()
}

#[test]
fn stamp_assigns_only_unassigned_scopable_records() {
    let mut records = vec![
        record(&"aa".repeat(32), "", false),
        record(&"bb".repeat(32), RAILWAY, false),
        record("", "", false),
        record("", "", true),
    ];
    stamp_unassigned(&mut records, AWS);
    let relays: Vec<_> = records.iter().map(|r| r.relay_url.as_str()).collect();
    assert_eq!(relays, [AWS, RAILWAY, AWS, ""]);
}

#[test]
fn record_in_community_hides_other_communities_but_not_builtins() {
    assert!(record_in_community(
        &record(&"aa".repeat(32), RAILWAY, false),
        AWS,
        RAILWAY
    ));
    assert!(!record_in_community(
        &record(&"aa".repeat(32), RAILWAY, false),
        AWS,
        AWS
    ));
    assert!(!record_in_community(&record("", RAILWAY, false), AWS, AWS));
    assert!(record_in_community(&record("", "", true), AWS, AWS));
}

#[cfg(not(target_os = "windows"))]
mod app_seams {
    use super::*;
    use tauri::test::MockRuntime;

    pub(super) struct TestApp {
        pub app: tauri::App<MockRuntime>,
        _data: tempfile::TempDir,
    }

    pub(super) fn app_on(relay: &str) -> TestApp {
        #[cfg(feature = "system-keyring")]
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let data = tempfile::tempdir().unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
        let state = crate::app_state::build_app_state();
        *state.relay_url_override.lock().unwrap() = Some(relay.into());
        let app = tauri::test::mock_builder()
            .manage(state)
            .build(context)
            .unwrap();
        TestApp { app, _data: data }
    }

    pub(super) fn set_relay(test: &TestApp, relay: &str) {
        let state = test.app.state::<crate::app_state::AppState>();
        *state.relay_url_override.lock().unwrap() = Some(relay.into());
    }

    fn active_relay(test: &TestApp) -> String {
        crate::relay::relay_ws_url_with_override(&test.app.state::<crate::app_state::AppState>())
    }

    fn base_dir(test: &TestApp) -> PathBuf {
        crate::managed_agents::managed_agents_base_dir(test.app.handle()).unwrap()
    }

    #[test]
    fn saves_stamp_unassigned_records_with_the_active_community_after_assignment() {
        let test = app_on(AWS);
        let handle = test.app.handle();
        // Before the legacy assignment: no stamping (it decides legacy records).
        crate::managed_agents::save_managed_agents(handle, &[record(&"aa".repeat(32), "", false)])
            .unwrap();
        assert_eq!(read_store(&base_dir(&test))[0]["relay_url"], "");

        assign_legacy_agent_communities(&base_dir(&test), RAILWAY).unwrap();
        crate::managed_agents::save_managed_agents(
            handle,
            &[
                record(&"aa".repeat(32), RAILWAY, false),
                record(&"bb".repeat(32), "", false),
            ],
        )
        .unwrap();
        let instances = crate::managed_agents::load_managed_agents(handle).unwrap();
        let relays: Vec<_> = instances.iter().map(|r| r.relay_url.as_str()).collect();
        assert_eq!(relays, [RAILWAY, AWS]);
    }

    #[test]
    fn agent_and_definition_lists_show_only_the_active_community() {
        let test = app_on(RAILWAY);
        let handle = test.app.handle();
        assign_legacy_agent_communities(&base_dir(&test), RAILWAY).unwrap();
        let on_railway = "aa".repeat(32);
        let on_aws = "bb".repeat(32);
        crate::managed_agents::save_managed_agents(
            handle,
            &[
                record(&on_railway, RAILWAY, false),
                record(&on_aws, AWS, false),
            ],
        )
        .unwrap();
        let mut railway_def = record("", RAILWAY, false);
        railway_def.slug = Some("railway-def".into());
        let mut aws_def = record("", AWS, false);
        aws_def.slug = Some("aws-def".into());
        crate::managed_agents::storage::save_agent_definitions(handle, &[railway_def, aws_def])
            .unwrap();

        let listed = |test: &TestApp| {
            let relay = active_relay(test);
            let agents = crate::commands::list_community_managed_agents(test.app.handle(), &relay)
                .unwrap()
                .into_iter()
                .map(|agent| agent.pubkey)
                .collect::<Vec<_>>();
            let mut personas = crate::managed_agents::load_personas(test.app.handle()).unwrap();
            crate::commands::retain_community_personas(test.app.handle(), &relay, &mut personas)
                .unwrap();
            let definitions = personas
                .into_iter()
                .filter(|persona| !persona.is_builtin)
                .map(|persona| persona.id)
                .collect::<Vec<_>>();
            (agents, definitions)
        };

        assert_eq!(
            listed(&test),
            (vec![on_railway.clone()], vec!["railway-def".to_string()])
        );
        // Switching communities flips both lists.
        set_relay(&test, AWS);
        assert_eq!(listed(&test), (vec![on_aws], vec!["aws-def".to_string()]));
    }

    #[test]
    fn persona_saves_keep_the_definitions_community() {
        let test = app_on(AWS);
        let handle = test.app.handle();
        assign_legacy_agent_communities(&base_dir(&test), RAILWAY).unwrap();
        let mut definition = record("", RAILWAY, false);
        definition.slug = Some("mine".into());
        crate::managed_agents::storage::save_agent_definitions(handle, &[definition]).unwrap();

        // A persona save goes through the `AgentDefinition` view, which drops
        // the community; the stored one must survive, not be re-stamped AWS.
        let personas = crate::managed_agents::load_personas(handle).unwrap();
        crate::managed_agents::save_personas(handle, &personas).unwrap();
        let stored = crate::managed_agents::storage::load_agent_definitions(handle).unwrap();
        let mine = stored
            .iter()
            .find(|record| record.slug.as_deref() == Some("mine"))
            .unwrap();
        assert_eq!(mine.relay_url, RAILWAY);
        // A brand-new definition is created in the active community.
        let fresh = stored
            .iter()
            .filter(|record| !record.is_builtin && record.slug.as_deref() != Some("mine"))
            .count();
        assert_eq!(fresh, 0, "built-in merge adds only global templates");
        assert!(stored
            .iter()
            .filter(|record| record.is_builtin)
            .all(|record| record.relay_url.is_empty()));
    }
}
