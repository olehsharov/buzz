//! The persona edit command surface: `update_persona` (best-effort enqueue)
//! and the `update_persona_with` seam that `update_persona_and_publish` reuses
//! to await relay acceptance for the same save.

use tauri::AppHandle;

use crate::{
    app_state::AppState,
    managed_agents::{
        apply_persona_behavior, effective_agent_command, load_managed_agents, load_personas,
        managed_agent_avatar_url, save_managed_agents, save_personas, try_regenerate_nest,
        validate_agent_definition_text, AgentDefinition, ManagedAgentRecord, UpdatePersonaRequest,
    },
    util::now_iso,
};

use super::{normalize_description, pending, retain_persona_pending, trim_optional, trim_required};

mod access_propagation;
#[cfg(test)]
mod name_propagation_tests;

/// Return value of the `update_persona` command. Uses flatten so all
/// `AgentDefinition` fields appear at the top level of the JSON response —
/// backward-compatible with callers that already destructure a raw persona object.
#[derive(Debug, serde::Serialize)]
pub struct UpdatePersonaResult {
    #[serde(flatten)]
    persona: AgentDefinition,
}

/// Propagate a persona definition's display_name rename to linked agent instances.
/// Only instances whose current `name` equals `old_display_name` are updated;
/// pool-named instances (e.g. "Birch", "Compass") keep their individualised name.
/// Updates both `record.name` (relay display name) and `record.display_name`.
/// Returns the pubkeys of the records that were renamed.
fn propagate_persona_name_rename(
    records: &mut [ManagedAgentRecord],
    persona_id: &str,
    old_display_name: &str,
    new_display_name: &str,
) -> Vec<String> {
    let mut renamed = Vec::new();
    for record in records.iter_mut() {
        if record.persona_id.as_deref() != Some(persona_id) {
            continue;
        }
        if record.name != old_display_name {
            continue; // pool-named instance — keep its individualised name
        }
        record.name = new_display_name.to_string();
        record.display_name = Some(new_display_name.to_string());
        renamed.push(record.pubkey.clone());
    }
    renamed
}

#[derive(Debug, PartialEq, Eq)]
struct LinkedProfileUpdate {
    /// Whether this update changed bytes in the managed-agent record.
    record_changed: bool,
    /// Whether this instance needs a complete kind:0 replacement event.
    profile_sync_required: bool,
    /// Avatar to publish with the complete kind:0 replacement event.
    profile_avatar: Option<String>,
}

/// Apply the persisted portion of a persona identity edit to one linked
/// instance and resolve the avatar for the complete kind:0 replacement.
///
/// Description-only edits deliberately leave the record unchanged, but still
/// need a non-empty avatar projection for legacy records whose `avatar_url`
/// has not yet been backfilled. The persona avatar is authoritative there;
/// the effective command icon is the final fallback.
fn prepare_linked_profile_update(
    record: &mut ManagedAgentRecord,
    persona: &AgentDefinition,
    renamed: bool,
    avatar_changed: bool,
    about_changed: bool,
) -> LinkedProfileUpdate {
    let mut record_changed = renamed;
    if avatar_changed {
        let effective_cmd = effective_agent_command(
            record.persona_id.as_deref(),
            std::slice::from_ref(persona),
            record.agent_command_override.as_deref(),
        );
        record.avatar_url = persona
            .avatar_url
            .clone()
            .or_else(|| managed_agent_avatar_url(&effective_cmd));
        record_changed = true;
    }

    let effective_cmd = effective_agent_command(
        record.persona_id.as_deref(),
        std::slice::from_ref(persona),
        record.agent_command_override.as_deref(),
    );
    let profile_avatar = record
        .avatar_url
        .clone()
        .or_else(|| persona.avatar_url.clone())
        .or_else(|| managed_agent_avatar_url(&effective_cmd));

    LinkedProfileUpdate {
        record_changed,
        profile_sync_required: record_changed || about_changed,
        profile_avatar,
    }
}

