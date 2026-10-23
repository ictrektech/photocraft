//! One error type for the whole service.
//!
//! VOS clients read `code` / `msg`, so the JSON shape is a contract: the platform's own error codes
//! (`10001` expired session, `10007` not found) are preserved where the original bridge reported
//! them. Messages never contain absolute paths or another user's data.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// A failed request: HTTP status plus a fixed, user-facing message.
#[derive(Debug)]
pub struct Error(pub StatusCode, pub &'static str);

pub type Result<T> = std::result::Result<T, Error>;

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = match self.0 {
            StatusCode::UNAUTHORIZED => 10001,
            StatusCode::NOT_FOUND => 10007,
            _ => 10002,
        };
        (self.0, Json(json!({"code": code, "msg": self.1, "data": null}))).into_response()
    }
}

/// Filesystem failures are reported the same way for reads and writes: the browser only needs to
/// know that the authorized file is unavailable, never why.
pub fn io(_: std::io::Error) -> Error {
    Error(StatusCode::BAD_REQUEST, "The authorized file is unavailable or read-only")
}

/// The success envelope every endpoint returns.
pub fn ok(data: Value) -> Json<Value> {
    Json(json!({"code": 0, "data": data}))
}
