use super::*;

const RAILWAY: &str = "wss://first.example";
const AWS: &str = "wss://second.example";
const GATEWAY: &str = "http://gateway-a.invalid:4001";

fn defaults_with(key: &str, value: &str) -> GlobalAgentConfig {
    GlobalAgentConfig {
        env_vars: BTreeMap::from([(key.to_string(), value.to_string())]),
        provider: Some("anthropic".into()),
        model: Some("claude-test".into()),
        preferred_runtime: Some("goose".into()),
    }
}

fn write_legacy(dir: &Path, config: &GlobalAgentConfig) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(config).unwrap();
    std::fs::write(legacy_path(dir), &bytes).unwrap();
    bytes
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn record(relay_url: &str) -> ManagedAgentRecord {
    serde_json::from_value(serde_json::json!({
        "pubkey": "aa".repeat(32), "name": "agent", "relay_url": relay_url,
        "acp_command": "buzz-acp", "agent_command": "goose", "agent_args": [],
        "mcp_command": "", "turn_timeout_seconds": 0,
        "created_at": "", "updated_at": ""
    }))
    .unwrap()
}

#[test]
fn migration_moves_legacy_defaults_to_the_first_community_only() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = defaults_with("ANTHROPIC_BASE_URL", GATEWAY);
    let original = write_legacy(dir.path(), &legacy);

    assert!(migrate_legacy_global_agent_config(dir.path(), "WSS://First.Example:443/").unwrap());

    let store = read_store(dir.path()).unwrap();
    assert_eq!(
        store.for_relay(RAILWAY),
        &legacy,
        "first community keeps them"
    );
    assert_eq!(store.for_relay(AWS), &GlobalAgentConfig::default());
    assert_eq!(store.communities.len(), 1, "no other community gets a copy");
    let marker = store.legacy_migration.expect("marker in the same write");
    assert_eq!(
        marker.home_relay_key,
        crate::relay::community_relay_key(RAILWAY)
    );
    assert!(marker.had_legacy_values);

    assert_eq!(
        std::fs::read(legacy_backup_path(dir.path())).unwrap(),
        original
    );
    assert!(
        !legacy_path(dir.path()).exists(),
        "no live-looking leftover"
    );
    #[cfg(unix)]
    {
        assert_eq!(mode(&store_path(dir.path())), 0o600);
        assert_eq!(mode(&legacy_backup_path(dir.path())), 0o600);
    }
}

#[test]
fn migration_is_idempotent_behind_its_marker() {
    let dir = tempfile::tempdir().unwrap();
    write_legacy(dir.path(), &defaults_with("ANTHROPIC_BASE_URL", GATEWAY));
    assert!(migrate_legacy_global_agent_config(dir.path(), RAILWAY).unwrap());
    let after_first = std::fs::read(store_path(dir.path())).unwrap();

    // A legacy file reappearing (e.g. an older build wrote it) is not re-read,
    // and a different "first" community never receives anything.
    write_legacy(
        dir.path(),
        &defaults_with("ANTHROPIC_BASE_URL", "http://other"),
    );
    assert!(!migrate_legacy_global_agent_config(dir.path(), AWS).unwrap());
    assert_eq!(std::fs::read(store_path(dir.path())).unwrap(), after_first);
    assert_eq!(
        read_store(dir.path()).unwrap().for_relay(AWS),
        &GlobalAgentConfig::default()
    );
}

#[test]
fn migration_follows_the_community_scope_home_over_the_first_community() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(COMMUNITY_SCOPE_MARKER),
        serde_json::json!({ "home_relay_url": RAILWAY, "assigned": 2 }).to_string(),
    )
    .unwrap();
    let legacy = defaults_with("ANTHROPIC_BASE_URL", GATEWAY);
    write_legacy(dir.path(), &legacy);

    migrate_legacy_global_agent_config(dir.path(), AWS).unwrap();

    let store = read_store(dir.path()).unwrap();
    assert_eq!(store.for_relay(RAILWAY), &legacy);
    assert_eq!(store.for_relay(AWS), &GlobalAgentConfig::default());
}

#[test]
fn migration_without_a_legacy_file_marks_and_leaves_every_community_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(migrate_legacy_global_agent_config(dir.path(), RAILWAY).unwrap());
    let store = read_store(dir.path()).unwrap();
    assert!(store.communities.is_empty());
    assert!(!store.legacy_migration.unwrap().had_legacy_values);
    assert!(!legacy_backup_path(dir.path()).exists());
}

