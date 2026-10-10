//! `GET /host/install.sh`: the agent-host installer, with the relay's own
//! `https://<host>/host` filled in.
//!
//! `curl … | bash` cannot know the URL it was fetched from, so the relay
//! substitutes [`BASE_PLACEHOLDER`] at serve time. The base is built from the
//! request's `Host` (the same host-derived community boundary as every other
//! HTTP path) and the scheme of `X-Forwarded-Proto`, else of `RELAY_URL`.
//! A `Host` that is not a plain hostname is never written into the script;
//! the placeholder then stays and the script needs `--base`.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Request path of the installer inside the public web bundle.
pub const INSTALLER_PATH: &str = "/host/install.sh";

/// Token in `scripts/install-buzz-host.sh` replaced with the served base.
pub const BASE_PLACEHOLDER: &str = "__BUZZ_HOST_BASE__";

/// `host[:port]` made only of characters that are safe inside the script's
/// single quotes and in a URL authority.
fn is_plain_authority(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 260
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

/// `https://<host>/host` (or `http://` for a plain-HTTP relay), or `None`
/// when the request carries no usable `Host`.
pub fn served_base(headers: &HeaderMap, config_relay_url: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?.trim();
    if !is_plain_authority(host) {
        return None;
    }
    let forwarded = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_ascii_lowercase());
    let scheme = match forwarded.as_deref() {
        Some("https") => "https",
        Some("http") => "http",
        _ if config_relay_url.trim_start().starts_with("wss://")
            || config_relay_url.trim_start().starts_with("https://") =>
        {
            "https"
        }
        _ => "http",
    };
    Some(format!("{scheme}://{host}/host"))
}

/// The installer script with `base` substituted (left as-is without one).
pub fn render(script: &str, base: Option<&str>) -> String {
    match base {
        Some(base) => script.replace(BASE_PLACEHOLDER, base),
        None => script.to_string(),
    }
}

/// Serve `<web_dir>/host/install.sh` for this request. A missing file is a
/// 404, like every other `/host/*` artifact.
pub async fn serve(
    web_dir: &std::path::Path,
    headers: &HeaderMap,
    config_relay_url: &str,
) -> Response {
    let Ok(script) = tokio::fs::read_to_string(web_dir.join("host").join("install.sh")).await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = render(&script, served_base(headers, config_relay_url).as_deref());
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-sh"),
            ),
            // The body depends on the request host; never share it across hosts.
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (
                header::VARY,
                HeaderValue::from_static("Host, X-Forwarded-Proto"),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(host: &str, proto: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(host).expect("host"));
        if let Some(proto) = proto {
            headers.insert(
                "x-forwarded-proto",
                HeaderValue::from_str(proto).expect("proto"),
            );
        }
        headers
    }

    #[test]
    fn base_is_the_request_host_with_the_relay_scheme() {
        assert_eq!(
            served_base(&headers("buzz.example", None), "wss://config.example").as_deref(),
            Some("https://buzz.example/host")
        );
        assert_eq!(
            served_base(&headers("localhost:3000", None), "ws://localhost:3000").as_deref(),
            Some("http://localhost:3000/host")
        );
        // A TLS-terminating proxy wins over a plain-HTTP RELAY_URL.
        assert_eq!(
            served_base(
                &headers("buzz.example", Some("https")),
                "ws://localhost:3000"
            )
            .as_deref(),
            Some("https://buzz.example/host")
        );
        assert_eq!(
            served_base(&headers("[::1]:3000", Some("http")), "wss://x").as_deref(),
            Some("http://[::1]:3000/host")
        );
    }

    #[test]
    fn hostile_or_missing_host_is_never_written_into_the_script() {
        for host in ["evil.example'; rm -rf ~ #", "a b", "x/y", "a$(id)", ""] {
            let mut map = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(host) {
                map.insert(header::HOST, value);
            }
            assert_eq!(served_base(&map, "wss://r"), None, "{host:?}");
        }
        assert_eq!(served_base(&HeaderMap::new(), "wss://r"), None);
    }

    #[test]
    fn render_fills_every_placeholder_or_leaves_the_script_alone() {
        let script = format!("A='{BASE_PLACEHOLDER}'\nB=\"{BASE_PLACEHOLDER}/x\"\n");
        assert_eq!(
            render(&script, Some("https://r.example/host")),
            "A='https://r.example/host'\nB=\"https://r.example/host/x\"\n"
        );
        assert_eq!(render(&script, None), script);
    }

    /// The real installer carries the placeholder the relay replaces, in
    /// the POSIX-safe assignment the script reads it from.
    #[test]
    fn the_shipped_installer_carries_the_placeholder() {
        let script = include_str!("../../../scripts/install-buzz-host.sh");
        let assignment = format!("BUZZ_HOST_SERVED_BASE='{BASE_PLACEHOLDER}'");
        assert_eq!(script.matches(&assignment).count(), 1, "{assignment}");
        let rendered = render(script, Some("https://r.example/host"));
        assert!(rendered.contains("BUZZ_HOST_SERVED_BASE='https://r.example/host'"));
        assert!(!rendered.contains(BASE_PLACEHOLDER));
    }
}
