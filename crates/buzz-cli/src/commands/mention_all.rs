//! `@all` channel-mention expansion for `buzz messages send`.
//!
//! `@all` notifies every **human** member of the target channel except the
//! sender. The wire contract (shared with Desktop) is:
//!
//! - one `["p", <lowercase hex>]` tag per recipient (deduped), and
//! - a marker tag `["buzz:mention-group", "all"]`.
//!
//! A member is an agent — and therefore excluded — when its kind:39002 roster
//! role is `bot` **or** its latest kind:0 profile carries a valid NIP-OA owner
//! attestation (`auth` tag). This mirrors Desktop's `is_agent` derivation in
//! `desktop/src-tauri/src/commands/channels.rs` (`role == "bot" ||
//! profile_has_valid_oa_owner`).
//!
//! Expansion never truncates: more than [`MENTION_CAP`] qualifying humans is a
//! usage error naming the count and the limit.
//!
//! Everything here is pure; the relay fetches live in `messages.rs`.

use std::collections::{HashMap, HashSet};

use buzz_sdk::mentions::MENTION_CAP;
use nostr::{PublicKey, Tag};

use crate::error::CliError;

/// Tag name of the group-mention marker.
pub(crate) const MENTION_GROUP_TAG: &str = "buzz:mention-group";
/// Marker value (and reserved `@` token) for the all-humans group mention.
pub(crate) const MENTION_GROUP_ALL: &str = "all";

/// One entry of a kind:39002 membership roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosterMember {
    /// Canonical lowercase hex pubkey.
    pub pubkey: String,
    /// NIP-29 role from the `p` tag's fourth element (`member` when absent).
    pub role: String,
}

/// Parse `["p", pubkey, relay?, role?]` tags of a kind:39002 event.
///
/// Invalid pubkeys are skipped and duplicates collapse to their first entry,
/// matching Desktop's `channel_members_from_event`.
pub(crate) fn parse_member_roster(event: &serde_json::Value) -> Vec<RosterMember> {
    let Some(tags) = event.get("tags").and_then(|t| t.as_array()) else {
        return vec![];
    };
    let mut seen = HashSet::new();
    let mut roster = Vec::new();
    for tag in tags.iter().filter_map(|t| t.as_array()) {
        if tag.first().and_then(|v| v.as_str()) != Some("p") {
            continue;
        }
        let Some(pubkey) = tag
            .get(1)
            .and_then(|v| v.as_str())
            .and_then(|pk| PublicKey::from_hex(pk).ok())
            .map(|pk| pk.to_hex())
        else {
            continue;
        };
        if !seen.insert(pubkey.clone()) {
            continue;
        }
        let role = tag
            .get(3)
            .and_then(|v| v.as_str())
            .filter(|r| !r.is_empty())
            .unwrap_or("member")
            .to_string();
        roster.push(RosterMember { pubkey, role });
    }
    roster
}

/// Split the reserved `all` token out of extracted (lowercased) `@names`.
///
/// `@all` is reserved: it always means the group mention, never a member
/// whose display name is "all" (address such a member with `--mention`).
/// Returns `(had_all_token, remaining_names)`.
pub(crate) fn split_mention_all(names: Vec<String>) -> (bool, Vec<String>) {
    let before = names.len();
    let rest: Vec<String> = names
        .into_iter()
        .filter(|n| n != MENTION_GROUP_ALL)
        .collect();
    (rest.len() != before, rest)
}

/// Whether a kind:39000 channel-metadata event describes a DM.
///
/// Prefers the explicit `["t", type]` tag; falls back to the NIP-29 `hidden`
/// tag, matching Desktop's `channel_info_from_event`.
pub(crate) fn channel_metadata_is_dm(event: &serde_json::Value) -> bool {
    let tags: Vec<&Vec<serde_json::Value>> = event
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|tags| tags.iter().filter_map(|t| t.as_array()).collect())
        .unwrap_or_default();
    let name_is = |tag: &Vec<serde_json::Value>, name: &str| {
        tag.first().and_then(|v| v.as_str()) == Some(name)
    };
    match tags.iter().find(|t| name_is(t, "t")) {
        Some(t) => t.get(1).and_then(|v| v.as_str()) == Some("dm"),
        None => tags.iter().any(|t| name_is(t, "hidden")),
    }
}

/// Pubkeys whose **latest** kind:0 profile carries a valid NIP-OA owner
/// attestation. Unparseable or unsigned events are ignored.
pub(crate) fn agent_pubkeys_from_profiles(events: &[serde_json::Value]) -> HashSet<String> {
    let mut latest: HashMap<String, nostr::Event> = HashMap::new();
    for raw in events {
        let Ok(event) = serde_json::from_value::<nostr::Event>(raw.clone()) else {
            continue;
        };
        let pubkey = event.pubkey.to_hex();
        let newer = latest
            .get(&pubkey)
            .is_none_or(|prev| event.created_at > prev.created_at);
        if newer {
            latest.insert(pubkey, event);
        }
    }
    latest
        .into_iter()
        .filter(|(_, event)| profile_has_valid_oa_owner(event))
        .map(|(pubkey, _)| pubkey)
        .collect()
}

