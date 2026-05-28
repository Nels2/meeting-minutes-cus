use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration as TokioDuration};
use url::{form_urlencoded, Url};

const DEFAULT_SCOPES: &str = "openid profile offline_access User.Read Calendars.Read";
const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/v1.0";
const O365_SIGN_IN_TIMEOUT_SECONDS: u64 = 120;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct O365CalendarSettings {
    pub tenant_id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct O365CalendarToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct O365CalendarEvent {
    pub id: String,
    pub title: String,
    pub join_url: Option<String>,
    pub participants: Vec<String>,
    pub description: Option<String>,
    pub start: String,
    pub end: String,
    pub web_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct O365CalendarConnectionState {
    pub settings: O365CalendarSettings,
    pub connected: bool,
    pub last_events: Vec<O365CalendarEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct O365CalendarSignInResult {
    pub connected: bool,
    pub auth_url: String,
    pub manual_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredCalendarState {
    settings: O365CalendarSettings,
    token: Option<O365CalendarToken>,
    pending_verifier: Option<String>,
    pending_state: Option<String>,
    pending_redirect_uri: Option<String>,
    last_events: Vec<O365CalendarEvent>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphEventsResponse {
    value: Vec<GraphEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphEvent {
    id: String,
    subject: Option<String>,
    body_preview: Option<String>,
    attendees: Option<Vec<GraphAttendee>>,
    start: GraphDateTime,
    end: GraphDateTime,
    online_meeting: Option<GraphOnlineMeeting>,
    online_meeting_url: Option<String>,
    web_link: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphOnlineMeeting {
    join_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphDateTime {
    #[serde(rename = "dateTime")]
    date_time: String,
}

#[derive(Debug, Deserialize)]
struct GraphAttendee {
    email_address: Option<GraphEmailAddress>,
}

#[derive(Debug, Deserialize)]
struct GraphEmailAddress {
    name: Option<String>,
    address: Option<String>,
}

fn calendar_state_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {}", e))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create app data directory: {}", e))?;
    Ok(dir.join("o365_calendar.json"))
}

fn normalize_scopes(scopes: &str) -> String {
    let normalized = scopes.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        DEFAULT_SCOPES.to_string()
    } else {
        normalized
    }
}

fn normalize_settings(mut settings: O365CalendarSettings) -> O365CalendarSettings {
    settings.tenant_id = settings.tenant_id.trim().to_string();
    settings.client_id = settings.client_id.trim().to_string();
    settings.redirect_uri = settings.redirect_uri.trim().to_string();
    settings.scopes = normalize_scopes(&settings.scopes);
    settings
}

fn read_state<R: Runtime>(app: &AppHandle<R>) -> Result<StoredCalendarState, String> {
    let path = calendar_state_path(app)?;
    if !path.exists() {
        return Ok(StoredCalendarState {
            settings: O365CalendarSettings {
                scopes: DEFAULT_SCOPES.to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
    }

    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read calendar settings: {}", e))?;
    let mut state: StoredCalendarState = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse calendar settings: {}", e))?;
    state.settings = normalize_settings(state.settings);
    Ok(state)
}

fn write_state<R: Runtime>(app: &AppHandle<R>, state: &StoredCalendarState) -> Result<(), String> {
    let path = calendar_state_path(app)?;
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize calendar settings: {}", e))?;
    fs::write(path, content).map_err(|e| format!("Failed to save calendar settings: {}", e))
}

pub fn default_calendar_scopes() -> &'static str {
    DEFAULT_SCOPES
}

pub fn default_calendar_redirect_uri() -> &'static str {
    "http://localhost"
}

fn reconcile_deployed_calendar_state(
    mut state: StoredCalendarState,
    deployed_settings: O365CalendarSettings,
    managed: bool,
) -> Result<(StoredCalendarState, bool), String> {
    let deployed_settings = normalize_settings(deployed_settings);
    require_settings(&deployed_settings)?;

    let has_existing_client =
        !state.settings.tenant_id.trim().is_empty() && !state.settings.client_id.trim().is_empty();
    if !managed && has_existing_client {
        return Ok((state, false));
    }

    let client_changed = state.settings.tenant_id.trim() != deployed_settings.tenant_id
        || state.settings.client_id.trim() != deployed_settings.client_id;

    state.settings = deployed_settings;

    if managed && client_changed {
        state.token = None;
        state.pending_verifier = None;
        state.pending_state = None;
        state.pending_redirect_uri = None;
        state.last_events.clear();
    }

    Ok((state, true))
}

pub fn apply_deployed_calendar_settings<R: Runtime>(
    app: &AppHandle<R>,
    deployed_settings: O365CalendarSettings,
    managed: bool,
) -> Result<bool, String> {
    let state = read_state(app)?;
    let (state, applied) = reconcile_deployed_calendar_state(state, deployed_settings, managed)?;

    if applied {
        write_state(app, &state)?;
    }

    Ok(applied)
}

fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn build_authorize_url(
    settings: &O365CalendarSettings,
    redirect_uri: &str,
    state_value: &str,
    challenge: &str,
) -> Result<String, String> {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &settings.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", &settings.scopes)
        .append_pair("state", state_value)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();

    if !query.contains("scope=") {
        return Err("Generated Microsoft sign-in URL is missing the scope parameter".to_string());
    }

    Ok(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize?{}",
        settings.tenant_id.trim(),
        query
    ))
}

fn token_endpoint(settings: &O365CalendarSettings) -> String {
    format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        settings.tenant_id.trim()
    )
}

fn require_settings(settings: &O365CalendarSettings) -> Result<(), String> {
    if settings.tenant_id.trim().is_empty()
        || settings.client_id.trim().is_empty()
        || settings.redirect_uri.trim().is_empty()
    {
        return Err("Tenant ID, client ID, and redirect URI are required".to_string());
    }

    if settings.scopes.trim().is_empty() {
        return Err("Microsoft calendar scopes are required".to_string());
    }

    validate_redirect_uri(&settings.redirect_uri)?;

    Ok(())
}

fn validate_redirect_uri(redirect_uri: &str) -> Result<(), String> {
    let normalized = redirect_uri.trim();
    let lower = normalized.to_lowercase();

    if lower.contains("login.microsoftonline.com")
        || lower.contains("/oauth2/")
        || lower.ends_with("/authorize")
        || lower.ends_with("/token")
        || lower.contains("/authorize?")
        || lower.contains("/token?")
    {
        return Err(
            "Redirect URI must be http://localhost for this desktop app, not a Microsoft authorize or token endpoint"
                .to_string(),
        );
    }

    let parsed = Url::parse(normalized).map_err(|_| {
        "Redirect URI must be a valid URL, for example http://localhost".to_string()
    })?;
    let host = parsed.host_str().unwrap_or_default();

    if parsed.scheme() != "http" || !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1")
    {
        return Err(
            "Redirect URI must be http://localhost or http://127.0.0.1 for this desktop app"
                .to_string(),
        );
    }

    Ok(())
}

fn parse_redirect_query(redirect_url: &str) -> Result<(String, String), String> {
    let query = redirect_url
        .split_once('?')
        .map(|(_, query)| query)
        .or_else(|| redirect_url.strip_prefix('?'))
        .unwrap_or(redirect_url);

    let params = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<String, String>>();

    if let Some(error) = params.get("error") {
        return Err(params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| format!("Microsoft sign-in returned {}", error)));
    }

    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| "Redirect URL does not contain an authorization code".to_string())?;
    let state = params
        .get("state")
        .cloned()
        .ok_or_else(|| "Redirect URL does not contain state".to_string())?;
    Ok((code, state))
}

fn exchange_redirect_uri(state: &StoredCalendarState) -> String {
    state
        .pending_redirect_uri
        .clone()
        .unwrap_or_else(|| state.settings.redirect_uri.clone())
}

fn loopback_success_page() -> &'static str {
    r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Meetily Calendar Connected</title></head>
<body style="font-family: system-ui, sans-serif; padding: 32px; color: #111827;">
<h1>Microsoft 365 calendar connected</h1>
<p>You can close this browser tab and return to Meetily.</p>
</body>
</html>"#
}

