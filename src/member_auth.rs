//! OAuth resource-server boundary. Authorization, PKCE, refresh and token revocation
//! belong to the configured OAuth provider; device credentials are never accepted here.
use crate::inbox::Actor;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub resource: String,
    pub issuer: String,
    pub introspection_url: String,
    pub introspection_client_id: String,
    pub secret_env: String,
    pub secrets_file: PathBuf,
    pub connections_file: PathBuf,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Only for isolated local testing; never permits non-loopback HTTP.
    #[serde(default)]
    pub allow_loopback_http: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connections {
    connections: Vec<Binding>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    id: String,
    subject: String,
    client_id: String,
    member_id: String,
    #[serde(default)]
    can_send: bool,
    #[serde(default)]
    revoked: bool,
}

fn endpoint(value: &str, local: bool) -> Result<reqwest::Url> {
    let u = reqwest::Url::parse(value)?;
    ensure!(
        u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none(),
        "invalid OAuth endpoint"
    );
    let loopback = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        u.scheme() == "https" || (local && loopback && u.scheme() == "http"),
        "OAuth endpoints require HTTPS"
    );
    Ok(u)
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(&std::fs::read_to_string(path)?)?;
        let base = path.parent().unwrap_or(Path::new("."));
        if config.secrets_file.is_relative() {
            config.secrets_file = base.join(&config.secrets_file);
        }
        if config.connections_file.is_relative() {
            config.connections_file = base.join(&config.connections_file);
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            endpoint(&self.resource, self.allow_loopback_http)?.path() == "/mcp",
            "resource must end in /mcp"
        );
        endpoint(&self.issuer, self.allow_loopback_http)?;
        endpoint(&self.introspection_url, self.allow_loopback_http)?;
        for origin in &self.allowed_origins {
            endpoint(origin, self.allow_loopback_http)?;
        }
        crate::team::validate_env_name(&self.secret_env)?;
        ensure!(
            !self.introspection_client_id.is_empty(),
            "missing introspection client"
        );
        self.bindings()?;
        Ok(())
    }
    fn bindings(&self) -> Result<Vec<Binding>> {
        let bytes = std::fs::read(&self.connections_file)?;
        ensure!(bytes.len() <= 65536, "connections file too large");
        let connections: Connections = toml::from_str(std::str::from_utf8(&bytes)?)?;
        let mut keys = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for b in &connections.connections {
            ensure!(
                !b.subject.is_empty() && !b.client_id.is_empty() && !b.member_id.is_empty(),
                "invalid member binding"
            );
            ensure!(
                !b.id.is_empty()
                    && b.id.len() <= 80
                    && b.id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c)),
                "invalid connection id"
            );
            ensure!(
                keys.insert((&b.subject, &b.client_id)) && ids.insert(&b.id),
                "duplicate member binding"
            );
        }
        Ok(connections.connections)
    }
    pub fn metadata_url(&self) -> String {
        format!(
            "{}/.well-known/oauth-protected-resource/mcp",
            self.resource.trim_end_matches("/mcp")
        )
    }
}

#[derive(Clone)]
pub struct Authenticator {
    pub config: Config,
    client: reqwest::Client,
}
#[derive(Debug)]
pub enum AuthError {
    Unauthorized,
    Forbidden,
    Unavailable,
}
impl Authenticator {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        // A redirect must never forward access tokens or the introspection secret.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { config, client })
    }
    pub async fn authenticate(&self, token: &str) -> std::result::Result<Actor, AuthError> {
        if token.is_empty() || token.len() > 8192 || token.chars().any(char::is_whitespace) {
            return Err(AuthError::Unauthorized);
        }
        let secret = crate::secrets::read_field(&self.config.secrets_file, &self.config.secret_env)
            .map_err(|_| AuthError::Unavailable)?;
        // OAuth Basic credentials use form encoding before HTTP Basic (RFC 6749 §2.3.1).
        // In particular, resource URL client IDs contain ':' and cannot be sent raw.
        let client_id =
            form_urlencoded::byte_serialize(self.config.introspection_client_id.as_bytes())
                .collect::<String>();
        let secret = form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>();
        let response = self
            .client
            .post(&self.config.introspection_url)
            .basic_auth(client_id, Some(secret))
            .form(&[("token", token), ("token_type_hint", "access_token")])
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        if !response.status().is_success() {
            return Err(AuthError::Unavailable);
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(AuthError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        let claims: Value = serde_json::from_slice(&bytes).map_err(|_| AuthError::Unavailable)?;
        self.actor(&claims)
    }
    fn actor(&self, claims: &Value) -> std::result::Result<Actor, AuthError> {
        let now = crate::store::now();
        let audience = &claims["aud"];
        let correct_audience = audience.as_str() == Some(self.config.resource.as_str())
            || audience.as_array().is_some_and(|a| {
                a.iter()
                    .any(|x| x.as_str() == Some(self.config.resource.as_str()))
            });
        if claims["active"] != true
            || claims["iss"] != self.config.issuer
            || !correct_audience
            || !claims["exp"].as_i64().is_some_and(|exp| exp > now)
            || claims
                .get("nbf")
                .is_some_and(|n| !n.as_i64().is_some_and(|n| n <= now))
        {
            return Err(AuthError::Unauthorized);
        }
        let subject = claims["sub"].as_str().ok_or(AuthError::Unauthorized)?;
        let client = claims["client_id"]
            .as_str()
            .ok_or(AuthError::Unauthorized)?;
        let scopes: Vec<&str> = claims["scope"]
            .as_str()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        if !scopes.contains(&"inbox.read") {
            return Err(AuthError::Forbidden);
        }
        // Reload on every request so revocation does not require a restart or wait for token expiry.
        let bindings = self.config.bindings().map_err(|_| AuthError::Unavailable)?;
        let b = bindings
            .iter()
            .find(|b| b.subject == subject && b.client_id == client && !b.revoked)
            .context("unbound subject")
            .map_err(|_| AuthError::Forbidden)?;
        Ok(Actor {
            member: b.member_id.clone(),
            origin: format!("dot:{}", b.id),
            can_send: b.can_send && scopes.contains(&"inbox.send"),
            human_view: false,
        })
    }
}
