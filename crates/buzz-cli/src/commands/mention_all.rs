//! `@all` channel-mention expansion for `buzz messages send`.
//!
//! `@all` notifies every member of the target channel except the sender —
//! people and agents (`bot`-role or NIP-OA-attested members) alike. The wire
//! contract (shared with Desktop) is:
//!
//! - one `["p", <lowercase hex>]` tag per recipient (deduped), and
//! - a marker tag `["buzz:mention-group", "all"]`.
//!
//! Expansion never truncates: more than [`MENTION_CAP`] recipients is a usage
//! error naming the count and the limit.
//!
//! Everything here is pure; the relay fetches live in `messages.rs`.

use std::collections::HashSet;

use buzz_sdk::mentions::MENTION_CAP;
use nostr::Tag;

use crate::error::CliError;

/// Tag name of the group-mention marker.
pub(crate) const MENTION_GROUP_TAG: &str = "buzz:mention-group";
/// Marker value (and reserved `@` token) for the all-members group mention.
pub(crate) const MENTION_GROUP_ALL: &str = "all";

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

/// Expand `@all` to every member of `members` except `sender`, deduped, in
/// roster order. `members` are canonical lowercase hex pubkeys.
///
/// More than [`MENTION_CAP`] recipients is a usage error — the list is never
/// truncated.
pub(crate) fn expand_mention_all(
    members: &[String],
    sender: &str,
) -> Result<Vec<String>, CliError> {
    let sender = sender.to_ascii_lowercase();
    let mut seen = HashSet::new();
    let recipients: Vec<String> = members
        .iter()
        .filter(|pubkey| **pubkey != sender && seen.insert(pubkey.as_str()))
        .cloned()
        .collect();
    if recipients.len() > MENTION_CAP {
        return Err(CliError::Usage(format!(
            "@all would mention {} members, exceeding the limit of {MENTION_CAP}; \
             mention members individually with --mention <pubkey> instead",
            recipients.len()
        )));
    }
    Ok(recipients)
}

/// Rejection for `@all` in a DM: every participant is already notified.
pub(crate) fn dm_rejection() -> CliError {
    CliError::Usage(
        "@all is not supported in DMs — every participant is already notified; \
         remove @all / --mention-all"
            .into(),
    )
}

/// Prepend the literal `@all` token for `--mention-all` sends whose content
/// lacks it.
///
/// Uses `"@all "` normally; a leading code fence gets `"@all\n"` so the fence
/// still opens at the start of a line. Empty content becomes `"@all"`.
pub(crate) fn prepend_all_token(content: &str) -> String {
    if content.is_empty() {
        return format!("@{MENTION_GROUP_ALL}");
    }
    let separator = if content.starts_with("```") || content.starts_with("~~~") {
        '\n'
    } else {
        ' '
    };
    format!("@{MENTION_GROUP_ALL}{separator}{content}")
}

/// The `["buzz:mention-group", "all"]` marker tag.
pub(crate) fn mention_all_marker_tag() -> Result<Tag, CliError> {
    Tag::parse([MENTION_GROUP_TAG, MENTION_GROUP_ALL])
        .map_err(|e| CliError::Other(format!("invalid mention-group tag: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;
    use serde_json::json;

    fn pk(n: u8) -> String {
        Keys::parse(&format!("{:064x}", u64::from(n) + 1))
            .expect("valid secret")
            .public_key()
            .to_hex()
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
    fn expansion_includes_every_member_but_the_sender_deduped() {
        // A project channel of its owner plus agents: every agent is notified.
        let (sender, human, bot, agent) = (pk(1), pk(2), pk(3), pk(4));
        let members = vec![
            sender.clone(),
            human.clone(),
            bot.clone(),
            agent.clone(),
            human.clone(),
        ];
        assert_eq!(
            expand_mention_all(&members, &sender.to_ascii_uppercase()).unwrap(),
            vec![human, bot, agent]
        );
        assert!(expand_mention_all(std::slice::from_ref(&sender), &sender)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn expansion_allows_exactly_cap_and_rejects_cap_plus_one_with_count() {
        let sender = pk(200);
        let mut members: Vec<String> = (0..MENTION_CAP as u8).map(pk).collect();
        members.push(sender.clone());
        members.push(pk(0));
        assert_eq!(
            expand_mention_all(&members, &sender).unwrap().len(),
            MENTION_CAP
        );

        members.push(pk(100));
        let err = expand_mention_all(&members, &sender).unwrap_err();
        let CliError::Usage(msg) = &err else {
            panic!("expected usage error, got {err:?}");
        };
        assert!(msg.contains("51 members"), "{msg}");
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

    #[test]
    fn prepend_all_token_keeps_leading_fences_intact() {
        assert_eq!(prepend_all_token("standup"), "@all standup");
        assert_eq!(prepend_all_token(""), "@all");
        assert_eq!(prepend_all_token("```\nx\n```"), "@all\n```\nx\n```");
        assert_eq!(prepend_all_token("~~~\nx\n~~~"), "@all\n~~~\nx\n~~~");
    }

    #[test]
    fn marker_tag_shape() {
        assert_eq!(
            mention_all_marker_tag().unwrap().as_slice(),
            ["buzz:mention-group", "all"]
        );
    }
}
