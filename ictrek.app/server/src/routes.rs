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
    routing::{get, post},
};

use crate::{App, album, files};

/// The HTTP surface, mounted by nginx under `/storage/`.
pub fn router(app: App) -> Router {
    let body_limit = app.config.request_body_limit;
    let router = Router::new()
        .route("/album/session", post(album::session))
        .route("/volumes", get(files::volumes))
        .route("/index", get(files::index))
        .route("/download", get(files::download))
        .route("/upload", post(files::upload))
        .route("/new_folder", post(files::new_folder));
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
        for (method, uri) in [("GET", "/volumes"), ("GET", "/index?adapter=x_1&path=/"), ("GET", "/download?adapter=x_1&path=/")] {
            let request = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
    }
}
