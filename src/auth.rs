use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};
use url::Url;
use uuid::Uuid;

pub const API_ROOT: &str = "https://api.resend.com";

// Deliberately no Debug implementation: credentials must never enter logs.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub profile: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub expires_at: i64,
    #[serde(skip)]
    needs_save: bool,
}

impl Credentials {
    pub fn api_key(key: String) -> Self {
        let profile = format!("key-{:x}", Sha256::digest(key.as_bytes()));
        Self {
            profile,
            access_token: key,
            refresh_token: None,
            client_id: None,
            expires_at: i64::MAX,
            needs_save: false,
        }
    }
    pub fn save(&mut self) -> Result<()> {
        self.save_with(|value| {
            credential_entry()?
                .set_password(value)
                .context("Could not save the connection in the operating system credential store")
        })
    }
    fn save_with(&mut self, persist: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        persist(&serde_json::to_string(self)?)?;
        self.needs_save = false;
        Ok(())
    }
    pub fn load() -> Result<Option<Self>> {
        match credential_entry()?.get_password() {
            Ok(value) => Ok(Some(serde_json::from_str(&value)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("Could not read the saved connection"),
        }
    }
    pub fn forget() -> Result<()> {
        match credential_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn refresh_if_needed(&mut self, client: &Client) -> Result<()> {
        // A failed keyring write must be retried even though the rotated access
        // token is now fresh. Otherwise the next launch only has the old token.
        if self.needs_save {
            self.save()?;
        }
        if self.expires_at > chrono::Utc::now().timestamp() + 90 {
            return Ok(());
        }
        let client_id = self
            .client_id
            .as_deref()
            .context("Reconnect your Resend account")?;
        let refresh_token = self
            .refresh_token
            .as_deref()
            .context("Reconnect your Resend account")?;
        let response = client
            .post(format!("{API_ROOT}/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", refresh_token),
            ])
            .send()?;
        let token: Token = token_response(response)?;
        self.apply(token);
        // Rotation must be persisted before making another API request.
        self.save()?;
        Ok(())
    }
    fn apply(&mut self, token: Token) {
        self.access_token = token.access_token;
        self.refresh_token = Some(token.refresh_token);
        self.expires_at = chrono::Utc::now().timestamp() + token.expires_in;
        self.needs_save = true;
    }
}

fn credential_entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(
        "receive.resend.desktop",
        "active-account",
    )?)
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

fn token_response(response: reqwest::blocking::Response) -> Result<Token> {
    if !response.status().is_success() {
        bail!(
            "Resend authorization failed ({}). Please reconnect your account.",
            response.status()
        );
    }
    Ok(response.json()?)
}

pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn oauth_login(
    client: &Client,
    open_browser: impl FnOnce(&str) -> Result<()>,
) -> Result<Credentials> {
    // This temporary loopback listener is only for the browser login callback.
    // It is never exposed publicly and is unrelated to receiving mail/webhooks.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect = format!(
        "http://127.0.0.1:{}/oauth/callback",
        listener.local_addr()?.port()
    );
    let response = client.post(format!("{API_ROOT}/oauth/register"))
        .json(&json!({ "client_name": "Receive Desktop", "redirect_uris": ["http://127.0.0.1/oauth/callback"],
            "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"],
            "token_endpoint_auth_method": "none", "scope": "full_access" })).send()?;
    anyhow::ensure!(
        response.status().is_success(),
        "Could not register Receive with Resend ({}).",
        response.status()
    );
    let registration: serde_json::Value = response.json()?;
    let client_id = registration["client_id"]
        .as_str()
        .context("Resend did not return a client ID")?
        .to_owned();
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let state = Uuid::new_v4().to_string();
    let mut authorize = Url::parse(&format!("{API_ROOT}/oauth/authorize"))?;
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", "full_access"),
        ("state", &state),
        ("code_challenge", &pkce_challenge(&verifier)),
        ("code_challenge_method", "S256"),
    ]);
    open_browser(authorize.as_str())?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(180) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                let mut request = Vec::new();
                let mut bytes = [0; 1024];
                while request.len() < 16384 {
                    let count = match stream.read(&mut bytes) {
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&bytes[..count]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let target = request
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/");
                let callback = Url::parse(&format!("http://127.0.0.1{target}"))?;
                let params: std::collections::HashMap<_, _> =
                    callback.query_pairs().into_owned().collect();
                if callback.path() != "/oauth/callback" || params.get("state") != Some(&state) {
                    let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let reply = "Login received. You can close this tab and return to Receive.";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    reply.len(),
                    reply
                );
                if params.contains_key("error") {
                    bail!("Resend connection was cancelled or denied.");
                }
                let code = params
                    .get("code")
                    .context("Resend did not return an authorization code")?;
                let response = client
                    .post(format!("{API_ROOT}/oauth/token"))
                    .form(&[
                        ("grant_type", "authorization_code"),
                        ("client_id", &client_id),
                        ("code", code),
                        ("redirect_uri", &redirect),
                        ("code_verifier", &verifier),
                    ])
                    .send()?;
                let token = token_response(response)?;
                let mut credentials = Credentials {
                    profile: Uuid::new_v4().to_string(),
                    access_token: String::new(),
                    refresh_token: None,
                    client_id: Some(client_id),
                    expires_at: 0,
                    needs_save: false,
                };
                credentials.apply(token);
                return Ok(credentials);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("The connection timed out. Click Connect Resend to try again.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_credentials_remain_pending_until_persistence_succeeds() {
        let mut credentials = Credentials::api_key("test".into());
        credentials.apply(Token {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_in: 3600,
        });
        assert!(
            credentials
                .save_with(|_| anyhow::bail!("keyring unavailable"))
                .is_err()
        );
        assert!(credentials.needs_save);
        credentials
            .save_with(|value| {
                let saved: Credentials = serde_json::from_str(value)?;
                assert_eq!(saved.refresh_token.as_deref(), Some("new-refresh"));
                assert_eq!(saved.access_token, "new-access");
                Ok(())
            })
            .unwrap();
        assert!(!credentials.needs_save);
    }
    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            super::pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
