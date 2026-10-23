//! The Finder file bridge the web app talks to (list, download, upload, new folder).
//!
//! Every handler starts by resolving the caller, then resolves the path through [`crate::roots`].
//! No handler touches a path that did not come out of that resolution.

use axum::{
    Json,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    App,
    error::{Error, Result, io, ok},
    roots::{leaf, resolve, roots},
};

pub async fn volumes(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    let username = app.auth.identify(&headers).await?;
    let volumes: Vec<_> = roots(&app.config.exposed_root, &username)
        .into_iter()
        .map(|root| json!({"uuid": root.id, "name": root.name, "adapter": format!("x_{}", root.id)}))
        .collect();
    Ok(ok(json!({"total": volumes.len(), "data": volumes})))
}

#[derive(Deserialize)]
pub struct Location {
    adapter: String,
    path: String,
}

pub async fn index(State(app): State<App>, headers: HeaderMap, Query(q): Query<Location>) -> Result<Json<Value>> {
    let username = app.auth.identify(&headers).await?;
    let dir = resolve(&app.config.exposed_root, &username, &q.adapter, &q.path, false)?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{}{name}", if q.path.ends_with('/') { q.path.clone() } else { format!("{}/", q.path) });
        // Do not list links leaving the granted root.
        if resolve(&app.config.exposed_root, &username, &q.adapter, &path, false).is_err() {
            continue;
        }
        let meta = entry.metadata().map_err(io)?;
        files.push(json!({
            "type": if meta.is_dir() { "dir" } else { "file" },
            "path": path,
            "basename": name,
            "extension": std::path::Path::new(&name).extension().and_then(|s| s.to_str()).unwrap_or(""),
            "file_size": meta.len(),
            "storage": q.adapter,
        }));
        if files.len() >= 10000 {
            return Err(Error(StatusCode::BAD_REQUEST, "Directory contains too many entries; choose a smaller directory"));
        }
    }
    files.sort_by(|a, b| a["basename"].as_str().cmp(&b["basename"].as_str()));
    Ok(ok(json!({"files": files})))
}

pub async fn download(State(app): State<App>, headers: HeaderMap, Query(q): Query<Location>) -> Result<Response> {
    let username = app.auth.identify(&headers).await?;
    let path = resolve(&app.config.exposed_root, &username, &q.adapter, &q.path, false)?;
    if std::fs::metadata(&path).map_err(io)?.len() > app.config.download_limit {
        return Err(Error(StatusCode::PAYLOAD_TOO_LARGE, "Image exceeds the browser file limit"));
    }
    let bytes = tokio::fs::read(path).await.map_err(io)?;
    Ok(([("Content-Type", "application/octet-stream"), ("Cache-Control", "no-store")], bytes).into_response())
}

#[derive(Deserialize)]
pub struct Mutation {
    adapter: String,
    path: String,
    name: String,
}

pub async fn upload(State(app): State<App>, headers: HeaderMap, Query(q): Query<Mutation>, bytes: Bytes) -> Result<Json<Value>> {
    let username = app.auth.identify(&headers).await?;
    leaf(&q.name)?;
    let full = format!("{}{name}", if q.path.ends_with('/') { q.path.clone() } else { format!("{}/", q.path) }, name = q.name);
    let path = resolve(&app.config.exposed_root, &username, &q.adapter, &full, true)?;
    // Temp file lives beside the target: atomic replacement and the same mount's write permission.
    let parent = path.parent().ok_or(Error(StatusCode::BAD_REQUEST, "Invalid target"))?;
    let temp = parent.join(format!(
        ".photocraft-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new().create_new(true).write(true).open(&temp).await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&temp, &path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temp).await;
    }
    result.map_err(io)?;
    Ok(ok(json!("saved")))
}

pub async fn new_folder(State(app): State<App>, headers: HeaderMap, Json(q): Json<Mutation>) -> Result<Json<Value>> {
    let username = app.auth.identify(&headers).await?;
    leaf(&q.name)?;
    let path = resolve(
        &app.config.exposed_root,
        &username,
        &q.adapter,
        &format!("{}{name}", if q.path.ends_with('/') { q.path.clone() } else { format!("{}/", q.path) }, name = q.name),
        true,
    )?;
    std::fs::create_dir(path).map_err(io)?;
    Ok(ok(json!("created")))
}