/// Port of Desktop's `profile_valid_oa_owner_pubkey`: a valid kind:0 with
/// exactly one well-formed `auth` tag whose owner signature verifies and whose
/// every condition applies to the profile event itself.
fn profile_has_valid_oa_owner(event: &nostr::Event) -> bool {
    if event.kind != nostr::Kind::Metadata {
        return false;
    }
    let mut auth_tags = event
        .tags
        .iter()
        .map(Tag::as_slice)
        .filter(|parts| parts.first().map(String::as_str) == Some("auth"));
    let Some(auth_tag) = auth_tags.next() else {
        return false;
    };
    if auth_tags.next().is_some() {
        return false;
    }
    let Ok(json) = serde_json::to_string(auth_tag) else {
        return false;
    };
    if buzz_sdk::nip_oa::parse_auth_tag(&json).is_err() || event.verify().is_err() {
        return false;
    }
    if buzz_sdk::nip_oa::verify_auth_tag(&json, &event.pubkey).is_err() {
        return false;
    }
    let Some(conditions) = auth_tag.get(2) else {
        return false;
    };
    conditions.is_empty()
        || conditions.split('&').all(|clause| {
            if let Some(value) = clause.strip_prefix("kind=") {
                value.parse::<u16>() == Ok(event.kind.as_u16())
            } else if let Some(value) = clause.strip_prefix("created_at<") {
                value
                    .parse::<u64>()
                    .is_ok_and(|bound| event.created_at.as_secs() < bound)
            } else if let Some(value) = clause.strip_prefix("created_at>") {
                value
                    .parse::<u64>()
                    .is_ok_and(|bound| event.created_at.as_secs() > bound)
            } else {
                false
            }
        })
}

/// Expand `@all` to the human members of `roster`, in roster order.
///
/// Excludes `sender`, every `bot`-role member, and every pubkey in `agents`.
/// More than [`MENTION_CAP`] qualifying humans is a usage error — the list is
/// never truncated.
pub(crate) fn expand_mention_all(
    roster: &[RosterMember],
    agents: &HashSet<String>,
    sender: &str,
) -> Result<Vec<String>, CliError> {
    let sender = sender.to_ascii_lowercase();
    let humans: Vec<String> = roster
        .iter()
        .filter(|m| m.role != "bot" && m.pubkey != sender && !agents.contains(&m.pubkey))
        .map(|m| m.pubkey.clone())
        .collect();
    if humans.len() > MENTION_CAP {
        return Err(CliError::Usage(format!(
            "@all would mention {} human members, exceeding the limit of {MENTION_CAP}; \
             mention people individually with --mention <pubkey> instead",
            humans.len()
        )));
    }
    Ok(humans)
}

/// Rejection for `@all` in a DM: every participant is already notified.
pub(crate) fn dm_rejection() -> CliError {
    CliError::Usage(
        "@all is not supported in DMs — every participant is already notified; \
         remove @all / --mention-all"
            .into(),
    )
}

