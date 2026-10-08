//! Control-frame verification, dedupe and dispatch.
//!
//! [`Host::handle_event`] is the single seam between the relay loop and the
//! host's state: it verifies an incoming kind:24200 frame, decrypts it,
//! dedupes it by `request_id`, runs the command and returns the telemetry to
//! send back. It performs no network I/O, so it is tested directly.

use std::collections::BTreeMap;

use buzz_core::kind::KIND_AGENT_OBSERVER_FRAME;
use buzz_core::observer::{
    decrypt_observer_payload, encrypt_observer_payload, OBSERVER_AGENT_TAG, OBSERVER_FRAME_CONTROL,
    OBSERVER_FRAME_TAG, OBSERVER_FRAME_TELEMETRY,
};
use nostr::{Event, Keys, Kind, PublicKey};
use serde::{Deserialize, Serialize};

use crate::error::{HostError, Result};
use crate::protocol::{
    decode_control, is_pubkey_hex, Ack, Control, DeployRequest, HostStatus, TELEMETRY_STATUS,
};
use crate::store::{self, now_secs, HostPaths, OwnerRecord};
use crate::supervisor::Supervisor;

/// Control frames older or newer than this (seconds) are dropped. Matches the
/// relay's observer-frame window and buzz-acp's control freshness.
pub const FRESHNESS_SECS: u64 = 300;
/// Seen request ids are kept this long (longer than the freshness window, so
/// any frame that could still pass freshness is remembered).
pub const SEEN_RETENTION_SECS: u64 = 2 * FRESHNESS_SECS;
/// Upper bound on remembered request ids.
pub const SEEN_MAX: usize = 1024;

/// Why a frame was dropped without a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason {
    /// Bad signature or not a kind:24200 event.
    Malformed,
    /// Signed by someone other than the paired owner.
    NotOwner,
    /// Not addressed to this host as a control frame.
    WrongRoute,
    /// Outside the ±[`FRESHNESS_SECS`] window.
    Stale,
    /// Could not be decrypted or carried no usable request id.
    Undecodable,
}

fn single_tag<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    let mut values = event
        .tags
        .iter()
        .filter(|t| t.kind().to_string() == name)
        .filter_map(|t| t.content());
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

/// Check that `event` is a fresh control frame from `owner` to `host`.
pub fn verify_control_frame(
    event: &Event,
    host: &PublicKey,
    owner: &PublicKey,
    now: u64,
) -> std::result::Result<(), DropReason> {
    if event.kind != Kind::Custom(KIND_AGENT_OBSERVER_FRAME as u16) || event.verify().is_err() {
        return Err(DropReason::Malformed);
    }
    if event.pubkey != *owner {
        return Err(DropReason::NotOwner);
    }
    let host_hex = host.to_hex();
    let routed = single_tag(event, "p") == Some(host_hex.as_str())
        && single_tag(event, OBSERVER_AGENT_TAG) == Some(host_hex.as_str())
        && single_tag(event, OBSERVER_FRAME_TAG) == Some(OBSERVER_FRAME_CONTROL);
    if !routed {
        return Err(DropReason::WrongRoute);
    }
    if event.created_at.as_secs().abs_diff(now) > FRESHNESS_SECS {
        return Err(DropReason::Stale);
    }
    Ok(())
}

/// Build a signed telemetry frame from H to O carrying `payload`.
pub fn seal_telemetry<T: Serialize>(keys: &Keys, owner: &PublicKey, payload: &T) -> Result<Event> {
    let encrypted = encrypt_observer_payload(keys, owner, payload)
        .map_err(|e| HostError::Relay(format!("encrypt telemetry: {e}")))?;
    buzz_sdk::build_agent_observer_frame(
        &owner.to_hex(),
        &keys.public_key().to_hex(),
        OBSERVER_FRAME_TELEMETRY,
        &encrypted,
    )
    .map_err(|e| HostError::Relay(format!("build telemetry: {e}")))?
    .sign_with_keys(keys)
    .map_err(|e| HostError::Relay(format!("sign telemetry: {e}")))
}

/// One remembered request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeenEntry {
    /// Unix seconds when handled.
    pub at: u64,
    /// The ack sent for it, replayed on duplicates (absent for status).
    pub ack: Option<Ack>,
}

/// Bounded, persisted record of handled request ids.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SeenLedger {
    entries: BTreeMap<String, SeenEntry>,
}

impl SeenLedger {
    /// Load from disk (empty when missing or unreadable).
    pub fn load(paths: &HostPaths) -> Self {
        match store::read_json(&paths.seen_file()) {
            Ok(Some(ledger)) => ledger,
            Ok(None) => Self::default(),
            Err(e) => {
                tracing::warn!("ignoring unreadable request ledger: {e}");
                Self::default()
            }
        }
    }