fn loopback_error_page(error: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Meetily Calendar Sign-in Failed</title></head>
<body style="font-family: system-ui, sans-serif; padding: 32px; color: #111827;">
<h1>Calendar sign-in failed</h1>
<p>{}</p>
<p>Return to Meetily and use the manual redirect URL fallback if needed.</p>
</body>
</html>"#,
        error
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    )
}

fn http_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.as_bytes().len(),
        body
    )
}

fn redirect_url_from_request(request: &str, port: u16) -> Result<String, String> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| "Loopback redirect request was empty".to_string())?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method != "GET" || target.is_empty() {
        return Err("Loopback redirect request was not a valid GET request".to_string());
    }

    Ok(format!("http://localhost:{}{}", port, target))
}

fn parse_graph_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
        })
}

fn event_has_not_ended(
    event: &O365CalendarEvent,
    now: DateTime<Utc>,
    grace_after: Duration,
) -> bool {
    parse_graph_datetime(&event.end)
        .map(|end| now <= end + grace_after)
        .unwrap_or(true)
}

fn prune_past_events(
    events: Vec<O365CalendarEvent>,
    now: DateTime<Utc>,
    grace_after: Duration,
) -> Vec<O365CalendarEvent> {
    events
        .into_iter()
        .filter(|event| event_has_not_ended(event, now, grace_after))
        .collect()
}

