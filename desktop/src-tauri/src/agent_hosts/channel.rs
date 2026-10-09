//! Request/response over the relay for one host control frame.
//!
//! Each exchange opens a short-lived owner-authenticated WebSocket,
//! subscribes to the host's telemetry addressed to the owner BEFORE
//! publishing (so the reply cannot slip into a gap between publish and
//! subscribe), publishes the control frame, and waits for the first
//! verified telemetry frame that echoes the request id. Nothing stays open
//! afterwards: the desktop keeps no standing control session to the host.

use std::time::Duration;

use buzz_core_pkg::kind::KIND_AGENT_OBSERVER_FRAME;
use buzz_ws_client_pkg::{NostrWsConnection, RelayMessage};
use futures_util::future::BoxFuture;
use nostr::{Event, Keys, PublicKey};

use super::frames::{parse_host_telemetry, HostTelemetry};

/// Transport seam for host control. Production uses [`RelayHostChannel`];
/// tests substitute a scripted host.
pub trait HostChannel: Send + Sync {
    /// Publish `frame` and wait at most `timeout` for `host`'s verified
    /// telemetry reply carrying `request_id`.
    fn exchange<'a>(
        &'a self,
        frame: Event,
        host: PublicKey,
        request_id: String,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<HostTelemetry, String>>;
}

impl<T: HostChannel + ?Sized> HostChannel for &T {
    fn exchange<'a>(
        &'a self,
        frame: Event,
        host: PublicKey,
        request_id: String,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<HostTelemetry, String>> {
        (**self).exchange(frame, host, request_id, timeout)
    }
}

/// Short-lived relay WebSocket per exchange, authenticated as the owner.
pub struct RelayHostChannel {
    pub relay_url: String,
    pub owner_keys: Keys,
}

const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How far back the reply subscription looks. Covers modest clock skew; the
/// request id still has to match, so older frames cannot satisfy a request.
const REPLY_LOOKBACK_SECS: u64 = 30;

impl HostChannel for RelayHostChannel {
    fn exchange<'a>(
        &'a self,
        frame: Event,
        host: PublicKey,
        request_id: String,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<HostTelemetry, String>> {
        Box::pin(async move {
            let deadline = tokio::time::Instant::now() + timeout;
            let mut conn =
                NostrWsConnection::connect_authenticated(&self.relay_url, &self.owner_keys, None)
                    .await
                    .map_err(|error| format!("could not reach the relay: {error}"))?;
            let result = exchange_on(
                &mut conn,
                &self.owner_keys,
                frame,
                host,
                &request_id,
                deadline,
            )
            .await;
            let _ = conn.disconnect().await;
            result
        })
    }
}

async fn exchange_on(
    conn: &mut NostrWsConnection,
    owner_keys: &Keys,
    frame: Event,
    host: PublicKey,
    request_id: &str,
    deadline: tokio::time::Instant,
) -> Result<HostTelemetry, String> {
    let sub_id = format!("host-{request_id}");
    let since = super::frames::now_secs().saturating_sub(REPLY_LOOKBACK_SECS);
    conn.send_raw(&serde_json::json!([
        "REQ",
        sub_id,
        {
            "kinds": [KIND_AGENT_OBSERVER_FRAME],
            "#p": [owner_keys.public_key().to_hex()],
            "authors": [host.to_hex()],
            "since": since,
        }
    ]))
    .await
    .map_err(|error| format!("could not subscribe for the machine's reply: {error}"))?;
    wait_for_eose(conn, &sub_id).await?;

    let ok = conn
        .send_event(frame)
        .await
        .map_err(|error| format!("could not send the command to the relay: {error}"))?;
    if !ok.accepted {
        return Err(format!("the relay rejected the command: {}", ok.message));
    }

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(timeout_error());
        }
        let message = match conn.next_event(remaining).await {
            Ok(message) => message,
            Err(buzz_ws_client_pkg::WsClientError::Timeout) => return Err(timeout_error()),
            Err(error) => return Err(format!("lost the relay connection: {error}")),
        };
        match message {
            RelayMessage::Event {
                subscription_id,
                event,
            } if subscription_id == sub_id => {
                if let Ok(Some(telemetry)) =
                    parse_host_telemetry(owner_keys, &host, &event, super::frames::now_secs())
                {
                    if telemetry.request_id() == Some(request_id) {
                        return Ok(telemetry);
                    }
                }
            }
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == sub_id => {
                return Err(format!(
                    "the relay closed the reply subscription: {message}"
                ));
            }
            _ => {}
        }
    }
}

async fn wait_for_eose(conn: &mut NostrWsConnection, sub_id: &str) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + SUBSCRIBE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.next_event(remaining).await {
            Ok(RelayMessage::Eose { subscription_id }) if subscription_id == sub_id => {
                return Ok(())
            }
            Ok(RelayMessage::Closed {
                subscription_id,
                message,
            }) if subscription_id == sub_id => {
                return Err(format!(
                    "the relay refused the reply subscription: {message}"
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(format!(
                    "could not subscribe for the machine's reply: {error}"
                ))
            }
        }
    }
}

pub fn timeout_error() -> String {
    "The machine did not answer in time. Check that it is online and try again.".to_string()
}
