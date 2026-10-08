use reqwest::Method;
use serde::de::DeserializeOwned;

use crate::app_state::AppState;

use super::{
    build_authenticated_relay_request, build_nip98_auth_header, classify_request_error,
    parse_json_response, relay_error_message,
};

/// Execute an authenticated GET against an explicit relay HTTP API base (the
/// invoking window's) and decode its JSON body.
pub async fn get_relay_json_at<T: DeserializeOwned>(
    state: &AppState,
    api_base_url: &str,
    path_with_query: &str,
) -> Result<T, String> {
    if !path_with_query.starts_with('/') {
        return Err("relay GET path must begin with '/'".to_string());
    }
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}{}", api_base_url.trim_end_matches('/'), path_with_query);
    let auth = build_nip98_auth_header(&Method::GET, &url, &[], state)?;
    let response = build_authenticated_relay_request(
        &state.http_client,
        Method::GET,
        &url,
        &auth,
        None,
        None,
        None,
    )
    .send()
    .await
    .map_err(|error| classify_request_error(&error))?;
    if !response.status().is_success() {
        return Err(relay_error_message(response).await);
    }
    parse_json_response(response).await
}