#[test]
fn migration_failure_leaves_no_marker_so_it_retries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(legacy_path(dir.path()), b"{ not json").unwrap();
    assert!(migrate_legacy_global_agent_config(dir.path(), RAILWAY).is_err());
    assert!(!store_path(dir.path()).exists());
    assert!(
        legacy_path(dir.path()).exists(),
        "source kept for the retry"
    );

    let legacy = defaults_with("ANTHROPIC_BASE_URL", GATEWAY);
    write_legacy(dir.path(), &legacy);
    assert!(migrate_legacy_global_agent_config(dir.path(), RAILWAY).unwrap());
    assert_eq!(read_store(dir.path()).unwrap().for_relay(RAILWAY), &legacy);
}

#[test]
fn migration_never_overwrites_defaults_already_saved_for_the_home_community() {
    let dir = tempfile::tempdir().unwrap();
    let saved = defaults_with("ANTHROPIC_BASE_URL", "http://saved");
    save_defaults_in(dir.path(), RAILWAY, &saved).unwrap();
    write_legacy(dir.path(), &defaults_with("ANTHROPIC_BASE_URL", GATEWAY));

    migrate_legacy_global_agent_config(dir.path(), RAILWAY).unwrap();

    assert_eq!(read_store(dir.path()).unwrap().for_relay(RAILWAY), &saved);
}

#[test]
fn saving_one_community_leaves_the_others_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let a = defaults_with("ANTHROPIC_BASE_URL", GATEWAY);
    let b = defaults_with("OPENAI_API_KEY", "b-key");
    save_defaults_in(dir.path(), RAILWAY, &a).unwrap();
    let (previous, saved) = save_defaults_in(dir.path(), AWS, &b).unwrap();
    assert_eq!(previous, GlobalAgentConfig::default());
    assert_eq!(saved, b);

    let store = read_store(dir.path()).unwrap();
    assert_eq!(store.for_relay(RAILWAY), &a);
    assert_eq!(store.for_relay(AWS), &b);
    // Keyed by the community-scope normalization.
    assert_eq!(store.for_relay("WSS://Second.Example:443/"), &b);

    // Clearing a community removes only its entry.
    let (previous, _) = save_defaults_in(dir.path(), AWS, &GlobalAgentConfig::default()).unwrap();
    assert_eq!(previous, b);
    let store = read_store(dir.path()).unwrap();
    assert_eq!(store.for_relay(RAILWAY), &a);
    assert!(!store
        .communities
        .contains_key(&crate::relay::community_relay_key(AWS)));
    #[cfg(unix)]
    assert_eq!(mode(&store_path(dir.path())), 0o600);
}

#[test]
fn saving_normalizes_inherit_values() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = defaults_with("KEEP", "v");
    config.env_vars.insert("EMPTY".into(), String::new());
    config.provider = Some("  ".into());
    let (_, saved) = save_defaults_in(dir.path(), RAILWAY, &config).unwrap();
    assert!(!saved.env_vars.contains_key("EMPTY"));
    assert_eq!(saved.provider, None);
    assert_eq!(read_store(dir.path()).unwrap().for_relay(RAILWAY), &saved);
}

#[test]
fn a_record_reads_its_own_communitys_defaults() {
    let mut store = CommunityAgentDefaults::default();
    store.set_for_relay(RAILWAY, defaults_with("ANTHROPIC_BASE_URL", GATEWAY));
    store.set_for_relay(AWS, defaults_with("OPENAI_API_KEY", "b-key"));

    // The record's relay wins over whatever community is active.
    let aws_agent = record(AWS);
    assert_eq!(store.for_record(&aws_agent, RAILWAY), store.for_relay(AWS));
    assert!(!store
        .for_record(&aws_agent, RAILWAY)
        .env_vars
        .contains_key("ANTHROPIC_BASE_URL"));
    // Only an unassigned record falls back.
    assert_eq!(
        store.for_record(&record(""), RAILWAY),
        store.for_relay(RAILWAY)
    );
    // A community with nothing saved is empty, never another's.
    assert_eq!(
        store.for_record(&record("wss://third.example"), RAILWAY),
        &GlobalAgentConfig::default()
    );
}
