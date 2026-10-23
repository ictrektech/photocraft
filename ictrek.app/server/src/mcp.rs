//! MCP over Streamable HTTP: the same tools the desktop app exposes on stdio, reachable by an
//! agent on VOS.
//!
//! Three properties matter here and each is enforced by a different piece:
//!
//! * **Per-user isolation.** One `StreamableHttpService` per username, whose factory closes over
//!   that user's capability-scoped workspace. A request can never reach another user's service.
//! * **Continuity.** The service keeps a `PhotocraftMcp` per MCP session, and that object owns the
//!   `Headless` documents, so an agent's second tool call sees the first one's document. This
//!   relies on the client negotiating a protocol version that still has sessions; a client that
//!   negotiates `2026-07-28` is served statelessly by rmcp and loses the document between calls.
//! * **Bounded memory.** A workspace holds decoded documents, so the number of resident users is
//!   capped and idle ones are dropped. Age is measured by the last request, not by the session
//!   table, because rmcp only removes a session on an explicit close.
//!
//! The bearer token is re-checked on every request and only picks the workspace; an MCP session id
//! is routing, never authorisation.

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager};
use tokio::sync::Mutex;
use tower::ServiceExt;

use photocraft_automation::{AuthorizedWorkspace, PhotocraftMcp};

use crate::{
    App,
    config::McpSettings,
    error::{Error, Result},
    roots,
};

type Service = StreamableHttpService<PhotocraftMcp, LocalSessionManager>;

/// One user's resident workspace.
struct Workspace {
    service: Service,
    manager: Arc<LocalSessionManager>,
    last_used: Instant,
}

impl Workspace {
    fn sessions(&self) -> usize {
        // `sessions` is rmcp's session table. It is only read for capacity accounting, and a
        // momentarily unavailable lock must not turn into "this workspace is free".
        self.manager.sessions.try_read().map(|sessions| sessions.len()).unwrap_or(1)
    }

    /// Age is measured by use, not by the session table: rmcp removes a session only on an
    /// explicit close, so a client that disconnects without one would otherwise pin memory for the
    /// life of the process.
    fn is_idle(&self, timeout: Duration) -> bool {
        self.last_used.elapsed() > timeout
    }
}

pub struct McpServices {
    exposed_root: PathBuf,
    max_users: usize,
    idle_timeout: Duration,
    max_request_bytes: usize,
    allowed_hosts: Vec<String>,
    users: Mutex<HashMap<String, Workspace>>,
}

impl McpServices {
    pub fn new(exposed_root: PathBuf, settings: &McpSettings) -> Self {
        Self {
            exposed_root,
            max_users: settings.max_users.max(1),
            idle_timeout: settings.idle_timeout,
            max_request_bytes: settings.max_request_bytes,
            allowed_hosts: settings.allowed_hosts.clone(),
            users: Mutex::new(HashMap::new()),
        }
    }

    /// The service for one user, building (or recycling) a workspace as needed.
    async fn service_for(&self, username: &str) -> Result<Service> {
        let mut users = self.users.lock().await;
        users.retain(|_, workspace| !workspace.is_idle(self.idle_timeout));
        if let Some(workspace) = users.get_mut(username) {
            workspace.last_used = Instant::now();
            return Ok(workspace.service.clone());
        }
        if users.len() >= self.max_users {
            // Only a workspace nobody holds a session on may be recycled: evicting a live one
            // would abort another user's in-flight edit (a five-minute filter included) and report
            // it as `session not found`. When every resident workspace is in use, wait instead.
            let idle =
                users.iter().filter(|(_, workspace)| workspace.sessions() == 0).min_by_key(|(_, workspace)| workspace.last_used).map(|(name, _)| name.clone());
            let Some(name) = idle else {
                return Err(Error(StatusCode::TOO_MANY_REQUESTS, "All agent workspaces are in use; retry when one is idle"));
            };
            users.remove(&name);
        }
        let workspace = self.build(username)?;
        let service = workspace.service.clone();
        users.insert(username.to_owned(), workspace);
        Ok(service)
    }