    /// Look up a request id.
    pub fn get(&self, request_id: &str) -> Option<&SeenEntry> {
        self.entries.get(request_id)
    }

    /// Remember a request, pruning expired and excess entries.
    pub fn insert(&mut self, request_id: &str, entry: SeenEntry, now: u64) {
        self.entries.insert(request_id.to_string(), entry);
        self.entries
            .retain(|_, e| now.saturating_sub(e.at) <= SEEN_RETENTION_SECS);
        while self.entries.len() > SEEN_MAX {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.at)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.entries.remove(&k),
                None => break,
            };
        }
    }

    /// Number of remembered ids.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no ids are remembered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Telemetry produced by handling a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// A `host.ack`.
    Ack(Ack),
    /// A `host.status`.
    Status(Box<HostStatus>),
}

/// Result of [`Host::handle_event`].
#[derive(Debug, Default)]
pub struct Outcome {
    /// Telemetry to publish, in order.
    pub replies: Vec<Reply>,
    /// The host was forgotten: publish the replies, wipe the key, exit.
    pub forgotten: bool,
    /// Why the frame was dropped, if it was.
    pub dropped: Option<DropReason>,
}

/// The paired host: identity, owner, supervisor and request ledger.
pub struct Host {
    /// State directory.
    pub paths: HostPaths,
    /// Host key H.
    pub keys: Keys,
    /// Owner grant.
    pub owner: OwnerRecord,
    /// Owner pubkey O.
    pub owner_pk: PublicKey,
    /// Agent supervisor.
    pub supervisor: Supervisor,
    /// Request dedupe ledger.
    pub ledger: SeenLedger,
    /// Agent search PATH.
    pub path: String,
    /// Home directory for `~` expansion.
    pub home: std::path::PathBuf,
}

impl Host {
    /// Assemble a host from its parts.
    pub fn new(
        paths: HostPaths,
        keys: Keys,
        owner: OwnerRecord,
        supervisor: Supervisor,
        path: String,
        home: std::path::PathBuf,
    ) -> Result<Self> {
        let owner_pk = PublicKey::from_hex(&owner.owner_pubkey)
            .map_err(|_| HostError::Invalid("owner.json has an invalid owner_pubkey".into()))?;
        let ledger = SeenLedger::load(&paths);
        Ok(Self {
            paths,
            keys,
            owner,
            owner_pk,
            supervisor,
            ledger,
            path,
            home,
        })
    }

    /// Build a `host.status` snapshot.
    pub async fn status(&mut self, request_id: Option<String>) -> HostStatus {
        let agents = store::list_agents(&self.paths).unwrap_or_default();
        let agents = self.supervisor.states(&agents).await;
        let (claude, tools) = crate::tools::tools_status(&self.path);
        let status = HostStatus {
            kind: TELEMETRY_STATUS.into(),
            request_id,
            name: self.owner.name.clone(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            agents,
            claude,
            tools,
        };
        if let Err(e) = store::write_json(&self.paths.status_file(), &status) {
            tracing::warn!("could not write status snapshot: {e}");
        }
        status
    }

    /// Verify, decrypt, dedupe and run one control frame.
    pub async fn handle_event(&mut self, event: &Event, now: u64) -> Outcome {
        if let Err(reason) =
            verify_control_frame(event, &self.keys.public_key(), &self.owner_pk, now)
        {
            tracing::warn!(event = %event.id, ?reason, "dropping control frame");
            return Outcome {
                dropped: Some(reason),
                ..Outcome::default()
            };
        }
        let value: serde_json::Value = match decrypt_observer_payload(&self.keys, event) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(event = %event.id, "undecryptable control frame: {e}");
                return Outcome {
                    dropped: Some(DropReason::Undecodable),
                    ..Outcome::default()
                };
            }
        };
        let control = match decode_control(value) {
            Ok(c) => c,
            Err((Some(request_id), message)) => {
                tracing::warn!(request_id = %request_id, "rejecting control frame: {message}");
                let ack = Ack::new(&request_id, Err(message));
                return self.finish(&request_id, Some(ack), now);
            }
            Err((None, message)) => {
                tracing::warn!(event = %event.id, "unusable control frame: {message}");
                return Outcome {
                    dropped: Some(DropReason::Undecodable),
                    ..Outcome::default()
                };
            }
        };
        let request_id = control.request_id().to_string();