/// The `["buzz:mention-group", "all"]` marker tag.
pub(crate) fn mention_all_marker_tag() -> Result<Tag, CliError> {
    Tag::parse([MENTION_GROUP_TAG, MENTION_GROUP_ALL])
        .map_err(|e| CliError::Other(format!("invalid mention-group tag: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};
    use serde_json::json;

    fn pk(n: u8) -> String {
        Keys::parse(&format!("{:064x}", u64::from(n) + 1))
            .expect("valid secret")
            .public_key()
            .to_hex()
    }

    fn member(pubkey: &str, role: &str) -> RosterMember {
        RosterMember {
            pubkey: pubkey.to_string(),
            role: role.to_string(),
        }
    }

    #[test]
    fn roster_parses_roles_defaults_member_and_dedupes() {
        let (a, b) = (pk(1), pk(2));
        let event = json!({"tags": [
            ["d", "chan"],
            ["p", a, "", "owner"],
            ["p", b],
            ["p", a, "", "bot"],
            ["p", "not-a-pubkey", "", "member"],
            ["e", pk(3)]
        ]});
        assert_eq!(
            parse_member_roster(&event),
            vec![member(&a, "owner"), member(&b, "member")]
        );
    }

    #[test]
    fn split_mention_all_detects_and_removes_reserved_token() {
        let (had, rest) = split_mention_all(vec!["alice".into(), "all".into(), "bob".into()]);
        assert!(had);
        assert_eq!(rest, vec!["alice".to_string(), "bob".to_string()]);

        let (had, rest) = split_mention_all(vec!["all-hands".into(), "alice".into()]);
        assert!(!had, "@all-hands is not the group token");
        assert_eq!(rest.len(), 2);
    }

    #[test]
    fn expansion_excludes_sender_bots_and_attested_agents() {
        let (sender, human, bot, agent) = (pk(1), pk(2), pk(3), pk(4));
        let roster = vec![
            member(&sender, "owner"),
            member(&human, "member"),
            member(&bot, "bot"),
            member(&agent, "member"),
        ];
        let agents: HashSet<String> = [agent.clone()].into_iter().collect();
        assert_eq!(
            expand_mention_all(&roster, &agents, &sender.to_ascii_uppercase()).unwrap(),
            vec![human]
        );
    }

    #[test]
    fn expansion_allows_exactly_cap_and_rejects_cap_plus_one_with_count() {
        let sender = pk(200);
        let mut roster: Vec<RosterMember> = (0..MENTION_CAP as u8)
            .map(|i| member(&pk(i), "member"))
            .collect();
        roster.push(member(&sender, "member"));
        roster.push(member(&pk(150), "bot"));
        assert_eq!(
            expand_mention_all(&roster, &HashSet::new(), &sender)
                .unwrap()
                .len(),
            MENTION_CAP
        );

        roster.push(member(&pk(100), "member"));
        let err = expand_mention_all(&roster, &HashSet::new(), &sender).unwrap_err();
        let CliError::Usage(msg) = &err else {
            panic!("expected usage error, got {err:?}");
        };
        assert!(msg.contains("51 human members"), "{msg}");
        assert!(msg.contains(&format!("limit of {MENTION_CAP}")), "{msg}");
        assert_eq!(crate::error::exit_code(&err), 1);
    }

    #[test]
    fn dm_detection_prefers_type_tag_then_hidden() {
        assert!(channel_metadata_is_dm(&json!({"tags": [["t", "dm"]]})));
        assert!(!channel_metadata_is_dm(
            &json!({"tags": [["t", "stream"], ["hidden"]]})
        ));
        assert!(channel_metadata_is_dm(&json!({"tags": [["hidden"]]})));
        assert!(!channel_metadata_is_dm(
            &json!({"tags": [["t", "forum"], ["public"]]})
        ));
        assert!(!channel_metadata_is_dm(&json!({})));
    }

    fn profile(keys: &Keys, tags: Vec<Tag>, created_at: u64) -> serde_json::Value {
        let event = EventBuilder::new(Kind::Metadata, r#"{"name":"x"}"#)
            .tags(tags)
            .custom_created_at(nostr::Timestamp::from(created_at))
            .sign_with_keys(keys)
            .expect("sign");
        serde_json::to_value(event).expect("serialize")
    }

    fn auth_tag_for(agent: &Keys) -> Tag {
        let owner = Keys::generate();
        let json =
            buzz_sdk::nip_oa::compute_auth_tag(&owner, &agent.public_key(), "").expect("auth tag");
        let parts: Vec<String> = serde_json::from_str(&json).expect("auth json");
        Tag::parse(parts).expect("tag")
    }

    #[test]
    fn agent_detection_requires_valid_oa_on_latest_profile() {
        let agent = Keys::generate();
        let human = Keys::generate();
        let forged = Keys::generate();
        let reformed = Keys::generate();

        // Forged: auth tag signed for a different agent pubkey.
        let forged_tag = auth_tag_for(&Keys::generate());
        let events = vec![
            profile(&agent, vec![auth_tag_for(&agent)], 100),
            profile(&human, vec![], 100),
            profile(&forged, vec![forged_tag], 100),
            // Newer plain profile supersedes an older attested one, regardless
            // of relay result order.
            profile(&reformed, vec![], 200),
            profile(&reformed, vec![auth_tag_for(&reformed)], 100),
        ];
        let agents = agent_pubkeys_from_profiles(&events);
        assert_eq!(
            agents,
            [agent.public_key().to_hex()]
                .into_iter()
                .collect::<HashSet<_>>()
        );
    }

    #[test]
    fn agent_detection_rejects_tampered_profile_signature() {
        let agent = Keys::generate();
        let mut raw = profile(&agent, vec![auth_tag_for(&agent)], 100);
        raw["content"] = json!(r#"{"name":"tampered"}"#);
        assert!(agent_pubkeys_from_profiles(&[raw]).is_empty());
    }

    #[test]
    fn marker_tag_shape() {
        assert_eq!(
            mention_all_marker_tag().unwrap().as_slice(),
            ["buzz:mention-group", "all"]
        );
    }
}