fn prune_stored_past_events(state: &mut StoredCalendarState, grace_after: Duration) -> bool {
    let original_len = state.last_events.len();
    state.last_events = prune_past_events(state.last_events.clone(), Utc::now(), grace_after);
    state.last_events.len() != original_len
}

fn event_context(event: &O365CalendarEvent) -> String {
    let mut lines = vec![
        format!("Calendar event: {}", event.title),
        format!("Start: {}", event.start),
        format!("End: {}", event.end),
    ];

    if !event.participants.is_empty() {
        lines.push(format!("Participants: {}", event.participants.join(", ")));
    }

    if let Some(description) = event
        .description
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Description: {}", description.trim()));
    }

    lines.join("\n")
}

async fn exchange_refresh_token(
    client: &reqwest::Client,
    settings: &O365CalendarSettings,
    refresh_token: &str,
) -> Result<O365CalendarToken, String> {
    let params = [
        ("client_id", settings.client_id.as_str()),
        ("scope", settings.scopes.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let response = client
        .post(token_endpoint(settings))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to refresh Microsoft token: {}", e))?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Microsoft token refresh failed: {}", body));
    }

    let token = response
        .json::<TokenResponse>()
        .await
        .map_err(|e| format!("Failed to parse Microsoft token response: {}", e))?;

    Ok(O365CalendarToken {
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .or_else(|| Some(refresh_token.to_string())),
        expires_at: Utc::now().timestamp() + token.expires_in.unwrap_or(3600) - 60,
    })
}

async fn ensure_access_token<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let mut state = read_state(app)?;
    let token = state
        .token
        .clone()
        .ok_or_else(|| "Calendar is not connected".to_string())?;

    if token.expires_at > Utc::now().timestamp() + 30 {
        return Ok(token.access_token);
    }

    let refresh_token = token
        .refresh_token
        .ok_or_else(|| "Calendar token expired and no refresh token is available".to_string())?;
    let client = reqwest::Client::new();
    let refreshed = exchange_refresh_token(&client, &state.settings, &refresh_token).await?;
    let access_token = refreshed.access_token.clone();
    state.token = Some(refreshed);
    write_state(app, &state)?;
    Ok(access_token)
}

