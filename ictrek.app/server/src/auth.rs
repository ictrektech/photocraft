//! VOS identity for one request.
//!
//! The platform hands the app a bearer token and exposes a userinfo endpoint that answers who it
//! belongs to. We deliberately do not parse the JWT ourselves: the verdict must come from the
//! platform, which also rejects expired sessions with a `200` + non-zero `code`.
//!
//! One agent task makes dozens of tool calls, so a positive verdict is cached briefly, keyed by a
//! hash of the token (never the token itself) and never shared across tokens.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::error::{Error, Result};

/// Bound on the cache. Reaching it drops the oldest generation rather than growing without limit.
const MAX_CACHED_TOKENS: usize = 1024;

struct Cached {
    username: String,
    at: Instant,
}

pub struct Authenticator {
    client: reqwest::Client,
    userinfo: String,
    ttl: Duration,
    /// `make vos-dev` only: the dev machine has no VOS userinfo service. Every deployed process has
    /// `None` here, so a request still needs a real token.
    dev_user: Option<String>,
    cache: RwLock<HashMap<String, Cached>>,
}

impl Authenticator {
    pub fn new(client: reqwest::Client, userinfo: String, ttl: Duration, dev_user: Option<String>) -> Self {
        Self { client, userinfo, ttl, dev_user, cache: RwLock::new(HashMap::new()) }
    }

    /// The username the request's bearer token belongs to.
    pub async fn identify(&self, headers: &HeaderMap) -> Result<String> {
        if let Some(user) = &self.dev_user {
            return Ok(user.clone());
        }
        let token = bearer(headers)?;
        let key = digest(token);
        if let Some(username) = self.hit(&key).await {
            return Ok(username);
        }
        let username = self.fetch(token).await?;
        self.remember(key, &username).await;
        Ok(username)
    }

    async fn hit(&self, key: &str) -> Option<String> {
        let cache = self.cache.read().await;
        cache.get(key).filter(|entry| entry.at.elapsed() < self.ttl).map(|entry| entry.username.clone())
    }

    async fn remember(&self, key: String, username: &str) {
        let mut cache = self.cache.write().await;
        cache.retain(|_, entry| entry.at.elapsed() < self.ttl);
        if cache.len() >= MAX_CACHED_TOKENS {
            cache.clear();
        }
        cache.insert(key, Cached { username: username.to_owned(), at: Instant::now() });
    }

    async fn fetch(&self, token: &str) -> Result<String> {
        let response = self
            .client
            .get(&self.userinfo)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await
            .map_err(|_| Error(StatusCode::BAD_GATEWAY, "VOS identity service unavailable"))?;
        // 401/403 mean the session is gone; anything else is the identity service itself failing,
        // and reporting that as "session expired" makes every client re-authenticate in a loop.
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(Error(StatusCode::UNAUTHORIZED, "VOS session expired")),
            status if !status.is_success() => return Err(Error(StatusCode::BAD_GATEWAY, "VOS identity service unavailable")),
            _ => {}
        }
        let value: Value = response.json().await.map_err(|_| Error(StatusCode::UNAUTHORIZED, "Invalid VOS identity"))?;
        // The platform reports an expired session as HTTP 200 with a non-zero envelope code.
        if value.get("code").is_some_and(|code| code != 0 && code != "0") {
            return Err(Error(StatusCode::UNAUTHORIZED, "VOS session expired"));
        }
        // An application context token is a permanent admin credential the platform issues to the
        // app itself, not to a person. Its userinfo reply carries the application id in `sub` (and
        // `preferred_username`), while a user's `sub` is a 32-character user id; accepting it would
        // make every install a single admin identity with access to whatever directory happens to
        // match. A reply without `sub` keeps the previous behaviour.
        if let Some(sub) = value["sub"].as_str().filter(|sub| !sub.is_empty())
            && !is_user_id(sub)
        {
            return Err(Error(StatusCode::FORBIDDEN, "application token not allowed"));
        }
        let username = value["preferred_username"]
            .as_str()
            .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || "_-@.".contains(c)) && !matches!(*name, "." | ".."));
        username.map(str::to_owned).ok_or(Error(StatusCode::UNAUTHORIZED, "VOS identity has no valid username"))
    }
}

