//! Agent configuration defaults, scoped per community.
//!
//! A [`GlobalAgentConfig`] record holds one community's defaults and applies
//! to every managed agent OF THAT COMMUNITY (see [`community`]: an agent gets
//! the defaults of its own record's relay, never the active community's and
//! never another community's). For a linked instance, the definition wins over global; for a
//! definition-less (legacy) instance, the record's own fields win over
//! global. Global is always the lowest user-settable layer.
//!
//! # Precedence (low → high)
//!
//! ```text
//! baked build env  <  GLOBAL  <  definition (linked) / instance (legacy)  <  Buzz-identity
//! ```
//!
//! # Semantics
//!
//! Unlike per-agent/persona env (snapshotted at create time), global config is
//! **live-resolved at spawn/readiness/deploy** — change a global key and every
//! agent picks it up on the next restart, with no delete+respawn required.
//!
//! # Storage
//!
//! `<app-data>/agents/agent-defaults-by-community.json`, a map keyed by
//! community relay, written `0o600` via `atomic_write_json_restricted` (same
//! as the agent store). See [`community`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::managed_agents::env_vars::{
    validate_user_env_keys, DERIVED_PROVIDER_MODEL_ENV_KEYS, MAX_ENV_VALUE_BYTES,
};
use crate::managed_agents::types::{AgentDefinition, ManagedAgentRecord};

/// One community's agent configuration defaults.
///
/// Shape mirrors the per-agent/persona trio (`env_vars` + `provider` + `model`)
/// so the config vocabulary is consistent across all three tiers.
///
/// `env_vars` is the lowest user-settable env layer — global < persona < agent.
/// `provider` / `model` are fallback defaults, resolved via
/// `effective_config::resolve_effective_config`: for a linked instance,
/// definition → global (the record's own `provider`/`model` bytes are never
/// consulted); for a definition-less instance, instance → global.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalAgentConfig {
    /// Env vars injected into every agent of the community.
    ///
    /// Lowest user-settable layer — per-agent and persona values win on any
    /// key collision. Reserved and derived keys are rejected at save time and
    /// stripped at spawn time.
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,

    /// Global fallback provider (e.g. `"databricks_v2"`, `"anthropic"`).
    ///
    /// Used only when neither the agent record nor the linked persona specifies
    /// a provider. `None` = no global default.
    #[serde(default)]
    pub provider: Option<String>,

    /// Global fallback model identifier.
    ///
    /// Used only when neither the agent record nor the linked persona specifies
    /// a model. `None` = no global default.
    #[serde(default)]
    pub model: Option<String>,

    /// Preferred ACP runtime for definitions without an explicit runtime.
    #[serde(default)]
    pub preferred_runtime: Option<String>,
}

