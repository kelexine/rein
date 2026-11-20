// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: OAuth 2.0 loopback browser login, keyring/file token resolution, and token refresh for Antigravity

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::event::CoreError;

/// Google OAuth 2.0 installed-app client ID used by Antigravity CLI (`agy`).
pub const DEFAULT_ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// Google OAuth 2.0 installed-app client secret paired with [`DEFAULT_ANTIGRAVITY_CLIENT_ID`].
pub const DEFAULT_ANTIGRAVITY_CLIENT_SECRET: &str = "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";

/// Google OAuth 2.0 authorization endpoint.
pub const OAUTH_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google OAuth 2.0 token exchange and refresh endpoint.
pub const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google OAuth 2.0 userinfo endpoint used to record authenticated account email.
pub const OAUTH_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";

/// OAuth scopes requested by Antigravity Cloud Code PA clients.
pub const OAUTH_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs https://www.googleapis.com/auth/aicode";

/// Resolved OAuth access/refresh credentials for Antigravity requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityTokenSet {
    /// Bearer access token.
    pub access_token: String,
    /// Optional OAuth refresh token for automatic renewal on `401`.
    pub refresh_token: Option<String>,
    /// Optional authenticated account email.
    pub email: Option<String>,
}

/// Summary returned after interactive browser OAuth completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityLoginResult {
    /// Filesystem path where the token JSON was persisted (`0o600`).
    pub token_path: PathBuf,
    /// Authenticated Google account email, when available.
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyringSecretEnvelope {
    token: Option<TokenPayload>,
    auth_method: Option<String>,
    id_token_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenPayload {
    access_token: String,
    token_type: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
}

/// Returns `(client_id, client_secret)` allowing env overrides on top of defaults.
#[must_use]
pub fn resolve_oauth_client() -> (String, String) {
    let client_id = std::env::var("ANTIGRAVITY_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_CLIENT_ID.to_owned());
    let client_secret = std::env::var("ANTIGRAVITY_CLIENT_SECRET")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_CLIENT_SECRET.to_owned());
    (client_id, client_secret)
}

/// Returns the default Rein Antigravity token file path (`~/.rein/antigravity-oauth.json`).
#[must_use]
pub fn default_rein_token_path() -> Option<PathBuf> {
    if let Some(custom_home) = std::env::var_os("REIN_HOME") {
        return Some(PathBuf::from(custom_home).join("antigravity-oauth.json"));
    }
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".rein")
            .join("antigravity-oauth.json")
    })
}

