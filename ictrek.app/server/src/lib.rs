//! VOS-side services for PhotoCraft.
//!
//! The web build is a static wasm page; everything that needs a process lives here, in the same
//! container as nginx:
//!
//! * [`files`] — the Finder file bridge (`/volumes`, `/index`, `/download`, `/upload`, …).
//! * [`album`] — the AI Album session exchange (`/album/session`).
//! * [`mcp`] — the agent surface (`/mcp`), the same tools the desktop app exposes on stdio.
//! * [`routes`] — the route table that mounts all of the above.
//!
//! Identity, path resolution and error shape each live in exactly one module, so a new endpoint
//! cannot quietly invent its own rules for them.

#![forbid(unsafe_code)]

pub mod album;
pub mod auth;
pub mod config;
pub mod error;
pub mod files;
pub mod mcp;
pub mod roots;
pub mod routes;

use std::{sync::Arc, time::Duration};

pub use config::Config;
pub use error::{Error, Result};
pub use routes::router;

/// Shared state for every handler.
#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    pub client: reqwest::Client,
    pub auth: Arc<auth::Authenticator>,
    /// `None` when agent traffic is disabled; the route is then not registered at all.
    pub mcp: Option<Arc<mcp::McpServices>>,
}

impl App {
    /// Fails only when the HTTP client cannot be built (a broken TLS backend), which must stop the
    /// process at startup rather than silently serving without the redirect policy.
    pub fn new(config: Config) -> std::io::Result<Self> {
        // One client for the whole process: userinfo and the album gateway both talk to the
        // platform, and redirects must never be followed with a token attached.
        let client =
            reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(std::io::Error::other)?;
        if let Some(user) = &config.dev_user {
            eprintln!(
                "[photocraft-storage] DEV identity active: every request is treated as `{user}` \
                 (PHOTOCRAFT_DEV=1 + PHOTOCRAFT_DEV_USER). Never run this outside a workstation."
            );
        }
        let auth = Arc::new(auth::Authenticator::new(client.clone(), config.userinfo_url.clone(), config.identity_cache_ttl, config.dev_user.clone()));
        let mcp = config.mcp.enabled.then(|| Arc::new(mcp::McpServices::new(config.exposed_root.clone(), &config.mcp)));
        Ok(Self { config: Arc::new(config), client, auth, mcp })
    }
}