/// Validate a `GlobalAgentConfig` before persisting it.
///
/// Rules beyond `validate_user_env_keys`:
/// - `DERIVED_PROVIDER_MODEL_ENV_KEYS` (`GOOSE_PROVIDER`, `GOOSE_MODEL`, …)
///   must NOT be set as global env vars — they would shadow the structured
///   `provider`/`model` fields and break provider/model resolution. Users
///   must use the structured fields instead.
/// - Empty per-key values are stripped before validation so a caller that
///   passes `KEY=""` does not accidentally shadow a real global value.
///
/// `provider` and `model` rules (applied to `Some` values only):
/// - Interior NUL bytes are rejected (they truncate C-string env injection).
/// - Values exceeding [`MAX_ENV_VALUE_BYTES`] are rejected.
/// - Blank / whitespace-only values are normalized to `None` by
///   [`normalize_global_config_fields`], which must be called before
///   persisting (done inside [`save_agent_defaults_for_relay`]).
pub fn validate_global_config(config: &GlobalAgentConfig) -> Result<(), String> {
    // Strip empty values first — they mean "inherit" and must not be stored.
    let non_empty: BTreeMap<String, String> = config
        .env_vars
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Standard env-var key validation (POSIX shape, reserved-key check, NUL/size caps).
    validate_user_env_keys(&non_empty)?;

    // Reject derived provider/model keys in global env_vars.
    let derived: Vec<&str> = non_empty
        .keys()
        .filter(|k| {
            DERIVED_PROVIDER_MODEL_ENV_KEYS
                .iter()
                .any(|d| d.eq_ignore_ascii_case(k.as_str()))
        })
        .map(String::as_str)
        .collect();
    if !derived.is_empty() {
        return Err(format!(
            "the following keys must be set via the structured provider/model fields, \
             not as env vars: {}",
            derived.join(", ")
        ));
    }

    // Validate the structured provider and model fields.
    for (field, value) in [("provider", &config.provider), ("model", &config.model)] {
        if let Some(v) = value {
            // Reject interior NUL bytes — they truncate C-string env injection.
            if v.contains('\0') {
                return Err(format!(
                    "global config `{field}` must not contain NUL bytes"
                ));
            }
            // Size cap: match the per-value env-var cap.
            if v.len() > MAX_ENV_VALUE_BYTES {
                return Err(format!(
                    "global config `{field}` exceeds the maximum allowed length \
                     ({} bytes)",
                    MAX_ENV_VALUE_BYTES
                ));
            }
            // Note: blank/whitespace-only values are normalized to None by
            // normalize_global_config_fields, called from every save path.
        }
    }

    Ok(())
}

/// Strip empty values from `env_vars`.
///
/// Empty per-agent/persona values mean "no value"; if stored they would shadow
/// a real global default with an empty string. Strip them at save time so a
/// caller that clears a row cannot accidentally shadow global.
pub fn strip_empty_env_vars(config: &mut GlobalAgentConfig) {
    config.env_vars.retain(|_, v| !v.is_empty());
}

/// Normalize `provider` and `model` to `None` when blank or whitespace-only.
///
/// `Some("")` and `Some("  ")` have no meaningful value and break
/// unset/fallback semantics (a blank provider would be treated as "provider
/// explicitly set to nothing" rather than "inherit"). Normalizing to `None`
/// preserves the invariant that `Some(s)` always contains a non-blank string.
///
/// Called from every save path so normalization is applied at
/// every persist boundary.
pub fn normalize_global_config_fields(config: &mut GlobalAgentConfig) {
    if let Some(v) = &config.provider {
        if v.trim().is_empty() {
            config.provider = None;
        }
    }
    if let Some(v) = &config.model {
        if v.trim().is_empty() {
            config.model = None;
        }
    }
}

/// Resolve the effective model and provider for an agent.
///
/// Delegates to `effective_config::resolve_effective_config` which enforces
/// definition-authoritative semantics for linked instances:
///   - **Linked:** definition → global. Record bytes are never consulted.
///   - **Definition-less:** instance → global.
///   - **Orphaned:** returns `(None, None)`. This function is a display/
///     readiness/hash convenience, not the spawn gate — an orphan must never
///     reach a real spawn regardless of what this returns, because
///     `spawn_agent_child` refuses orphans itself via
///     `effective_config::resolve_effective_config(...).require_resolved()`
///     before resolving model/provider/prompt for the process env.
///
/// Returns owned `String`s because the effective value may come from a
/// tier that outlives none of the input borrows (e.g. global config).
pub(crate) fn resolve_effective_model_provider(
    record: &ManagedAgentRecord,
    personas: &[AgentDefinition],
    global: &GlobalAgentConfig,
) -> (Option<String>, Option<String>) {
    super::effective_config::resolve_effective_model_provider_pair(record, personas, global)
        .unwrap_or((None, None))
}

mod community;
pub(crate) use community::migrate_legacy_global_agent_config;
pub use community::{
    load_agent_defaults_for_agent, load_agent_defaults_for_record, load_agent_defaults_for_relay,
    load_community_agent_defaults, save_agent_defaults_for_relay, CommunityAgentDefaults,
};

#[cfg(test)]
mod tests;