/// Removes the persisted Rein Antigravity OAuth token file at `path` if present.
pub fn logout_rein_token_at(path: &Path) -> Result<bool, CoreError> {
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(path).map_err(|source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(true)
}

/// Removes the default Rein Antigravity OAuth token file (`~/.rein/antigravity-oauth.json`) if present.
pub fn logout_rein_token() -> Result<Option<PathBuf>, CoreError> {
    let Some(path) = default_rein_token_path() else {
        return Ok(None);
    };
    if logout_rein_token_at(&path)? {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

/// Resolves Antigravity credentials across config path, env vars, Rein file, OS keyring, and CLI paths.
pub fn resolve_token(
    custom_path: Option<&Path>,
) -> Result<(AntigravityTokenSet, Option<PathBuf>), CoreError> {
    if let Some(path) = custom_path
        && let Ok(tok) = load_token_from_file(path)
    {
        return Ok((tok, Some(path.to_path_buf())));
    }

    if let Ok(env_path) = std::env::var("ANTIGRAVITY_TOKEN_PATH")
        && let p = Path::new(&env_path)
        && let Ok(tok) = load_token_from_file(p)
    {
        return Ok((tok, Some(p.to_path_buf())));
    }

    for env_key in ["ANTIGRAVITY_ACCESS_TOKEN", "ANTIGRAVITY_TOKEN"] {
        if let Ok(access_token) = std::env::var(env_key)
            && !access_token.trim().is_empty()
        {
            let refresh_token = std::env::var("ANTIGRAVITY_REFRESH_TOKEN")
                .ok()
                .filter(|s| !s.trim().is_empty());
            return Ok((
                AntigravityTokenSet {
                    access_token,
                    refresh_token,
                    email: None,
                },
                None,
            ));
        }
    }

    if let Some(rein_path) = default_rein_token_path()
        && let Ok(tok) = load_token_from_file(&rein_path)
    {
        return Ok((tok, Some(rein_path)));
    }

    if let Ok(tok) = load_token_from_keyring() {
        return Ok((tok, default_rein_token_path()));
    }

    if let Some(home) = std::env::var_os("HOME") {
        let base = PathBuf::from(home).join(".config");
        let candidates = [
            base.join("antigravity/oauth.json"),
            base.join("gorilla-opencode/antigravity-oauth.json"),
            base.join("gorilla-opencode/gemini-oauth.json"),
        ];
        for path in &candidates {
            if let Ok(tok) = load_token_from_file(path) {
                return Ok((tok, Some(path.clone())));
            }
        }
    }

    Err(CoreError::Provider(
        "missing Antigravity credentials — run `rein login` (or `/login` in TUI), authenticate `agy`, or set ANTIGRAVITY_TOKEN"
            .to_owned(),
    ))
}

/// Parses either an `agy` keyring JSON envelope or a flat OAuth token JSON document.
pub fn parse_token_json(raw: &str) -> Result<AntigravityTokenSet, String> {
    if let Ok(env) = serde_json::from_str::<KeyringSecretEnvelope>(raw)
        && let Some(tok) = env.token
        && !tok.access_token.trim().is_empty()
    {
        return Ok(AntigravityTokenSet {
            access_token: tok.access_token,
            refresh_token: tok.refresh_token.filter(|s| !s.trim().is_empty()),
            email: env.id_token_email,
        });
    }

    if let Ok(flat) = serde_json::from_str::<TokenPayload>(raw)
        && !flat.access_token.trim().is_empty()
    {
        return Ok(AntigravityTokenSet {
            access_token: flat.access_token,
            refresh_token: flat.refresh_token.filter(|s| !s.trim().is_empty()),
            email: None,
        });
    }

    if let Ok(val) = serde_json::from_str::<Value>(raw) {
        if let Some(tok) = val.get("access_token").and_then(Value::as_str)
            && !tok.trim().is_empty()
        {
            return Ok(AntigravityTokenSet {
                access_token: tok.to_owned(),
                refresh_token: val
                    .get("refresh_token")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                email: val
                    .get("id_token_email")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
        if let Some(tok) = val.pointer("/token/access_token").and_then(Value::as_str)
            && !tok.trim().is_empty()
        {
            return Ok(AntigravityTokenSet {
                access_token: tok.to_owned(),
                refresh_token: val
                    .pointer("/token/refresh_token")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                email: val
                    .get("id_token_email")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }

    Err("failed to parse access_token from Antigravity token JSON".to_owned())
}

/// Persists Antigravity OAuth tokens to `path` with owner-only `0o600` permissions on Unix.
pub fn save_token_file(
    path: &Path,
    tokens: &AntigravityTokenSet,
    expires_in: Option<u64>,
) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| CoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let payload = json!({
        "token": {
            "access_token": tokens.access_token,
            "token_type": "Bearer",
            "refresh_token": tokens.refresh_token,
            "expires_in": expires_in.unwrap_or(3599),
        },
        "auth_method": "consumer",
        "id_token_email": tokens.email,
    });
    let encoded = serde_json::to_string_pretty(&payload)?;
    std::fs::write(path, format!("{encoded}\n")).map_err(|source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    restrict_permissions(path);
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Exchanges an OAuth `refresh_token` for a fresh `access_token`.
pub fn refresh_access_token(
    agent: &ureq::Agent,
    refresh_token: &str,
) -> Result<(String, Option<u64>), CoreError> {
    let (client_id, client_secret) = resolve_oauth_client();
    let resp = agent
        .post(OAUTH_TOKEN_URL)
        .send_form([
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .map_err(|err| CoreError::Provider(format!("Antigravity OAuth refresh error: {err}")))?;

    let body = resp
        .into_body()
        .read_to_string()
        .map_err(|err| CoreError::Provider(format!("OAuth refresh read failed: {err}")))?;
    let parsed: OAuthTokenResponse = serde_json::from_str(&body)?;
    Ok((parsed.access_token, parsed.expires_in))
}

/// Runs an interactive Google OAuth 2.0 loopback flow and saves credentials to `token_save_path`.
pub fn login_with_browser(
    token_save_path: &Path,
    notify_url: impl FnOnce(&str),
) -> Result<AntigravityLoginResult, CoreError> {
    let never_cancel = AtomicBool::new(false);
    login_with_browser_until(token_save_path, notify_url, &never_cancel)
}

/// Runs an interactive Google OAuth 2.0 loopback flow that aborts cleanly if `cancel` becomes `true`.
pub fn login_with_browser_until(
    token_save_path: &Path,
    notify_url: impl FnOnce(&str),
    cancel: &AtomicBool,
) -> Result<AntigravityLoginResult, CoreError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|err| CoreError::Provider(format!("failed to bind OAuth callback port: {err}")))?;
    let _ = listener.set_nonblocking(true);
    let port = listener
        .local_addr()
        .map_err(|err| CoreError::Provider(format!("failed to read local callback addr: {err}")))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    let state = generate_oauth_state();
    let (client_id, client_secret) = resolve_oauth_client();

    let auth_url = build_authorize_url(&client_id, &redirect_uri, &state);
    notify_url(&auth_url);
    let _ = Command::new("xdg-open")
        .arg(&auth_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    let code = wait_for_oauth_callback(&listener, &state, cancel)?;
    let agent = ureq::Agent::new_with_defaults();
    let token_resp = agent
        .post(OAUTH_TOKEN_URL)
        .send_form([
            ("code", code.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .map_err(|err| CoreError::Provider(format!("OAuth token exchange failed: {err}")))?;

    let token_body = token_resp.into_body().read_to_string().map_err(|err| {
        CoreError::Provider(format!("failed reading OAuth token response: {err}"))
    })?;
    let parsed: OAuthTokenResponse = serde_json::from_str(&token_body)?;
    let _ = parsed.token_type.as_deref();

    let email = fetch_userinfo_email(&agent, &parsed.access_token);
    let tokens = AntigravityTokenSet {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        email: email.clone(),
    };
    save_token_file(token_save_path, &tokens, parsed.expires_in)?;
    Ok(AntigravityLoginResult {
        token_path: token_save_path.to_path_buf(),
        email,
    })
}

fn build_authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{OAUTH_AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        percent_encode(client_id),
        percent_encode(redirect_uri),
        percent_encode(OAUTH_SCOPES),
        percent_encode(state),
    )
}

fn wait_for_oauth_callback(
    listener: &TcpListener,
    expected_state: &str,
    cancel: &AtomicBool,
) -> Result<String, CoreError> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Provider("OAuth login cancelled".to_owned()));
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                stream
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(80));
                continue;
            }
            Err(err) => {
                return Err(CoreError::Provider(format!(
                    "OAuth callback accept failed: {err}"
                )));
            }
        };
        let Some(target) = read_http_request_target(&mut stream) else {
            continue;
        };
        if !target.starts_with("/auth/callback") {
            let _ = write_html_response(&mut stream, "404 Not Found", "Not found.");
            continue;
        }
        let query = target.split_once('?').map_or("", |(_, q)| q);
        if let Some(err) = extract_query_param(query, "error") {
            let _ = write_html_response(
                &mut stream,
                "400 Bad Request",
                "Authentication was cancelled or denied. You may close this tab.",
            );
            return Err(CoreError::Provider(format!(
                "Google OAuth authorization failed: {err}"
            )));
        }
        let got_state = extract_query_param(query, "state").unwrap_or_default();
        if got_state != expected_state {
            let _ = write_html_response(
                &mut stream,
                "400 Bad Request",
                "Invalid OAuth state parameter.",
            );
            return Err(CoreError::Provider(
                "OAuth callback state mismatch".to_owned(),
            ));
        }
        let Some(code) = extract_query_param(query, "code").filter(|c| !c.is_empty()) else {
            let _ = write_html_response(
                &mut stream,
                "400 Bad Request",
                "Missing authorization code.",
            );
            return Err(CoreError::Provider(
                "OAuth callback missing authorization code".to_owned(),
            ));
        };
        let _ = write_html_response(
            &mut stream,
            "200 OK",
            "Authenticated with Antigravity for Rein. You can close this browser tab and return to your terminal.",
        );
        return Ok(code);
    }
}

fn read_http_request_target(stream: &mut TcpStream) -> Option<String> {
    let cloned = stream.try_clone().ok()?;
    let mut reader = BufReader::new(cloned);
    let mut request_line = String::new();
    let bytes = reader.read_line(&mut request_line).ok()?;
    if bytes == 0 {
        return None;
    }
    request_line.split_whitespace().nth(1).map(str::to_owned)
}

fn write_html_response(stream: &mut TcpStream, status: &str, message: &str) -> std::io::Result<()> {
    let body = format!(
        "<!doctype html><html><body style=\"font-family:sans-serif;padding:2rem\"><h2>Rein · Antigravity Auth</h2><p>{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn fetch_userinfo_email(agent: &ureq::Agent, access_token: &str) -> Option<String> {
    let resp = agent
        .get(OAUTH_USERINFO_URL)
        .header("Authorization", &format!("Bearer {access_token}"))
        .call()
        .ok()?;
    let body = resp.into_body().read_to_string().ok()?;
    let parsed: Value = serde_json::from_str(&body).ok()?;
    parsed
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn load_token_from_file(path: &Path) -> Result<AntigravityTokenSet, String> {
    let raw = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    parse_token_json(&raw)
}

fn load_token_from_keyring() -> Result<AntigravityTokenSet, String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", "gemini", "username", "antigravity"])
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err("secret-tool lookup failed".to_owned());
    }
    let raw = String::from_utf8(output.stdout).map_err(|err| err.to_string())?;
    parse_token_json(&raw)
}

fn generate_oauth_state() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", nanos, std::process::id())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(char::from(*byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn extract_query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(percent_decode(v));
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'+' {
            out.push(b' ');
            idx += 1;
        } else if bytes[idx] == b'%'
            && idx + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[idx + 1]), hex_val(bytes[idx + 2]))
        {
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keyring_and_flat_oauth_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let keyring = r#"{
            "token": {
                "access_token": "ya29.abc",
                "refresh_token": "1//ref",
                "token_type": "Bearer",
                "expires_in": 3599
            },
            "auth_method": "consumer",
            "id_token_email": "kelexine@example.com"
        }"#;
        let parsed = parse_token_json(keyring)?;
        assert_eq!(parsed.access_token, "ya29.abc");
        assert_eq!(parsed.refresh_token.as_deref(), Some("1//ref"));
        assert_eq!(parsed.email.as_deref(), Some("kelexine@example.com"));

        let flat = r#"{"access_token": "ya29.flat", "refresh_token": "1//flat"}"#;
        let parsed_flat = parse_token_json(flat)?;
        assert_eq!(parsed_flat.access_token, "ya29.flat");
        assert_eq!(parsed_flat.refresh_token.as_deref(), Some("1//flat"));
        assert_eq!(parsed_flat.email, None);
        Ok(())
    }

    #[test]
    fn query_param_and_percent_encoding_roundtrip() {
        let query = "state=abc-123&code=4%2F0AfJohX%2Bxyz";
        assert_eq!(
            extract_query_param(query, "state").as_deref(),
            Some("abc-123")
        );
        assert_eq!(
            extract_query_param(query, "code").as_deref(),
            Some("4/0AfJohX+xyz")
        );
    }
}
