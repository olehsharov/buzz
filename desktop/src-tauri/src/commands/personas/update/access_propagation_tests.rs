use super::*;
use crate::managed_agents::BackendKind;

const TEMPLATE: &str = "template-1";

fn instance(pubkey: &str, persona_id: &str, respond_to: RespondTo) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": pubkey, "name": pubkey, "relay_url": "", "acp_command": "",
        "agent_command": "", "agent_args": [], "mcp_command": "",
        "turn_timeout_seconds": 0, "system_prompt": null, "created_at": "",
        "updated_at": "", "last_started_at": null, "last_stopped_at": null,
        "last_exit_code": null, "last_error": null
    }))
    .unwrap();
    record.persona_id = Some(persona_id.to_string());
    record.respond_to = respond_to;
    record
}

fn deployed_provider(mut record: ManagedAgentRecord) -> ManagedAgentRecord {
    record.backend = BackendKind::Provider {
        id: "ssh-host".into(),
        config: serde_json::json!({}),
    };
    record.backend_agent_id = Some("deployment".into());
    record
}

fn owner_only() -> Option<AccessPolicy> {
    Some((RespondTo::OwnerOnly, Vec::new()))
}

fn anyone() -> Option<AccessPolicy> {
    Some((RespondTo::Anyone, Vec::new()))
}

#[test]
fn inheriting_local_and_provider_instances_follow_the_template_edit() {
    let selected = "f".repeat(64);
    let mut overridden = instance("overridden", TEMPLATE, RespondTo::Allowlist);
    overridden.respond_to_allowlist = vec![selected];
    let records = vec![
        instance("local", TEMPLATE, RespondTo::OwnerOnly),
        deployed_provider(instance("remote", TEMPLATE, RespondTo::OwnerOnly)),
        overridden,
        instance("other-template", "template-2", RespondTo::OwnerOnly),
    ];

    let propagation = plan_access_propagation(&records, TEMPLATE, owner_only(), anyone())
        .expect("two instances still run the template's previous policy");

    assert_eq!(propagation.pubkeys, vec!["local", "remote"]);
    assert_eq!(propagation.previous, (RespondTo::OwnerOnly, Vec::new()));
}

#[test]
fn an_individually_set_instance_is_never_selected() {
    let records = vec![instance("custom", TEMPLATE, RespondTo::Anyone)];

    assert_eq!(
        plan_access_propagation(&records, TEMPLATE, owner_only(), anyone()),
        None
    );
}

#[test]
fn an_unchanged_or_unreadable_template_policy_propagates_nothing() {
    let records = vec![instance("local", TEMPLATE, RespondTo::OwnerOnly)];

    assert_eq!(
        plan_access_propagation(&records, TEMPLATE, owner_only(), owner_only()),
        None
    );
    assert_eq!(
        plan_access_propagation(&records, TEMPLATE, None, anyone()),
        None
    );
}

#[test]
fn the_instance_edit_carries_the_template_policy() {
    let allowlist = vec!["e".repeat(64)];
    let propagation = AccessPropagation {
        previous: (RespondTo::OwnerOnly, Vec::new()),
        next: (RespondTo::Allowlist, allowlist.clone()),
        pubkeys: vec!["remote".into()],
    };

    let request = propagation.request_for("remote");
    assert_eq!(request.pubkey, "remote");
    assert_eq!(request.respond_to, Some(RespondTo::Allowlist));
    assert_eq!(request.respond_to_allowlist, Some(allowlist));
    // Nothing else about the instance is touched.
    assert!(request.name.is_none() && request.model.is_none() && request.env_vars.is_none());

    let widened = AccessPropagation {
        next: (RespondTo::Anyone, Vec::new()),
        ..propagation
    };
    assert_eq!(widened.request_for("remote").respond_to_allowlist, None);
}