async fn fetch_events_from_graph<R: Runtime>(
    app: &AppHandle<R>,
    days_before: i64,
    days_after: i64,
    keep_ended_grace: Duration,
) -> Result<Vec<O365CalendarEvent>, String> {
    let access_token = ensure_access_token(app).await?;
    let start = Utc::now() - Duration::days(days_before.max(0));
    let end = Utc::now() + Duration::days(days_after.max(1));
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("startDateTime", &start.to_rfc3339())
        .append_pair("endDateTime", &end.to_rfc3339())
        .append_pair(
            "$select",
            "id,subject,bodyPreview,attendees,start,end,onlineMeeting,onlineMeetingUrl,webLink",
        )
        .append_pair("$orderby", "start/dateTime")
        .finish();

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", access_token))
            .map_err(|_| "Invalid calendar access token".to_string())?,
    );
    headers.insert(
        "Prefer",
        HeaderValue::from_static("outlook.timezone=\"UTC\""),
    );

    let response = reqwest::Client::new()
        .get(format!("{}/me/calendarView?{}", GRAPH_BASE_URL, query))
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch calendar events: {}", e))?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Microsoft Graph calendar request failed: {}", body));
    }

    let events = response
        .json::<GraphEventsResponse>()
        .await
        .map_err(|e| format!("Failed to parse calendar events: {}", e))?
        .value
        .into_iter()
        .map(|event| {
            let participants = event
                .attendees
                .unwrap_or_default()
                .into_iter()
                .filter_map(|attendee| attendee.email_address)
                .map(|email| match (email.name, email.address) {
                    (Some(name), Some(address)) if !name.trim().is_empty() => {
                        format!("{} <{}>", name, address)
                    }
                    (_, Some(address)) => address,
                    (Some(name), None) => name,
                    _ => String::new(),
                })
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>();

            O365CalendarEvent {
                id: event.id,
                title: event
                    .subject
                    .unwrap_or_else(|| "Untitled event".to_string()),
                join_url: event
                    .online_meeting
                    .and_then(|meeting| meeting.join_url)
                    .or(event.online_meeting_url),
                participants,
                description: event.body_preview,
                start: event.start.date_time,
                end: event.end.date_time,
                web_link: event.web_link,
            }
        })
        .collect::<Vec<_>>();
    let events = prune_past_events(events, Utc::now(), keep_ended_grace);

    let mut state = read_state(app)?;
    state.last_events = events.clone();
    write_state(app, &state)?;

    Ok(events)
}

async fn exchange_o365_redirect_url<R: Runtime>(
    app: &AppHandle<R>,
    redirect_url: &str,
) -> Result<(), String> {
    let (code, returned_state) = parse_redirect_query(redirect_url)?;
    let mut state = read_state(app)?;
    state.settings = normalize_settings(state.settings);
    require_settings(&state.settings)?;

    if state.pending_state.as_deref() != Some(returned_state.as_str()) {
        return Err("OIDC state did not match the pending calendar sign-in".to_string());
    }

    let verifier = state
        .pending_verifier
        .clone()
        .ok_or_else(|| "No pending calendar sign-in verifier found".to_string())?;
    let redirect_uri = exchange_redirect_uri(&state);

    let params = [
        ("client_id", state.settings.client_id.as_str()),
        ("scope", state.settings.scopes.as_str()),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier.as_str()),
    ];

    let response = reqwest::Client::new()
        .post(token_endpoint(&state.settings))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to exchange Microsoft authorization code: {}", e))?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Microsoft authorization code exchange failed: {}",
            body
        ));
    }

    let token = response
        .json::<TokenResponse>()
        .await
        .map_err(|e| format!("Failed to parse Microsoft token response: {}", e))?;

    state.token = Some(O365CalendarToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: Utc::now().timestamp() + token.expires_in.unwrap_or(3600) - 60,
    });
    state.pending_state = None;
    state.pending_verifier = None;
    state.pending_redirect_uri = None;
    write_state(app, &state)
}

async fn handle_loopback_redirect<R: Runtime>(
    app: AppHandle<R>,
    listener: TcpListener,
    port: u16,
) -> Result<(), String> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|e| format!("Failed to receive Microsoft redirect: {}", e))?;
    let mut buffer = vec![0_u8; 8192];
    let bytes_read = stream
        .read(&mut buffer)
        .await
        .map_err(|e| format!("Failed to read Microsoft redirect: {}", e))?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
    let redirect_url = redirect_url_from_request(&request, port);

    let result = match redirect_url {
        Ok(url) => exchange_o365_redirect_url(&app, &url).await,
        Err(error) => Err(error),
    };

    let (status, body) = match &result {
        Ok(_) => ("200 OK", loopback_success_page().to_string()),
        Err(error) => ("400 Bad Request", loopback_error_page(error)),
    };

    let response = http_response(status, &body);
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    result
}

#[tauri::command]
pub async fn calendar_get_o365_settings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<O365CalendarConnectionState, String> {
    let mut state = read_state(&app)?;
    if prune_stored_past_events(&mut state, Duration::zero()) {
        write_state(&app, &state)?;
    }

    Ok(O365CalendarConnectionState {
        settings: state.settings,
        connected: state.token.is_some(),
        last_events: state.last_events,
    })
}

