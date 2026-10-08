//! `buzz host run`: stay connected, publish presence and telemetry, and
//! handle control frames until forgotten or signalled.

use std::time::{Duration, Instant};

use buzz_core::kind::KIND_AGENT_OBSERVER_FRAME;
use buzz_ws_client::{NostrWsConnection, RelayMessage, WsClientError};
use nostr::{Event, Keys, PublicKey};
use serde_json::json;

use crate::control::{seal_telemetry, Host, Reply, FRESHNESS_SECS};
use crate::error::{HostError, Result};
use crate::store::{self, HostPaths};
use crate::supervisor::Supervisor;

/// Presence (kind 20001) refresh interval; the relay TTL is 180 s.
pub const PRESENCE_INTERVAL: Duration = Duration::from_secs(60);
/// Unsolicited `host.status` interval.
pub const STATUS_INTERVAL: Duration = Duration::from_secs(300);
/// Supervision tick for child-process agents.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// A connection that lasted this long resets the reconnect backoff.
pub const STABLE_CONNECTION: Duration = Duration::from_secs(60);
/// Reconnect backoff floor.
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Reconnect backoff ceiling.
pub const BACKOFF_CAP: Duration = Duration::from_secs(120);

const CONTROL_SUB: &str = "buzz-host-control";

/// Capped exponential reconnect backoff with full jitter.
#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    /// Upper bound of the next delay (before jitter), at most [`BACKOFF_CAP`].
    pub fn ceiling(&self) -> Duration {
        BACKOFF_BASE
            .saturating_mul(1u32 << self.attempt.min(16))
            .min(BACKOFF_CAP)
    }

    /// Next delay: uniform in `[ceiling/2, ceiling]`; advances the attempt.
    pub fn next_delay(&mut self) -> Duration {
        let ceiling = self.ceiling();
        self.attempt = self.attempt.saturating_add(1);
        let half = ceiling / 2;
        let jitter_ms = rand::random::<u64>() % (half.as_millis() as u64 + 1);
        half + Duration::from_millis(jitter_ms)
    }

    /// Reset after a stable connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// Why one connection ended.
enum Ended {
    /// Transport failure; reconnect.
    Disconnected(HostError),
    /// The host was forgotten; exit.
    Forgotten,
    /// SIGTERM / Ctrl-C; exit.
    Signalled,
}

/// The tracing filter for the daemon. `buzz_ws_client` is pinned to `info`
/// because its `debug` wire dump would include the AUTH event's NIP-OA tag.
pub fn log_filter(user: Option<&str>) -> String {
    let base = user.filter(|s| !s.trim().is_empty()).unwrap_or("info");
    format!("{base},buzz_ws_client=info,tungstenite=info,tokio_tungstenite=info")
}

/// Run the daemon until forgotten or signalled.
pub async fn run(paths: HostPaths) -> Result<()> {
    let keys = store::load_host_key(&paths)?.ok_or(HostError::NotPaired)?;
    let owner = store::load_owner(&paths)?.ok_or(HostError::NotPaired)?;
    let auth_tag = buzz_sdk::nip_oa::parse_auth_tag(&owner.auth_tag)
        .map_err(|_| HostError::Invalid("owner.json holds an invalid auth tag".into()))?;
    let relay = owner.relay_url.clone();
    let supervisor = Supervisor::detect(&paths).await?;
    let home = store::home_dir()?;
    let mut host = Host::new(
        paths.clone(),
        keys.clone(),
        owner,
        supervisor,
        crate::tools::agent_path(),
        home,
    )?;
    let agents = store::list_agents(&paths)?;
    host.supervisor.start_all(&agents).await;
    tracing::info!(host = %keys.public_key(), relay = %relay, "buzz host running");

    let mut backoff = Backoff::default();
    let result = loop {
        let started = Instant::now();
        let ended = tokio::select! {
            ended = connection(&mut host, &relay, &auth_tag) => ended,
            _ = shutdown_signal() => Ended::Signalled,
        };
        match ended {
            Ended::Forgotten => break Ok(()),
            Ended::Signalled => {
                tracing::info!("signal received; stopping");
                break Ok(());
            }
            Ended::Disconnected(e) => {
                if started.elapsed() >= STABLE_CONNECTION {
                    backoff.reset();
                }
                let delay = backoff.next_delay();
                tracing::warn!("relay connection lost: {e}; reconnecting in {delay:?}");
                let wait = supervise_for(&mut host, delay);
                tokio::select! {
                    _ = wait => {}
                    _ = shutdown_signal() => break Ok(()),
                }
            }
        }
    };
    host.supervisor.shutdown().await;
    result
}

/// Keep supervising children while waiting out a reconnect delay.
async fn supervise_for(host: &mut Host, delay: Duration) {
    let deadline = tokio::time::Instant::now() + delay;
    while tokio::time::Instant::now() < deadline {
        host.supervisor.tick().await;
        tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + TICK_INTERVAL)).await;
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn presence(keys: &Keys, status: &str) -> Result<Event> {
    buzz_sdk::build_presence_update(status)
        .map_err(|e| HostError::Relay(e.to_string()))?
        .sign_with_keys(keys)
        .map_err(|e| HostError::Relay(e.to_string()))
}