    fn build(&self, username: &str) -> Result<Workspace> {
        let root = roots::workspace_root(&self.exposed_root, username).ok_or(Error(StatusCode::FORBIDDEN, "No authorized directory for this user"))?;
        // Read and write authority over the same directory: an agent may open a file and write a
        // new one next to it, but never leave the grant.
        let capability =
            AuthorizedWorkspace::new(Some(&root.path), Some(&root.path)).map_err(|_| Error(StatusCode::BAD_GATEWAY, "The agent workspace is unavailable"))?;

        let manager = Arc::new(LocalSessionManager::default());
        let request_body_limit = self.max_request_bytes;
        let config = if self.allowed_hosts.is_empty() {
            // Traefik fronted, bearer authenticated, reachable only from the platform network: the
            // Host allowlist would otherwise have to repeat every deployment's hostname.
            StreamableHttpServerConfig::default().disable_allowed_hosts()
        } else {
            StreamableHttpServerConfig::default().with_allowed_hosts(self.allowed_hosts.clone())
        }
        .with_max_request_body_bytes(request_body_limit);

        let factory = move || Ok::<PhotocraftMcp, io::Error>(PhotocraftMcp::headless_with_workspace(capability.clone()));
        let service = StreamableHttpService::new(factory, Arc::clone(&manager), config);
        Ok(Workspace { service, manager, last_used: Instant::now() })
    }
}

/// `POST/GET/DELETE /mcp`. The body is passed through untouched so rmcp can stream SSE.
pub async fn handler(State(app): State<App>, request: Request) -> Result<Response> {
    let Some(mcp) = app.mcp.as_ref() else {
        return Err(Error(StatusCode::NOT_FOUND, "Not found"));
    };
    let username = app.auth.identify(request.headers()).await?;
    let service = mcp.service_for(&username).await?;
    match service.oneshot(request).await {
        Ok(response) => Ok(response.map(axum::body::Body::new)),
        // `Infallible`, but spelled out instead of unwrapped.
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn a_user_without_a_grant_gets_no_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = McpServices::new(tmp.path().to_path_buf(), &Config::default().mcp);
        assert!(mcp.service_for("alice").await.is_err());
    }

    #[tokio::test]
    async fn the_same_user_reuses_one_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("v/users/alice/data")).unwrap();
        let mcp = McpServices::new(tmp.path().to_path_buf(), &Config::default().mcp);
        assert!(mcp.service_for("alice").await.is_ok());
        assert!(mcp.service_for("alice").await.is_ok());
        assert_eq!(mcp.users.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn the_cap_evicts_the_least_recently_used_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        for user in ["alice", "bob"] {
            std::fs::create_dir_all(tmp.path().join(format!("users/{user}/data"))).unwrap();
        }
        let mut settings = Config::default().mcp;
        settings.max_users = 1;
        let mcp = McpServices::new(tmp.path().to_path_buf(), &settings);
        assert!(mcp.service_for("alice").await.is_ok());
        assert!(mcp.service_for("bob").await.is_ok());
        let users = mcp.users.lock().await;
        assert_eq!(users.len(), 1);
        assert!(users.contains_key("bob"));
    }

    /// The wire contract an agent depends on: a session can be opened and the engine tools are
    /// announced. This exercises the transport, the factory and the tool router together.
    #[tokio::test]
    async fn a_session_initializes_and_announces_the_engine_tools() {
        use axum::{body::Body, body::to_bytes, http::Request};

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("users/alice/data")).unwrap();
        let mcp = McpServices::new(tmp.path().to_path_buf(), &Config::default().mcp);

        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "photocraft-test", "version": "0"}
            }
        });
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(initialize.to_string()))
            .unwrap();
        let service = mcp.service_for("alice").await.unwrap();
        let response = service.oneshot(request).await.unwrap();
        let status = response.status();
        let session = response.headers().get("mcp-session-id").and_then(|value| value.to_str().ok()).map(str::to_owned);
        let body = to_bytes(Body::new(response.into_body()), 1 << 20).await.unwrap();
        let text = String::from_utf8_lossy(&body).into_owned();
        assert_eq!(status, 200, "{text}");
        assert!(text.contains("serverInfo"), "{text}");
        let Some(session) = session else {
            // Stateless servers (protocol 2026-07-28) have no session to continue; that path is
            // documented as unsupported for multi-step work.
            return;
        };

        let list = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", session)
            .body(Body::from(serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}).to_string()))
            .unwrap();
        let service = mcp.service_for("alice").await.unwrap();
        let response = service.oneshot(list).await.unwrap();
        let body = to_bytes(Body::new(response.into_body()), 8 << 20).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        for tool in ["doc_open", "command_batch", "doc_render_preview", "doc_export"] {
            assert!(text.contains(tool), "{tool} missing from {text}");
        }
    }
}