#[tauri::command]
pub async fn calendar_save_o365_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: O365CalendarSettings,
) -> Result<(), String> {
    let settings = normalize_settings(settings);
    require_settings(&settings)?;
    let mut state = read_state(&app)?;
    state.settings = settings;
    write_state(&app, &state)
}

#[tauri::command]
pub async fn calendar_get_o365_auth_url<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    let mut state = read_state(&app)?;
    state.settings = normalize_settings(state.settings);
    require_settings(&state.settings)?;

    if state.settings.scopes.trim().is_empty() {
        return Err("Cannot start Microsoft sign-in without a scope parameter".to_string());
    }

    let verifier = random_token(96);
    let state_value = random_token(32);
    let challenge = pkce_challenge(&verifier);
    state.pending_verifier = Some(verifier);
    state.pending_state = Some(state_value.clone());
    state.pending_redirect_uri = Some(state.settings.redirect_uri.clone());
    write_state(&app, &state)?;

    build_authorize_url(
        &state.settings,
        &state.settings.redirect_uri,
        &state_value,
        &challenge,
    )
}

#[tauri::command]
pub async fn calendar_start_o365_sign_in<R: Runtime>(
    app: AppHandle<R>,
    settings: O365CalendarSettings,
) -> Result<O365CalendarSignInResult, String> {
    let settings = normalize_settings(settings);
    require_settings(&settings)?;

    let listener = match TcpListener::bind("localhost:0").await {
        Ok(listener) => listener,
        Err(_) => {
            let mut state = read_state(&app)?;
            state.settings = settings;
            write_state(&app, &state)?;
            let auth_url = calendar_get_o365_auth_url(app.clone()).await?;
            crate::api::open_external_url(auth_url.clone()).await?;
            return Ok(O365CalendarSignInResult {
                connected: false,
                auth_url,
                manual_fallback: true,
            });
        }
    };

    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to read localhost listener address: {}", e))?
        .port();
    let active_redirect_uri = format!("http://localhost:{}", port);
    let verifier = random_token(96);
    let state_value = random_token(32);
    let challenge = pkce_challenge(&verifier);
    let auth_url = build_authorize_url(&settings, &active_redirect_uri, &state_value, &challenge)?;

    let mut state = read_state(&app)?;
    state.settings = settings;
    state.pending_verifier = Some(verifier);
    state.pending_state = Some(state_value);
    state.pending_redirect_uri = Some(active_redirect_uri);
    write_state(&app, &state)?;

    crate::api::open_external_url(auth_url.clone()).await?;

    timeout(
        TokioDuration::from_secs(O365_SIGN_IN_TIMEOUT_SECONDS),
        handle_loopback_redirect(app.clone(), listener, port),
    )
    .await
    .map_err(|_| {
        "Timed out waiting for Microsoft sign-in redirect. Paste the browser redirect URL manually to complete sign-in."
            .to_string()
    })??;

    Ok(O365CalendarSignInResult {
        connected: true,
        auth_url,
        manual_fallback: false,
    })
}

#[tauri::command]
pub async fn calendar_exchange_o365_redirect<R: Runtime>(
    app: AppHandle<R>,
    redirect_url: String,
) -> Result<(), String> {
    exchange_o365_redirect_url(&app, &redirect_url).await
}

#[tauri::command]
pub async fn calendar_disconnect_o365<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let mut state = read_state(&app)?;
    state.token = None;
    state.pending_state = None;
    state.pending_verifier = None;
    state.last_events = Vec::new();
    write_state(&app, &state)
}

#[tauri::command]
pub async fn calendar_test_o365_connection<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let access_token = ensure_access_token(&app).await?;
    let response = reqwest::Client::new()
        .get(format!("{}/me", GRAPH_BASE_URL))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Failed to test Microsoft Graph connection: {}", e))?;

    if response.status().is_success() {
        Ok(true)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("Microsoft Graph connection test failed: {}", body))
    }
}

#[tauri::command]
pub async fn calendar_fetch_o365_events<R: Runtime>(
    app: AppHandle<R>,
    days_before: Option<i64>,
    days_after: Option<i64>,
) -> Result<Vec<O365CalendarEvent>, String> {
    fetch_events_from_graph(
        &app,
        days_before.unwrap_or(1),
        days_after.unwrap_or(14),
        Duration::zero(),
    )
    .await
}