async fn send(conn: &mut NostrWsConnection, event: Event, what: &str) -> Result<()> {
    let ok = conn
        .send_event(event)
        .await
        .map_err(|e| HostError::Relay(e.to_string()))?;
    if !ok.accepted {
        // Rejections are not transport failures; report and carry on.
        tracing::warn!("relay rejected {what}: {}", ok.message);
    }
    Ok(())
}

async fn send_reply(
    conn: &mut NostrWsConnection,
    keys: &Keys,
    owner: &PublicKey,
    reply: &Reply,
) -> Result<()> {
    let event = match reply {
        Reply::Ack(ack) => seal_telemetry(keys, owner, ack)?,
        Reply::Status(status) => seal_telemetry(keys, owner, status)?,
    };
    send(conn, event, "telemetry").await
}

async fn connection(host: &mut Host, relay: &str, auth_tag: &nostr::Tag) -> Ended {
    match connection_inner(host, relay, auth_tag).await {
        Ok(ended) => ended,
        Err(e) => Ended::Disconnected(e),
    }
}

async fn connection_inner(host: &mut Host, relay: &str, auth_tag: &nostr::Tag) -> Result<Ended> {
    let keys = host.keys.clone();
    let owner = host.owner_pk;
    let rerr = |e: WsClientError| HostError::Relay(e.to_string());
    let mut conn = NostrWsConnection::connect_authenticated(relay, &keys, Some(auth_tag))
        .await
        .map_err(rerr)?;
    let since = store::now_secs().saturating_sub(FRESHNESS_SECS);
    conn.send_raw(&json!(["REQ", CONTROL_SUB, {
        "kinds": [KIND_AGENT_OBSERVER_FRAME],
        "#p": [keys.public_key().to_hex()],
        "since": since,
    }]))
    .await
    .map_err(rerr)?;
    tracing::info!("connected; listening for control frames");

    send(&mut conn, presence(&keys, "online")?, "presence").await?;
    let status = host.status(None).await;
    send_reply(&mut conn, &keys, &owner, &Reply::Status(Box::new(status))).await?;
    let mut next_presence = Instant::now() + PRESENCE_INTERVAL;
    let mut next_status = Instant::now() + STATUS_INTERVAL;

    loop {
        let now = Instant::now();
        if now >= next_presence {
            send(&mut conn, presence(&keys, "online")?, "presence").await?;
            next_presence = now + PRESENCE_INTERVAL;
        }
        if now >= next_status {
            let status = host.status(None).await;
            send_reply(&mut conn, &keys, &owner, &Reply::Status(Box::new(status))).await?;
            next_status = now + STATUS_INTERVAL;
        }
        host.supervisor.tick().await;

        let wait = next_presence
            .min(next_status)
            .saturating_duration_since(Instant::now())
            .clamp(Duration::from_millis(10), TICK_INTERVAL);
        match conn.next_event(wait).await {
            Ok(RelayMessage::Event {
                subscription_id,
                event,
            }) if subscription_id == CONTROL_SUB => {
                let outcome = host.handle_event(&event, store::now_secs()).await;
                for reply in &outcome.replies {
                    send_reply(&mut conn, &keys, &owner, reply).await?;
                }
                if outcome.forgotten {
                    let _ = send(&mut conn, presence(&keys, "offline")?, "presence").await;
                    store::remove_file(&host.paths.key_file())?;
                    tracing::info!("host forgotten by its owner; exiting");
                    let _ = conn.disconnect().await;
                    return Ok(Ended::Forgotten);
                }
            }
            Ok(RelayMessage::Closed {
                subscription_id,
                message,
            }) if subscription_id == CONTROL_SUB => {
                return Err(HostError::Relay(format!(
                    "relay closed the control subscription: {message}"
                )));
            }
            Ok(RelayMessage::Notice { message }) => tracing::info!("relay notice: {message}"),
            Ok(_) => {}
            Err(WsClientError::Timeout) => {}
            Err(e) => return Err(rerr(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Delays never exceed the cap, grow until it, and reset.
    ///
    /// Mutation: drop `.min(BACKOFF_CAP)` → RED; make `reset` a no-op → RED.
    #[test]
    fn reconnect_backoff_is_bounded() {
        let mut b = Backoff::default();
        let mut last_ceiling = Duration::ZERO;
        for _ in 0..200 {
            let ceiling = b.ceiling();
            assert!(ceiling >= last_ceiling);
            let d = b.next_delay();
            assert!(d <= BACKOFF_CAP, "{d:?}");
            assert!(d >= ceiling / 2, "{d:?} < half of {ceiling:?}");
            last_ceiling = ceiling;
        }
        assert_eq!(b.ceiling(), BACKOFF_CAP);
        b.reset();
        assert_eq!(b.ceiling(), BACKOFF_BASE);
    }

    /// The wire-dumping websocket crates stay at info whatever RUST_LOG says.
    #[test]
    fn log_filter_pins_wire_crates() {
        for user in [None, Some("trace"), Some("buzz_host=debug")] {
            let f = log_filter(user);
            assert!(
                f.ends_with("buzz_ws_client=info,tungstenite=info,tokio_tungstenite=info"),
                "{f}"
            );
        }
    }
}