/// Phase 1 of a persona edit: the saved definition, the caller's retained
/// publication, linked profile syncs, and whether an access change was queued
/// for its instances.
type PersonaSaved<R> = (AgentDefinition, R, ProfileSyncParams, bool);

/// Profile sync params collected under the store lock for async relay publish:
/// (agent keys, relay url, display name, avatar url, kind:0 about, auth tag).
type ProfileSyncParams = Vec<(
    nostr::Keys,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
)>;

/// Apply every queued template access change in the background, then tell
/// the Agents page to refresh. Each change selects its instances when it runs.
fn spawn_access_change_runner(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        access_propagation::ACCESS_CHANGES
            .drain(|change| {
                let app = app.clone();
                async move { apply_access_change(&app, change).await }
            })
            .await;
        let _ = tauri::Emitter::emit(&app, "agents-data-changed", ());
    });
}

/// Select the instances `change` moves, from the store as it is now.
fn plan_access_change(
    app: &AppHandle,
    change: &access_propagation::AccessChange,
) -> Result<Option<access_propagation::AccessPropagation>, String> {
    use tauri::Manager;

    let state = app.state::<AppState>();
    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    Ok(access_propagation::plan_access_propagation(
        &load_managed_agents(app)?,
        &change.definition_id,
        Some(change.previous.clone()),
        Some(change.next.clone()),
    ))
}

async fn apply_access_change(app: &AppHandle, change: access_propagation::AccessChange) {
    match plan_access_change(app, &change) {
        Ok(Some(propagation)) => {
            apply_access_propagation(app, &change.community_relay, &propagation).await;
        }
        Ok(None) => {}
        Err(error) => eprintln!(
            "buzz-desktop: applying the access change of agent definition {} failed: {error}",
            change.definition_id
        ),
    }
}

/// Move each instance that still runs the template's previous access policy
/// to the new one through the instance edit path (`update_managed_agent`):
/// a running local agent restarts with the new gate, a deployed remote agent
/// is redeployed (pending and retried if that fails), and kind:30177 is
/// republished. Each edit is fenced on the previous policy, so an instance
/// whose access changed meanwhile is left alone.
///
/// The template edit is already saved, so a failure here does not fail it:
/// every failure leaves durable state on the instance (its saved policy, and
/// for a remote agent the pending redeploy plus `last_error`) and is logged.
async fn apply_access_propagation(
    app: &AppHandle,
    community_relay: &str,
    propagation: &access_propagation::AccessPropagation,
) {
    use tauri::Manager;

    let state = app.state::<AppState>();
    for pubkey in &propagation.pubkeys {
        match crate::commands::update_managed_agent_scoped(
            propagation.request_for(pubkey),
            app.clone(),
            &state,
            community_relay,
            Some(propagation.previous.clone()),
        )
        .await
        {
            Ok(_) => {}
            Err(error) if error == crate::commands::ACCESS_PRECONDITION_FAILED => {}
            Err(error) => eprintln!(
                "buzz-desktop: applying the template access change to agent {pubkey} failed: {error}"
            ),
        }
    }
}

#[tauri::command]
pub async fn update_persona(
    input: UpdatePersonaRequest,
    app: AppHandle,
    relay: crate::window_relay::WindowRelay,
) -> Result<UpdatePersonaResult, String> {
    let community_relay = relay.ws_url().to_string();
    let retain_relay = community_relay.clone();
    let (persona, ()) =
        update_persona_with(input, app, community_relay, move |app, state, persona| {
            retain_persona_pending(app, state, &retain_relay, persona);
            // F2: immediately refresh any shared 30178 heads that include this
            // persona as a member. Best-effort inside retain so a hiccup cannot
            // fail the persona edit itself.
            crate::commands::refresh_team_catalog_heads_for_persona(
                app,
                state,
                &retain_relay,
                &persona.id,
            );
            Ok(())
        })
        .await?;
    Ok(UpdatePersonaResult { persona })
}