#[tauri::command]
pub async fn calendar_get_current_o365_event<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<O365CalendarEvent>, String> {
    let now = Utc::now();
    let grace_before = Duration::minutes(10);
    let grace_after = Duration::minutes(10);
    let mut events = prune_past_events(read_state(&app)?.last_events, now, grace_after);
    if events.is_empty() {
        events = fetch_events_from_graph(&app, 1, 1, grace_after).await?;
    }

    Ok(events
        .into_iter()
        .filter_map(|event| {
            let start = parse_graph_datetime(&event.start)?;
            let end = parse_graph_datetime(&event.end)?;
            if now >= start - grace_before && now <= end + grace_after {
                let distance = if now < start {
                    (start - now).num_seconds().abs()
                } else if now > end {
                    (now - end).num_seconds().abs()
                } else {
                    0
                };
                Some((distance, event))
            } else {
                None
            }
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, event)| event))
}

#[tauri::command]
pub async fn calendar_build_event_context(event: O365CalendarEvent) -> Result<String, String> {
    Ok(event_context(&event))
}

#[cfg(test)]
mod tests {
    use super::{
        exchange_redirect_uri, normalize_scopes, normalize_settings, parse_redirect_query,
        prune_past_events, reconcile_deployed_calendar_state, redirect_url_from_request,
        validate_redirect_uri, O365CalendarEvent, O365CalendarSettings, O365CalendarToken,
        StoredCalendarState, DEFAULT_SCOPES,
    };
    use chrono::{Duration, Utc};

    #[test]
    fn blank_scopes_fall_back_to_default() {
        assert_eq!(normalize_scopes(""), DEFAULT_SCOPES);
        assert_eq!(normalize_scopes("   \t\n  "), DEFAULT_SCOPES);
    }

    #[test]
    fn custom_scopes_are_preserved_and_compacted() {
        assert_eq!(
            normalize_scopes("openid   profile\nCalendars.Read"),
            "openid profile Calendars.Read"
        );
    }

    #[test]
    fn settings_are_trimmed_and_scopes_are_defaulted() {
        let settings = normalize_settings(O365CalendarSettings {
            tenant_id: " tenant ".to_string(),
            client_id: " client ".to_string(),
            redirect_uri: " http://localhost ".to_string(),
            scopes: " ".to_string(),
        });

        assert_eq!(settings.tenant_id, "tenant");
        assert_eq!(settings.client_id, "client");
        assert_eq!(settings.redirect_uri, "http://localhost");
        assert_eq!(settings.scopes, DEFAULT_SCOPES);
    }

    #[test]
    fn localhost_redirect_uri_is_valid() {
        assert!(validate_redirect_uri("http://localhost").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1").is_ok());
    }

    #[test]
    fn microsoft_auth_endpoints_are_not_valid_redirect_uris() {
        assert!(validate_redirect_uri(
            "https://login.microsoftonline.com/common/oauth2/v2.0/token"
        )
        .is_err());
        assert!(validate_redirect_uri(
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
        )
        .is_err());
    }

    #[test]
    fn redirect_query_returns_code_and_state() {
        let (code, state) =
            parse_redirect_query("http://localhost:49152/?code=abc123&state=state456").unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "state456");
    }

    #[test]
    fn redirect_query_returns_microsoft_error_description() {
        let error = parse_redirect_query(
            "http://localhost:49152/?error=invalid_request&error_description=Missing%20scope",
        )
        .unwrap_err();

        assert_eq!(error, "Missing scope");
    }

    #[test]
    fn redirect_query_requires_authorization_code() {
        let error = parse_redirect_query("http://localhost:49152/?state=state456").unwrap_err();
        assert_eq!(error, "Redirect URL does not contain an authorization code");
    }

    #[test]
    fn loopback_request_reconstructs_redirect_url() {
        let request = "GET /?code=abc123&state=state456 HTTP/1.1\r\nHost: localhost:49152\r\n\r\n";
        let redirect_url = redirect_url_from_request(request, 49152).unwrap();
        assert_eq!(
            redirect_url,
            "http://localhost:49152/?code=abc123&state=state456"
        );
    }

