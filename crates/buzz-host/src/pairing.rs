//! `buzz host pair`: NIP-AB target role carrying the host hello and grant.
//!
//! Sequence (both payloads are `PayloadType::Custom` in one session):
//!
//! ```text
//! Desktop (source)                     Host (target)
//! new_source → shows URI + SAS         new_target(uri) → offer
//! handle_offer → SAS                   prints SAS
//! [user clicks Approve] confirm_sas →  handle_sas_confirm → SAS
//!                                      [user confirms] confirm_target_sas
//! handle_return_payload  ←             send_return_payload(buzz-host-hello)
//! send_reply_payload(buzz-host-grant) → handle_payload
//! handle_complete        ←             send_complete
//! ```

use std::io::{BufRead, Write};
use std::time::Duration;

use buzz_core::kind::KIND_PAIRING;
use buzz_core::pairing::qr::decode_qr;
use buzz_core::pairing::session::PairingSession;
use buzz_core::pairing::types::{AbortReason, PayloadType};
use buzz_core::pairing::PairingError;
use buzz_ws_client::{NostrWsConnection, RelayMessage, WsClientError};
use nostr::{Event, EventBuilder, Keys, RelayUrl};
use serde_json::json;
use zeroize::Zeroizing;

use crate::error::{HostError, Result};
use crate::protocol::{HostGrant, HostHello};
use crate::store::{self, now_secs, HostPaths, OwnerRecord};

const SUB_ID: &str = "buzz-host-pair";
/// Overall pairing deadline (the NIP-AB session itself expires at 120 s).
const PAIRING_DEADLINE: Duration = Duration::from_secs(130);

/// How the target confirms the SAS words.
pub enum Confirm {
    /// Ask on the terminal (reads `/dev/tty` when stdin is not a terminal).
    Prompt,
    /// Accept without asking (`--yes`).
    Assume,
}

fn ws_err(e: WsClientError) -> HostError {
    HostError::Relay(e.to_string())
}

fn pair_err(e: PairingError) -> HostError {
    HostError::Pairing(e.to_string())
}

/// Pair this machine using the desktop's pairing URI. Persists `owner.json`
/// on success and returns the record.
pub async fn pair(
    paths: &HostPaths,
    uri: &str,
    relay_override: Option<&str>,
    name: &str,
    confirm: Confirm,
) -> Result<OwnerRecord> {
    let keys = store::load_or_create_host_key(paths)?;
    let mut qr = decode_qr(uri.trim()).map_err(pair_err)?;
    if let Some(relay) = relay_override {
        qr.relays = vec![relay.to_string()];
    }
    let relay = qr
        .relays
        .first()
        .cloned()
        .ok_or_else(|| HostError::Pairing("pairing URI has no relay".into()))?;
    let (mut session, offer) = PairingSession::new_target(&qr).map_err(pair_err)?;

    let run = run_target(&keys, &mut session, offer, &relay, name, confirm);
    let grant = tokio::time::timeout(PAIRING_DEADLINE, run)
        .await
        .map_err(|_| HostError::Pairing("timed out waiting for the desktop".into()))??;

    let record = OwnerRecord {
        owner_pubkey: grant.owner_pubkey.clone(),
        auth_tag: grant.auth_tag.clone(),
        relay_url: grant.relay_url.clone(),
        paired_at: now_secs(),
        name: name.to_string(),
    };
    store::write_json(&paths.owner_file(), &record)?;
    Ok(record)
}

async fn run_target(
    keys: &Keys,
    session: &mut PairingSession,
    offer: Event,
    relay: &str,
    name: &str,
    confirm: Confirm,
) -> Result<HostGrant> {
    println!("Connecting to {relay} ...");
    let mut conn = NostrWsConnection::connect(relay).await.map_err(ws_err)?;
    authenticate_ephemeral(&mut conn, session, relay).await?;

    let our_pk = session.pubkey().to_hex();
    conn.send_raw(&json!(["REQ", SUB_ID, {"kinds": [KIND_PAIRING], "#p": [our_pk]}]))
        .await
        .map_err(ws_err)?;
    wait_eose(&mut conn).await?;
    publish(&mut conn, offer).await?;

    let sas = session
        .sas_code()
        .ok_or_else(|| HostError::Pairing("no SAS code".into()))?;
    println!();
    println!("  Pairing code: {sas}");
    println!();
    println!("Check that Buzz desktop shows the same code, then click Approve there.");

    // Wait for the desktop's sas-confirm (sent after the user approves).
    loop {
        let event = next_pairing_event(&mut conn).await?;
        check_abort(session, &event)?;
        match session.handle_sas_confirm(&event) {
            Ok(_) => break,
            Err(PairingError::TranscriptMismatch) => {
                abort(&mut conn, session, AbortReason::SasMismatch).await;
                return Err(HostError::Pairing(
                    "SECURITY: transcript mismatch (possible interception); aborted".into(),
                ));
            }
            Err(_) => continue,
        }
    }

    let approved = match confirm {
        Confirm::Assume => true,
        Confirm::Prompt => ask_yes_no(&format!("Desktop approved. Does it show {sas}? [y/N]: "))?,
    };
    if !approved {
        abort(&mut conn, session, AbortReason::SasMismatch).await;
        return Err(HostError::Pairing(
            "pairing code not confirmed; aborted".into(),
        ));
    }
    session.confirm_target_sas().map_err(pair_err)?;

    let hello = HostHello::new(keys.public_key().to_hex(), name.to_string());
    let hello = Zeroizing::new(serde_json::to_string(&hello)?);
    let hello_event = session
        .send_return_payload(PayloadType::Custom, hello)
        .map_err(pair_err)?;
    publish(&mut conn, hello_event).await?;

    let grant = loop {
        let event = next_pairing_event(&mut conn).await?;
        check_abort(session, &event)?;
        match session.handle_payload(&event) {
            Ok((PayloadType::Custom, payload)) => {
                break HostGrant::decode_verified(&payload, &keys.public_key());
            }
            Ok((other, _)) => {
                break Err(HostError::Pairing(format!(
                    "unexpected payload type {other:?}"
                )));
            }
            Err(_) => continue,
        }
    };
    let grant = match grant {
        Ok(g) => g,
        Err(e) => {
            abort(&mut conn, session, AbortReason::ProtocolError).await;
            return Err(e);
        }
    };
    let complete = session.send_complete().map_err(pair_err)?;
    publish(&mut conn, complete).await?;
    let _ = conn.disconnect().await;
    Ok(grant)
}

