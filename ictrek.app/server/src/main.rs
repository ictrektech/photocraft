//! VOS-authorized mounts only. Every request resolves the VOS user server-side.
#![forbid(unsafe_code)]
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

#[derive(Clone)]
struct App {
    root: PathBuf,
    userinfo: String,
    client: reqwest::Client,
}
struct Error(StatusCode, &'static str);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"code": if self.0 == StatusCode::UNAUTHORIZED {10001} else if self.0 == StatusCode::NOT_FOUND {10007} else {10002}, "msg":self.1, "data":null}))).into_response()
    }
}
type Result<T> = std::result::Result<T, Error>;
fn io(_: std::io::Error) -> Error {
    Error(StatusCode::BAD_REQUEST, "The authorized file is unavailable or read-only")
}
fn ok(data: Value) -> Json<Value> {
    Json(json!({"code":0,"data":data}))
}
async fn identity(app: &App, headers: &HeaderMap) -> Result<String> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer "))
        .ok_or(Error(StatusCode::UNAUTHORIZED, "Sign in to VOS"))?;
    let response = app
        .client
        .get(&app.userinfo)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "VOS identity service unavailable"))?;
    if !response.status().is_success() {
        return Err(Error(StatusCode::UNAUTHORIZED, "VOS session expired"));
    }
    let value: Value = response.json().await.map_err(|_| Error(StatusCode::UNAUTHORIZED, "Invalid VOS identity"))?;
    if value.get("code").is_some_and(|v| v != 0 && v != "0") {
        return Err(Error(StatusCode::UNAUTHORIZED, "VOS session expired"));
    }
    let username = value["preferred_username"]
        .as_str()
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "_-@.".contains(c)) && !matches!(*s, "." | ".."));
    username.map(str::to_owned).ok_or(Error(StatusCode::UNAUTHORIZED, "VOS identity has no valid username"))
}
#[derive(Debug)]
struct Root {
    id: String,
    name: String,
    path: PathBuf,
}
fn root_id(path: &Path) -> String {
    format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()))
}
// The mount tree is supplied by VOS. Traverse scaffolding only; never descend into another user's data.
fn roots(base: &Path, username: &str) -> Vec<Root> {
    fn visit(base: &Path, dir: &Path, username: &str, depth: usize, seen: &mut HashSet<PathBuf>, out: &mut Vec<Root>) {
        if depth > 6 {
            return;
        }
        let Ok(real) = dir.canonicalize() else { return };
        if !real.starts_with(base) || !seen.insert(real) {
            return;
        }
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "public"
            || (name == "data"
                && dir.parent().and_then(Path::file_name).and_then(|v| v.to_str()) == Some(username)
                && dir.parent().and_then(Path::parent).and_then(Path::file_name).and_then(|v| v.to_str()) == Some("users"))
        {
            // VOS may mount only public/<selected-folder>; its parent is scaffolding.
            let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
            let mounts: HashSet<PathBuf> = mounts
                .lines()
                .filter_map(|line| line.split_whitespace().nth(4))
                .map(|p| PathBuf::from(p.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\")))
                .collect();
            let children: Vec<PathBuf> =
                std::fs::read_dir(dir).ok().into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir() && mounts.contains(p)).collect();
            let paths = if children.is_empty() { vec![dir.to_path_buf()] } else { children };
            for path in paths {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                out.push(Root { id: root_id(rel), name: rel.to_string_lossy().into_owned(), path });
            }
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if name == "users" && entry.file_name() != username {
                continue;
            }
            visit(base, &path, username, depth + 1, seen, out);
        }
    }
    let mut out = Vec::new();
    if let Ok(base) = base.canonicalize() {
        visit(&base, &base, username, 0, &mut HashSet::new(), &mut out);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
fn relative(path: &str, adapter: &str) -> Result<PathBuf> {
    let raw = path.strip_prefix(&format!("{adapter}://")).unwrap_or(path);
    let rel = Path::new(raw);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) || raw.contains('\\') || raw.chars().any(char::is_control) {
        return Err(Error(StatusCode::FORBIDDEN, "Invalid authorized path"));
    }
    Ok(rel.into())
}
fn resolve(app: &App, username: &str, adapter: &str, path: &str, create: bool) -> Result<PathBuf> {
    let id = adapter.strip_prefix("x_").ok_or(Error(StatusCode::FORBIDDEN, "Unknown authorized directory"))?;
    let root = roots(&app.root, username).into_iter().find(|r| r.id == id).ok_or(Error(StatusCode::FORBIDDEN, "Directory is not authorized for this user"))?;
    let base = root.path.canonicalize().map_err(io)?;
    let target = base.join(relative(path, adapter)?);
    let check = if create && !target.exists() {
        target.parent().ok_or(Error(StatusCode::FORBIDDEN, "Invalid target"))?.canonicalize().map_err(io)?
    } else {
        target.canonicalize().map_err(|_| Error(StatusCode::NOT_FOUND, "Authorized file not found"))?
    };
    if !check.starts_with(&base) {
        return Err(Error(StatusCode::FORBIDDEN, "Path leaves the authorized directory"));
    }
    Ok(if create && !target.exists() { check.join(target.file_name().ok_or(Error(StatusCode::BAD_REQUEST, "Invalid target"))?) } else { check })
}
async fn volumes(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    let username = identity(&app, &headers).await?;
    let volumes: Vec<_> = roots(&app.root, &username).into_iter().map(|r| json!({"uuid":r.id,"name":r.name,"adapter":format!("x_{}",r.id)})).collect();
    Ok(ok(json!({"total":volumes.len(),"data":volumes})))
}
#[derive(Deserialize)]
struct Location {
    adapter: String,
    path: String,
}
async fn index(State(app): State<App>, headers: HeaderMap, Query(q): Query<Location>) -> Result<Json<Value>> {
    let username = identity(&app, &headers).await?;
    let dir = resolve(&app, &username, &q.adapter, &q.path, false)?;
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).map_err(io)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{}{name}", if q.path.ends_with('/') { q.path.clone() } else { format!("{}/", q.path) });
        // Do not list links leaving the granted root.
        if resolve(&app, &username, &q.adapter, &path, false).is_err() {
            continue;
        }
        let meta = e.metadata().map_err(io)?;
        files.push(json!({"type":if meta.is_dir(){"dir"}else{"file"},"path":path,"basename":name,"extension":Path::new(&name).extension().and_then(|s|s.to_str()).unwrap_or(""),"file_size":meta.len(),"storage":q.adapter}));
        if files.len() >= 10000 {
            return Err(Error(StatusCode::BAD_REQUEST, "Directory contains too many entries; choose a smaller directory"));
        }
    }
    files.sort_by(|a, b| a["basename"].as_str().cmp(&b["basename"].as_str()));
    Ok(ok(json!({"files":files})))
}
async fn download(State(app): State<App>, headers: HeaderMap, Query(q): Query<Location>) -> Result<Response> {
    let username = identity(&app, &headers).await?;
    let path = resolve(&app, &username, &q.adapter, &q.path, false)?;
    if std::fs::metadata(&path).map_err(io)?.len() > 256 * 1024 * 1024 {
        return Err(Error(StatusCode::PAYLOAD_TOO_LARGE, "Image exceeds the browser file limit"));
    }
    let bytes = tokio::fs::read(path).await.map_err(io)?;
    Ok(([("Content-Type", "application/octet-stream"), ("Cache-Control", "no-store")], bytes).into_response())
}
#[derive(Deserialize)]
struct Mutation {
    adapter: String,
    path: String,
    name: String,
}
fn leaf(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        Err(Error(StatusCode::BAD_REQUEST, "Invalid file name"))
    } else {
        Ok(())
    }
}
async fn upload(State(app): State<App>, headers: HeaderMap, Query(q): Query<Mutation>, bytes: Bytes) -> Result<Json<Value>> {
    let username = identity(&app, &headers).await?;
    leaf(&q.name)?;
    let full = format!("{}{name}", if q.path.ends_with('/') { q.path.clone() } else { format!("{}/", q.path) }, name = q.name);
    let path = resolve(&app, &username, &q.adapter, &full, true)?;
    // Temp file lives beside the target: atomic replacement and the same mount's write permission.
    let parent = path.parent().ok_or(Error(StatusCode::BAD_REQUEST, "Invalid target"))?;
    let temp = parent.join(format!(
        ".photocraft-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
    ));
    let result = async {
        let mut f = tokio::fs::OpenOptions::new().create_new(true).write(true).open(&temp).await?;
        use tokio::io::AsyncWriteExt;
        f.write_all(&bytes).await?;
        f.sync_all().await?;
        tokio::fs::rename(&temp, &path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temp).await;
    }
    result.map_err(io)?;
    Ok(ok(json!("saved")))
}
async fn new_folder(State(app): State<App>, headers: HeaderMap, Json(q): Json<Mutation>) -> Result<Json<Value>> {
    let username = identity(&app, &headers).await?;
    leaf(&q.name)?;
    let path = resolve(
        &app,
        &username,
        &q.adapter,
        &format!("{}{name}", if q.path.ends_with('/') { q.path.clone() } else { format!("{}/", q.path) }, name = q.name),
        true,
    )?;
    std::fs::create_dir(path).map_err(io)?;
    Ok(ok(json!("created")))
}
#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let app = App {
        root: std::env::var_os("PHOTOCRAFT_EXPOSED_ROOT").map(PathBuf::from).unwrap_or_else(|| "/exposed".into()),
        userinfo: std::env::var("VOS_OIDC_USERINFO_URL").unwrap_or_else(|_| "http://172.17.0.1:8105/v1000/oauth2/userinfo".into()),
        client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build()?,
    };
    let router = Router::new()
        .route("/volumes", get(volumes))
        .route("/index", get(index))
        .route("/download", get(download))
        .route("/upload", post(upload))
        .route("/new_folder", post(new_folder))
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8081").await?;
    axum::serve(listener, router).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roots_isolate_users_and_stop_at_grants() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["v/public/photos", "v/users/alice/data", "v/users/bob/data"] {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        let found = roots(tmp.path(), "alice");
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|r| !r.name.contains("bob")));
    }
    #[test]
    fn rejects_traversal_and_external_symlinks() {
        assert!(relative("x_root://../outside", "x_root").is_err());
        assert!(relative("/etc/passwd", "x_root").is_err());
        assert!(leaf("../x").is_err());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("v/public");
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
        let app = App { root: tmp.path().into(), userinfo: String::new(), client: reqwest::Client::new() };
        let id = &roots(tmp.path(), "alice")[0].id;
        assert!(resolve(&app, "alice", &format!("x_{id}"), "escape/passwd", false).is_err());
    }
    #[tokio::test]
    async fn no_token_never_reaches_files() {
        let app = App { root: "/".into(), userinfo: String::new(), client: reqwest::Client::new() };
        assert!(identity(&app, &HeaderMap::new()).await.is_err());
    }
    #[tokio::test]
    async fn rejects_expired_identity_even_with_http_200() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/userinfo", get(|| async { Json(json!({"code":10001,"preferred_username":"alice"})) }))).await.unwrap();
        });
        let app = App { root: "/".into(), userinfo: format!("http://{address}/userinfo"), client: reqwest::Client::new() };
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer fixture".parse().unwrap());
        assert!(identity(&app, &headers).await.is_err());
        server.abort();
    }
}