    #[test]
    fn pending_redirect_uri_overrides_saved_redirect_uri_for_exchange() {
        let state = StoredCalendarState {
            settings: O365CalendarSettings {
                redirect_uri: "http://localhost".to_string(),
                ..Default::default()
            },
            pending_redirect_uri: Some("http://localhost:49152".to_string()),
            ..Default::default()
        };

        assert_eq!(exchange_redirect_uri(&state), "http://localhost:49152");
    }

    #[test]
    fn seed_calendar_config_does_not_replace_existing_client() {
        let state = StoredCalendarState {
            settings: O365CalendarSettings {
                tenant_id: "existing-tenant".to_string(),
                client_id: "existing-client".to_string(),
                redirect_uri: "http://localhost".to_string(),
                scopes: DEFAULT_SCOPES.to_string(),
            },
            ..Default::default()
        };
        let deployed = O365CalendarSettings {
            tenant_id: "deployed-tenant".to_string(),
            client_id: "deployed-client".to_string(),
            redirect_uri: "http://localhost".to_string(),
            scopes: DEFAULT_SCOPES.to_string(),
        };

        let (next, applied) = reconcile_deployed_calendar_state(state, deployed, false).unwrap();

        assert!(!applied);
        assert_eq!(next.settings.tenant_id, "existing-tenant");
        assert_eq!(next.settings.client_id, "existing-client");
    }

    #[test]
    fn managed_calendar_client_change_clears_auth_state() {
        let state = StoredCalendarState {
            settings: O365CalendarSettings {
                tenant_id: "old-tenant".to_string(),
                client_id: "old-client".to_string(),
                redirect_uri: "http://localhost".to_string(),
                scopes: DEFAULT_SCOPES.to_string(),
            },
            token: Some(O365CalendarToken {
                access_token: "token".to_string(),
                refresh_token: Some("refresh".to_string()),
                expires_at: 123,
            }),
            pending_verifier: Some("verifier".to_string()),
            pending_state: Some("state".to_string()),
            pending_redirect_uri: Some("http://localhost:49152".to_string()),
            last_events: vec![super::O365CalendarEvent {
                id: "1".to_string(),
                title: "event".to_string(),
                join_url: None,
                participants: Vec::new(),
                description: None,
                start: "2026-01-01T00:00:00".to_string(),
                end: "2026-01-01T01:00:00".to_string(),
                web_link: None,
            }],
        };
        let deployed = O365CalendarSettings {
            tenant_id: "new-tenant".to_string(),
            client_id: "new-client".to_string(),
            redirect_uri: "http://localhost".to_string(),
            scopes: DEFAULT_SCOPES.to_string(),
        };

        let (next, applied) = reconcile_deployed_calendar_state(state, deployed, true).unwrap();

        assert!(applied);
        assert_eq!(next.settings.tenant_id, "new-tenant");
        assert!(next.token.is_none());
        assert!(next.pending_verifier.is_none());
        assert!(next.pending_state.is_none());
        assert!(next.pending_redirect_uri.is_none());
        assert!(next.last_events.is_empty());
    }

    fn test_event(
        id: &str,
        start: chrono::DateTime<Utc>,
        end: chrono::DateTime<Utc>,
    ) -> O365CalendarEvent {
        O365CalendarEvent {
            id: id.to_string(),
            title: id.to_string(),
            join_url: None,
            participants: Vec::new(),
            description: None,
            start: start.to_rfc3339(),
            end: end.to_rfc3339(),
            web_link: None,
        }
    }

    #[test]
    fn past_calendar_events_are_pruned() {
        let now = Utc::now();
        let events = vec![
            test_event("past", now - Duration::hours(2), now - Duration::hours(1)),
            test_event(
                "current",
                now - Duration::minutes(5),
                now + Duration::minutes(30),
            ),
            test_event("future", now + Duration::hours(1), now + Duration::hours(2)),
        ];

        let remaining = prune_past_events(events, now, Duration::zero());

        assert_eq!(
            remaining
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["current", "future"]
        );
    }

    #[test]
    fn current_event_grace_keeps_recently_ended_event() {
        let now = Utc::now();
        let events = vec![
            test_event(
                "recent",
                now - Duration::hours(1),
                now - Duration::minutes(5),
            ),
            test_event("old", now - Duration::hours(2), now - Duration::minutes(20)),
        ];

        let remaining = prune_past_events(events, now, Duration::minutes(10));

        assert_eq!(
            remaining
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["recent"]
        );
    }
}