/// NIP-42 with the session's ephemeral key, when the relay asks for it.
async fn authenticate_ephemeral(
    conn: &mut NostrWsConnection,
    session: &PairingSession,
    relay: &str,
) -> Result<()> {
    let challenge = loop {
        match conn.next_event(Duration::from_secs(5)).await {
            Ok(RelayMessage::Auth { challenge }) => break challenge,
            Ok(_) => continue,
            Err(WsClientError::Timeout) => return Ok(()), // relay does not require AUTH
            Err(e) => return Err(ws_err(e)),
        }
    };
    let url = RelayUrl::parse(relay).map_err(|e| HostError::Relay(e.to_string()))?;
    let auth = session
        .sign_event(EventBuilder::auth(challenge, url))
        .map_err(pair_err)?;
    let id = auth.id.to_hex();
    conn.send_raw(&json!(["AUTH", auth]))
        .await
        .map_err(ws_err)?;
    loop {
        match conn
            .next_event(Duration::from_secs(20))
            .await
            .map_err(ws_err)?
        {
            RelayMessage::Ok(ok) if ok.event_id == id => {
                return if ok.accepted {
                    Ok(())
                } else {
                    Err(HostError::Relay(format!(
                        "relay refused pairing auth: {}",
                        ok.message
                    )))
                };
            }
            _ => continue,
        }
    }
}

async fn wait_eose(conn: &mut NostrWsConnection) -> Result<()> {
    loop {
        match conn
            .next_event(Duration::from_secs(15))
            .await
            .map_err(ws_err)?
        {
            RelayMessage::Eose { subscription_id } if subscription_id == SUB_ID => return Ok(()),
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == SUB_ID => {
                return Err(HostError::Relay(format!(
                    "relay closed pairing subscription: {message}"
                )));
            }
            _ => continue,
        }
    }
}

async fn publish(conn: &mut NostrWsConnection, event: Event) -> Result<()> {
    let ok = conn.send_event(event).await.map_err(ws_err)?;
    if ok.accepted {
        Ok(())
    } else {
        Err(HostError::Relay(format!(
            "relay rejected pairing event: {}",
            ok.message
        )))
    }
}

async fn next_pairing_event(conn: &mut NostrWsConnection) -> Result<Event> {
    loop {
        match conn.next_event(PAIRING_DEADLINE).await.map_err(ws_err)? {
            RelayMessage::Event {
                subscription_id,
                event,
            } if subscription_id == SUB_ID => return Ok(*event),
            _ => continue,
        }
    }
}

fn check_abort(session: &mut PairingSession, event: &Event) -> Result<()> {
    match session.handle_abort(event) {
        Ok(reason) => Err(HostError::Pairing(format!(
            "desktop aborted pairing: {reason:?}"
        ))),
        Err(_) => Ok(()),
    }
}

async fn abort(conn: &mut NostrWsConnection, session: &mut PairingSession, reason: AbortReason) {
    if let Ok(Some(event)) = session.abort(reason) {
        let _ = conn.send_event(event).await;
    }
}

/// Ask a yes/no question on the controlling terminal.
pub fn ask_yes_no(question: &str) -> Result<bool> {
    let answer = prompt_line(question)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

/// Read one line from the terminal. Uses `/dev/tty` so `curl … | sh`
/// installers can still prompt.
pub fn prompt_line(question: &str) -> Result<String> {
    print!("{question}");
    std::io::stdout()
        .flush()
        .map_err(|e| HostError::io("write prompt", e))?;
    let mut line = String::new();
    let tty = std::fs::File::open("/dev/tty");
    let read = match tty {
        Ok(f) => std::io::BufReader::new(f).read_line(&mut line),
        Err(_) => std::io::stdin().lock().read_line(&mut line),
    };
    read.map_err(|e| HostError::io("read answer", e))?;
    Ok(line.trim().to_string())
}