#[test]
fn a_propagated_edit_to_a_deployed_provider_is_redeployed() {
    // The propagation request goes through `update_managed_agent`'s access
    // path, which plans the deployed provider's redeploy and marks it pending.
    let records = vec![deployed_provider(instance(
        "remote",
        TEMPLATE,
        RespondTo::OwnerOnly,
    ))];
    let propagation = plan_access_propagation(&records, TEMPLATE, owner_only(), anyone()).unwrap();
    let request = propagation.request_for("remote");
    let mut record = records[0].clone();

    let changed = crate::commands::managed_agent_access_policy_changed(
        record.respond_to,
        &record.respond_to_allowlist,
        request.respond_to.unwrap(),
        &[],
        false,
    );
    let transition = crate::commands::agents::access_transition::plan_access_runtime_transition(
        &mut record,
        changed,
        &std::collections::HashMap::<crate::managed_agents::ManagedAgentRuntimeKey, ()>::new(),
        "wss://relay.example",
    );

    assert!(matches!(
        transition,
        crate::commands::agents::access_transition::AccessRuntimeTransition::Redeploy(_)
    ));
    assert!(record.provider_policy_pending);
}

#[test]
fn the_definition_policy_defaults_to_owner_only_and_drops_a_stray_allowlist() {
    let mut definition: AgentDefinition = serde_json::from_value(serde_json::json!({
        "id": TEMPLATE, "display_name": "Template", "system_prompt": "",
        "created_at": "", "updated_at": ""
    }))
    .unwrap();
    assert_eq!(definition_access_policy(&definition), owner_only());

    definition.respond_to = Some("anyone".into());
    definition.respond_to_allowlist = vec!["e".repeat(64)];
    assert_eq!(definition_access_policy(&definition), anyone());

    definition.respond_to = Some("future-mode".into());
    assert_eq!(definition_access_policy(&definition), None);
}

#[test]
fn only_a_readable_policy_change_is_queued() {
    assert_eq!(
        access_change(TEMPLATE, "wss://c.example", owner_only(), owner_only()),
        None
    );
    assert_eq!(
        access_change(TEMPLATE, "wss://c.example", None, anyone()),
        None
    );
    assert_eq!(
        access_change(TEMPLATE, "wss://c.example", owner_only(), anyone()),
        Some(AccessChange {
            definition_id: TEMPLATE.into(),
            community_relay: "wss://c.example".into(),
            previous: (RespondTo::OwnerOnly, Vec::new()),
            next: (RespondTo::Anyone, Vec::new()),
        })
    );
}

/// Two quick template edits (owner-only → anyone → allowlist) queued before
/// either reaches the instances: applied in save order, each selecting its
/// instances when it runs, every inheriting instance ends on the latest
/// policy and an individually set one is untouched.
#[test]
fn queued_template_edits_apply_in_save_order_and_select_when_they_run() {
    let member = "a".repeat(64);
    let allowlist = (RespondTo::Allowlist, vec![member.clone()]);
    let queue = AccessChangeQueue::new();
    queue.push(access_change(TEMPLATE, "wss://c.example", owner_only(), anyone()).unwrap());
    queue.push(
        access_change(
            TEMPLATE,
            "wss://c.example",
            anyone(),
            Some(allowlist.clone()),
        )
        .unwrap(),
    );
    let mut custom = instance("custom", TEMPLATE, RespondTo::Allowlist);
    custom.respond_to_allowlist = vec!["b".repeat(64)];
    let records = std::cell::RefCell::new(vec![
        instance("one", TEMPLATE, RespondTo::OwnerOnly),
        deployed_provider(instance("two", TEMPLATE, RespondTo::OwnerOnly)),
        custom,
    ]);

    tauri::async_runtime::block_on(queue.drain(|change| {
        let planned = plan_access_propagation(
            &records.borrow(),
            &change.definition_id,
            Some(change.previous.clone()),
            Some(change.next.clone()),
        );
        if let Some(propagation) = planned {
            for record in records.borrow_mut().iter_mut() {
                if propagation.pubkeys.contains(&record.pubkey) {
                    record.respond_to = propagation.next.0;
                    record.respond_to_allowlist = propagation.next.1.clone();
                }
            }
        }
        std::future::ready(())
    }));

    let records = records.into_inner();
    for record in &records[..2] {
        assert_eq!(
            (record.respond_to, record.respond_to_allowlist.clone()),
            allowlist,
            "{} follows both edits",
            record.pubkey
        );
    }
    assert_eq!(records[2].respond_to_allowlist, vec!["b".repeat(64)]);
    assert!(queue.pop().is_none(), "every queued change was applied");
}