        if let Some(seen) = self.ledger.get(&request_id) {
            tracing::info!(request_id = %request_id, "duplicate control frame; replaying result");
            let replies = match &seen.ack {
                Some(ack) => vec![Reply::Ack(ack.clone())],
                None => vec![Reply::Status(Box::new(self.status(Some(request_id)).await))],
            };
            return Outcome {
                replies,
                ..Outcome::default()
            };
        }

        match control {
            Control::Deploy(req) => {
                tracing::info!(request_id = %request_id, agent = %req.agent_pubkey, "deploy");
                let result = self.deploy(&req).await.map_err(|e| e.to_string());
                if let Err(e) = &result {
                    tracing::warn!(request_id = %request_id, "deploy failed: {e}");
                }
                self.finish(&request_id, Some(Ack::new(&request_id, result)), now)
            }
            Control::Undeploy(req) => {
                tracing::info!(request_id = %request_id, agent = %req.agent_pubkey, "undeploy");
                let result = self
                    .undeploy(&req.agent_pubkey)
                    .await
                    .map_err(|e| e.to_string());
                self.finish(&request_id, Some(Ack::new(&request_id, result)), now)
            }
            Control::Status { .. } => {
                let status = self.status(Some(request_id.clone())).await;
                let mut out = self.finish(&request_id, None, now);
                out.replies.push(Reply::Status(Box::new(status)));
                out
            }
            Control::Forget { .. } => {
                tracing::info!(request_id = %request_id, "forget");
                match self.forget_local().await {
                    // The ledger was wiped with everything else; do not
                    // recreate it.
                    Ok(()) => Outcome {
                        replies: vec![Reply::Ack(Ack::new(&request_id, Ok(())))],
                        forgotten: true,
                        dropped: None,
                    },
                    Err(e) => self.finish(
                        &request_id,
                        Some(Ack::new(&request_id, Err(e.to_string()))),
                        now,
                    ),
                }
            }
        }
    }

    fn finish(&mut self, request_id: &str, ack: Option<Ack>, now: u64) -> Outcome {
        self.ledger.insert(
            request_id,
            SeenEntry {
                at: now,
                ack: ack.clone(),
            },
            now,
        );
        if let Err(e) = store::write_json(&self.paths.seen_file(), &self.ledger) {
            tracing::warn!("could not persist request ledger: {e}");
        }
        Outcome {
            replies: ack.map(Reply::Ack).into_iter().collect(),
            ..Outcome::default()
        }
    }

    /// Write the agent's config and (re)start it. Idempotent per pubkey.
    pub async fn deploy(&mut self, req: &DeployRequest) -> Result<()> {
        let record =
            crate::env::build_agent_record(req, &self.owner.owner_pubkey, &self.home, &self.path)?;
        let file = self.paths.agent_file(&record.agent_pubkey);
        let existed = file.exists();
        store::write_json(&file, &record)?;
        if let Err(e) = self.supervisor.apply(&record.agent_pubkey).await {
            if !existed {
                // A first deploy that failed must not leave a key behind.
                let _ = self.supervisor.remove(&record.agent_pubkey).await;
                store::remove_file(&file)?;
            }
            return Err(e);
        }
        Ok(())
    }

    /// Stop the agent and delete its config. Unknown agents succeed.
    pub async fn undeploy(&mut self, agent_pubkey: &str) -> Result<()> {
        if !is_pubkey_hex(agent_pubkey) {
            return Err(HostError::Invalid(
                "agent_pubkey must be 64 lowercase hex characters".into(),
            ));
        }
        self.supervisor.remove(agent_pubkey).await?;
        store::remove_file(&self.paths.agent_file(agent_pubkey))
    }

    /// Stop and remove every agent and drop the owner grant. The host key
    /// is left for the caller, which still needs it to sign the ack.
    pub async fn forget_local(&mut self) -> Result<()> {
        let mut first_err = None;
        for agent in store::list_agents(&self.paths)? {
            if let Err(e) = self.undeploy(&agent).await {
                tracing::warn!(agent = %agent, "undeploy during forget failed: {e}");
                first_err.get_or_insert(e);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        self.supervisor.shutdown().await;
        wipe_except_key(&self.paths)
    }
}

/// Remove owner, ledger, status, pids and logs (everything but `host.key`).
pub fn wipe_except_key(paths: &HostPaths) -> Result<()> {
    for file in [
        paths.owner_file(),
        paths.seen_file(),
        paths.status_file(),
        paths.root.join("pids.json"),
    ] {
        store::remove_file(&file)?;
    }
    for dir in [paths.agents_dir(), paths.logs_dir()] {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(HostError::io(format!("remove {}", dir.display()), e)),
        }
    }
    Ok(())
}

/// Seconds since the Unix epoch (re-exported for the daemon loop).
pub fn now() -> u64 {
    now_secs()
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