/// Save an edited persona, hand the saved record to `retain` while the store
/// lock is still held, then sync the relay profiles of linked agent instances.
///
/// `retain` is the only difference between the two update commands:
/// [`update_persona`] enqueues best-effort, while
/// [`sharing::update_persona_and_publish`] prepares a strict publication and
/// returns the event so the caller can await relay acceptance.
pub(super) async fn update_persona_with<R: Send + 'static>(
    input: UpdatePersonaRequest,
    app: AppHandle,
    // The invoking window's community: its retention scope holds the
    // definition's sharing state.
    community_relay: String,
    retain: impl FnOnce(&AppHandle, &AppState, &AgentDefinition) -> Result<R, String> + Send + 'static,
) -> Result<(AgentDefinition, R), String> {
    use tauri::Manager;

    // Phase 1: synchronous save (persona record + linked agent avatar updates)
    let (result, retained, profile_sync_params, access_change_queued) =
        tokio::task::spawn_blocking({
            let app = app.clone();
            move || -> Result<PersonaSaved<R>, String> {
                let state = app.state::<AppState>();
                let display_name = trim_required(&input.display_name, "Display name")?;
                let system_prompt = input.system_prompt.clone();
                validate_agent_definition_text(&display_name, &system_prompt)?;
                let description = normalize_description(input.description)?;
                let avatar_url = trim_optional(input.avatar_url);
                let acp_command = trim_optional(input.acp_command);
                let runtime = trim_optional(input.runtime);
                let model = trim_optional(input.model);
                let provider = trim_optional(input.provider);

                let _store_guard = state
                    .managed_agents_store_lock
                    .lock()
                    .map_err(|error| error.to_string())?;
                let mut personas = load_personas(&app)?;
                pending::project_active_persona_sharing(
                    &app,
                    &state,
                    &community_relay,
                    &mut personas,
                );
                let persona = personas
                    .iter_mut()
                    .find(|record| record.id == input.id)
                    .ok_or_else(|| format!("agent {} not found", input.id))?;

                // Track what changed so we can propagate to linked agent records.
                let avatar_changed = persona.avatar_url != avatar_url;
                let name_changed = persona.display_name != display_name;
                let old_display_name = persona.display_name.clone();
                // The kind:0 `about` is the authored description, so a
                // description edit changes what should be published.
                let old_about = crate::managed_agents::effective_agent_description(
                    persona.description.as_deref(),
                );
                let new_about =
                    crate::managed_agents::effective_agent_description(description.as_deref());
                let about_changed = old_about != new_about;

                persona.display_name = display_name;
                persona.avatar_url = avatar_url;
                persona.description = description;
                persona.system_prompt = system_prompt;
                persona.acp_command = acp_command;
                persona.runtime = runtime;
                persona.model = model;
                persona.provider = provider;
                persona.name_pool = input
                    .name_pool
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(env_vars) = input.env_vars {
                    crate::managed_agents::validate_user_env_keys(&env_vars)?;
                    persona.env_vars = env_vars;
                }
                let previous_access = access_propagation::definition_access_policy(persona);
                apply_persona_behavior(persona, input.behavior)?;
                let next_access = access_propagation::definition_access_policy(persona);
                persona.updated_at = now_iso();

                let result = persona.clone();
                save_personas(&app, &personas)?;

                let retained = retain(&app, &state, &result)?;
                try_regenerate_nest(&app);

                // If the avatar, display_name, or effective description changed,
                // propagate to linked agent records and collect relay profile sync
                // params for the async phase. An about-only change touches no
                // record bytes but still republishes each linked kind:0 profile.
                let sync_params: ProfileSyncParams =
                    if avatar_changed || name_changed || about_changed {
                        let mut records = load_managed_agents(&app)?;
                        let mut params: ProfileSyncParams = Vec::new();
                        let mut agents_modified = false;
                        let workspace_relay = crate::relay::relay_ws_url_with_override(&state);

                        // Propagate the display_name rename to instances that still
                        // carry the old definition display_name (pool-named instances
                        // keep their individualised name) in one pass; the loop below
                        // only decides which records need a relay profile sync.
                        let renamed: Vec<String> = if name_changed {
                            propagate_persona_name_rename(
                                &mut records,
                                &result.id,
                                &old_display_name,
                                &result.display_name,
                            )
                        } else {
                            Vec::new()
                        };

                        for record in records.iter_mut() {
                            if record.persona_id.as_deref() != Some(&result.id) {
                                continue;
                            }
                            let was_renamed = renamed.contains(&record.pubkey);
                            let update = prepare_linked_profile_update(
                                record,
                                &result,
                                was_renamed,
                                avatar_changed,
                                about_changed,
                            );

                            agents_modified = agents_modified || update.record_changed;
                            if update.profile_sync_required {
                                if let Ok(agent_keys) = nostr::Keys::parse(&record.private_key_nsec)
                                {
                                    let relay_url = crate::relay::effective_agent_relay_url(
                                        &record.relay_url,
                                        &workspace_relay,
                                    );
                                    params.push((
                                        agent_keys,
                                        relay_url,
                                        record.name.clone(),
                                        update.profile_avatar,
                                        new_about.clone(),
                                        record.auth_tag.clone(),
                                    ));
                                }
                            }
                        }

                        if agents_modified {
                            save_managed_agents(&app, &records)?;
                            // Keep retained kind:30177 identity records in lockstep with
                            // the rename (#2423): `record.name` is part of the published
                            // identity projection, so skipping this strands the relay on
                            // the stale name→pubkey binding until the next boot reconcile.
                            // Avatar-only edits are excluded — the avatar is not in the
                            // projection, so retaining would be a guaranteed no-op.
                            for record in records.iter().filter(|r| renamed.contains(&r.pubkey)) {
                                crate::commands::agents::retain_managed_agent_pending(
                                    &app, &state, record,
                                );
                            }
                        }

                        params
                    } else {
                        Vec::new()
                    };

                // Instances still on the template's previous access policy
                // follow the edit in the background. Queued last, while the store
                // lock is held, so changes apply in save order.
                let access_change = access_propagation::access_change(
                    &result.id,
                    &community_relay,
                    previous_access,
                    next_access,
                );
                let access_change_queued = access_change.is_some();
                if let Some(change) = access_change {
                    access_propagation::ACCESS_CHANGES.push(change);
                }

                Ok((result, retained, sync_params, access_change_queued))
            }
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))??;

    // The template is saved; its instances follow without holding the Save
    // open (a redeploy can take minutes). Every failure is left on the
    // instance (pending redeploy + `last_error`) and logged.
    if access_change_queued {
        spawn_access_change_runner(app.clone());
    }

    // Phase 2: await relay profile sync for linked agents whose avatar,
    // display_name, or effective description (kind:0 about) was just
    // updated. We await (rather than fire-and-forget)
    // so the frontend cache invalidation that follows the mutation settlement
    // sees the fresh relay profile. Best-effort — failures are logged, not surfaced.
    if !profile_sync_params.is_empty() {
        let state = app.state::<AppState>();
        for (agent_keys, relay_url, display_name, avatar_url, about, auth_tag) in
            profile_sync_params
        {
            if let Err(e) = crate::relay::sync_managed_agent_profile(
                &state,
                &relay_url,
                &agent_keys,
                &display_name,
                avatar_url.as_deref(),
                about.as_deref(),
                auth_tag.as_deref(),
            )
            .await
            {
                eprintln!("buzz-desktop: relay profile sync failed after persona update: {e}");
            }
        }
    }

    Ok((result, retained))
}
