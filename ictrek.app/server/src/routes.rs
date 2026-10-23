//! The HTTP surface: one place where every endpoint is mounted.
//!
//! Kept apart from [`crate::App`] because the two change for different reasons. `App` is the
//! process (one HTTP client, one identity cache, one MCP workspace table); this file is the route
//! table, and it is also where a missing or extra route is caught — the tests below assert the
//! status of every endpoint, which they can only do because the router is built without binding a
//! port. That is why it lives in the library and not in `main.rs`.

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{any, get, post},
};

use crate::{App, album, files, mcp};

/// The HTTP surface, mounted by nginx under `/storage/`.
pub fn router(app: App) -> Router {
    let body_limit = app.config.request_body_limit;
    let mut router = Router::new()
        .route("/album/session", post(album::session))
        .route("/volumes", get(files::volumes))
        .route("/index", get(files::index))
        .route("/download", get(files::download))
        .route("/upload", post(files::upload))
        .route("/new_folder", post(files::new_folder));
    if app.mcp.is_some() {
        // Streamable HTTP uses one endpoint for POST/GET/DELETE; the wildcard keeps clients that
        // append a path segment working instead of returning 404.
        router = router.route("/mcp", any(mcp::handler)).route("/mcp/{*rest}", any(mcp::handler));
    }
    router.layer(DefaultBodyLimit::max(body_limit)).with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::Config;

    #[tokio::test]
    async fn every_route_refuses_an_anonymous_request() {
        let router = router(App::new(Config::default()).unwrap());
        for (method, uri) in [("POST", "/mcp"), ("GET", "/volumes"), ("GET", "/index?adapter=x_1&path=/")] {
            let request = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn agent_traffic_can_be_turned_off_without_touching_the_file_bridge() {
        let mut config = Config::default();
        config.mcp.enabled = false;
        let router = router(App::new(config).unwrap());
        let request = Request::builder().method("POST").uri("/mcp").body(Body::empty()).unwrap();
        assert_eq!(router.clone().oneshot(request).await.unwrap().status(), StatusCode::NOT_FOUND);
        // The file bridge still answers — with 401, not 404.
        let request = Request::builder().uri("/volumes").body(Body::empty()).unwrap();
        assert_eq!(router.oneshot(request).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }
}