/// The bearer token, refusing anything else. An application context token is a platform-issued
/// admin credential and must never be accepted as a user.
fn bearer(headers: &HeaderMap) -> Result<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(Error(StatusCode::UNAUTHORIZED, "Sign in to VOS"))
}

fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

/// A person's `sub` is a 32-character user id (a ULID); an application id such as
/// `com.ictrek.photocraft` is not, which is what lets a context token be told apart.
fn is_user_id(sub: &str) -> bool {
    sub.len() == 32 && sub.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use serde_json::json;

    fn authenticator(userinfo: String) -> Authenticator {
        Authenticator::new(reqwest::Client::new(), userinfo, Duration::from_secs(60), None)
    }

    #[tokio::test]
    async fn the_development_override_skips_the_identity_service() {
        let auth = Authenticator::new(reqwest::Client::new(), String::new(), Duration::from_secs(60), Some("alice".to_owned()));
        assert_eq!(auth.identify(&HeaderMap::new()).await.ok().as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn no_token_never_reaches_the_identity_service() {
        let auth = authenticator(String::new());
        assert!(auth.identify(&HeaderMap::new()).await.is_err());
    }

    #[tokio::test]
    async fn a_token_without_the_bearer_scheme_is_refused() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "fixture".parse().expect("a header value"));
        let auth = authenticator("http://127.0.0.1:1/userinfo".to_owned());
        assert!(auth.identify(&headers).await.is_err());
    }

    #[tokio::test]
    async fn an_expired_session_is_refused_even_with_http_200() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free local port");
        let address = listener.local_addr().expect("the bound address");
        let server = tokio::spawn(async move {
            let router = Router::new().route("/userinfo", get(|| async { Json(json!({"code": 10001, "preferred_username": "alice"})) }));
            let _ = axum::serve(listener, router).await;
        });
        let auth = authenticator(format!("http://{address}/userinfo"));
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer fixture".parse().expect("a header value"));
        assert!(auth.identify(&headers).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn an_application_context_token_is_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free local port");
        let address = listener.local_addr().expect("the bound address");
        let server = tokio::spawn(async move {
            let reply = json!({"code": 0, "sub": "com.ictrek.photocraft", "preferred_username": "com.ictrek.photocraft"});
            let router = Router::new().route("/userinfo", get(move || async move { Json(reply) }));
            let _ = axum::serve(listener, router).await;
        });
        let auth = authenticator(format!("http://{address}/userinfo"));
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer fixture".parse().expect("a header value"));
        let error = auth.identify(&headers).await.expect_err("the app token must be refused");
        assert_eq!(error.0, StatusCode::FORBIDDEN);
        server.abort();
    }

    #[tokio::test]
    async fn an_identity_service_outage_is_not_reported_as_an_expired_session() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free local port");
        let address = listener.local_addr().expect("the bound address");
        let server = tokio::spawn(async move {
            let router = Router::new().route("/userinfo", get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
            let _ = axum::serve(listener, router).await;
        });
        let auth = authenticator(format!("http://{address}/userinfo"));
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer fixture".parse().expect("a header value"));
        let error = auth.identify(&headers).await.expect_err("an outage must not authenticate");
        assert_eq!(error.0, StatusCode::BAD_GATEWAY);
        server.abort();
    }

    #[tokio::test]
    async fn a_valid_session_is_cached_and_returns_the_username() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free local port");
        let address = listener.local_addr().expect("the bound address");
        let server = tokio::spawn(async move {
            let router = Router::new().route("/userinfo", get(|| async { Json(json!({"code": 0, "preferred_username": "alice"})) }));
            let _ = axum::serve(listener, router).await;
        });
        let auth = authenticator(format!("http://{address}/userinfo"));
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer fixture".parse().expect("a header value"));
        assert_eq!(auth.identify(&headers).await.ok().as_deref(), Some("alice"));
        // The second call is served from the cache; the server can go away and it still answers.
        server.abort();
        assert_eq!(auth.identify(&headers).await.ok().as_deref(), Some("alice"));
    }
}
