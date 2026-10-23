//! Session exchange with AI Album.
//!
//! Like Image Studio, credentials are exchanged through the internal VOS gateway. A full user
//! session is needed here for selecting originals; upload-only API keys cannot list images.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header::HOST},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    App,
    error::{Error, Result},
};

#[derive(Deserialize)]
pub struct AlbumSession {
    origin: String,
}

/// Rewrite the caller's origin into the platform's own HTTPS origin, refusing anything else.
pub fn album_origin(origin: &str, host: &str, external_port: u16) -> Result<String> {
    let mut url = reqwest::Url::parse(origin).map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid VOS origin"))?;
    let expected = reqwest::Url::parse(&format!("http://{host}")).map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid VOS host"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != expected.host_str()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error(StatusCode::BAD_REQUEST, "Invalid VOS origin"));
    }
    if url.scheme() == "http" {
        url.set_scheme("https").map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid VOS origin"))?;
        url.set_port(Some(external_port)).map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid VOS port"))?;
    }
    Ok(url.origin().ascii_serialization())
}

pub async fn session(State(app): State<App>, headers: HeaderMap, Json(q): Json<AlbumSession>) -> Result<Json<Value>> {
    app.auth.identify(&headers).await?;
    let host = headers.get(HOST).and_then(|value| value.to_str().ok()).ok_or(Error(StatusCode::BAD_REQUEST, "Missing VOS host"))?;
    let origin = album_origin(&q.origin, host, app.config.gateway_external_port)?;
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(Error(StatusCode::UNAUTHORIZED, "Sign in to VOS"))?;
    let response = app
        .client
        .post(format!("http://{}/app/com.ictrek.ai-album/api/oauth/fastpath", gateway(&app)))
        .json(&json!({"accessToken": token, "origin": origin}))
        .send()
        .await
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "AI Album is unavailable"))?;
    if !response.status().is_success() {
        return Err(Error(
            if response.status().as_u16() == 401 { StatusCode::UNAUTHORIZED } else { StatusCode::BAD_GATEWAY },
            "AI Album could not authorize the current VOS account",
        ));
    }
    let value: Value = response.json().await.map_err(|_| Error(StatusCode::BAD_GATEWAY, "Invalid AI Album session"))?;
    if value["accessToken"].as_str().is_none_or(str::is_empty) {
        return Err(Error(StatusCode::BAD_GATEWAY, "AI Album returned no session"));
    }
    Ok(Json(value))
}

fn gateway(app: &App) -> String {
    format!("{}:{}", app.config.gateway_host, app.config.gateway_port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_origin_uses_platform_https_and_rejects_other_hosts() {
        assert_eq!(album_origin("http://box.test:1182", "box.test:1182", 1180).unwrap(), "https://box.test:1180");
        assert_eq!(album_origin("https://box.test", "box.test", 1180).unwrap(), "https://box.test");
        for origin in ["https://other.test", "https://user@box.test", "https://box.test/path", "https://box.test?x=1", "file:///etc/passwd"] {
            assert!(album_origin(origin, "box.test", 1180).is_err());
        }
    }
}
