//! Startup configuration, read once from the environment VOS injects.
//!
//! Nothing here reads the environment after startup: a request must never observe a different
//! configuration from the one the process was launched with. Every value has the same default the
//! bridge used before this module existed, so an install that injects nothing behaves identically.

use std::{path::PathBuf, time::Duration};

/// Where the platform mounts the directories a user is allowed to reach.
const DEFAULT_STORAGE_PORT: u16 = 8081;
const DEFAULT_BIND_ADDR: &str = "0.0.0.0";
const DEFAULT_EXPOSED_ROOT: &str = "/exposed";
const DEFAULT_USERINFO: &str = "http://172.17.0.1:8105/v1000/oauth2/userinfo";
const DEFAULT_GATEWAY_HOST: &str = "172.17.0.1";
const DEFAULT_GATEWAY_PORT: u16 = 1182;
const DEFAULT_GATEWAY_EXTERNAL_PORT: u16 = 1180;
/// The bridge accepts the same 256 MiB the browser upload path uses.
const DEFAULT_BODY_LIMIT: usize = 256 * 1024 * 1024;
const DEFAULT_DOWNLOAD_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    /// Port the service listens on inside the container. nginx proxies `/storage/` here, so only a
    /// local smoke test changes it.
    pub storage_port: u16,
    /// Address the listener binds. The container binds everywhere (nginx talks to it over loopback
    /// and the platform may publish the port); `make vos-dev` narrows it to the docker gateway so a
    /// development identity cannot be reached from the LAN.
    pub bind_addr: String,
    /// Root of the VOS-exposed tree (`VOS_APP_EXPOSED_PATH`).
    pub exposed_root: PathBuf,
    /// OIDC userinfo endpoint used to turn a bearer token into a username.
    pub userinfo_url: String,
    /// Internal gateway other apps use to reach this one.
    pub gateway_host: String,
    pub gateway_port: u16,
    pub gateway_external_port: u16,
    /// Largest request body the file bridge accepts (uploads).
    pub request_body_limit: usize,
    /// Largest file the bridge hands to the browser.
    pub download_limit: u64,
    /// How long one userinfo verdict is reused for a token.
    pub identity_cache_ttl: Duration,
    /// Development-only identity override. `None` in every deployment; set by `make vos-dev`, where
    /// there is no VOS userinfo service. It must never be reachable from a production container.
    pub dev_user: Option<String>,
    pub mcp: McpSettings,
}

#[derive(Debug, Clone)]
pub struct McpSettings {
    /// Expose `POST /mcp`. Off for installs that must not accept agent traffic.
    pub enabled: bool,
    /// Distinct users whose MCP workspace stays resident at once.
    pub max_users: usize,
    /// Idle time after which an unused workspace is dropped.
    pub idle_timeout: Duration,
    /// Body ceiling for one JSON-RPC frame; the automation layer's own limit is 1 MiB.
    pub max_request_bytes: usize,
    /// `Host` values rmcp accepts. Empty disables the check, which is the right default behind
    /// Traefik: the deployment's hostname is not known at build time and the bearer token is the
    /// real gate.
    pub allowed_hosts: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage_port: DEFAULT_STORAGE_PORT,
            bind_addr: DEFAULT_BIND_ADDR.to_owned(),
            exposed_root: PathBuf::from(DEFAULT_EXPOSED_ROOT),
            userinfo_url: DEFAULT_USERINFO.to_owned(),
            gateway_host: DEFAULT_GATEWAY_HOST.to_owned(),
            gateway_port: DEFAULT_GATEWAY_PORT,
            gateway_external_port: DEFAULT_GATEWAY_EXTERNAL_PORT,
            request_body_limit: DEFAULT_BODY_LIMIT,
            download_limit: DEFAULT_DOWNLOAD_LIMIT,
            identity_cache_ttl: Duration::from_secs(60),
            dev_user: None,
            mcp: McpSettings { enabled: true, max_users: 8, idle_timeout: Duration::from_secs(30 * 60), max_request_bytes: 1 << 20, allowed_hosts: Vec::new() },
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            storage_port: number("PHOTOCRAFT_STORAGE_PORT", defaults.storage_port),
            bind_addr: text("PHOTOCRAFT_BIND_ADDR", &defaults.bind_addr),
            exposed_root: std::env::var_os("PHOTOCRAFT_EXPOSED_ROOT").map(PathBuf::from).unwrap_or(defaults.exposed_root),
            userinfo_url: text("VOS_OIDC_USERINFO_URL", &defaults.userinfo_url),
            gateway_host: text("VOS_HOST_GW_IP", &defaults.gateway_host),
            gateway_port: number("VOS_API_GW_PORT_INTERNAL", defaults.gateway_port),
            gateway_external_port: number("VOS_API_GW_PORT_EXTERNAL", defaults.gateway_external_port),
            request_body_limit: number("PHOTOCRAFT_BODY_LIMIT", defaults.request_body_limit),
            download_limit: number("PHOTOCRAFT_DOWNLOAD_LIMIT", defaults.download_limit),
            identity_cache_ttl: Duration::from_secs(number("PHOTOCRAFT_IDENTITY_TTL_SECS", defaults.identity_cache_ttl.as_secs())),
            dev_user: dev_user(defaults.dev_user),
            mcp: McpSettings {
                enabled: !matches!(text("PHOTOCRAFT_MCP", "1").trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"),
                max_users: number("PHOTOCRAFT_MCP_MAX_USERS", defaults.mcp.max_users),
                idle_timeout: Duration::from_secs(number("PHOTOCRAFT_MCP_IDLE_SECS", defaults.mcp.idle_timeout.as_secs())),
                max_request_bytes: number("PHOTOCRAFT_MCP_MAX_REQUEST_BYTES", defaults.mcp.max_request_bytes),
                allowed_hosts: list("PHOTOCRAFT_MCP_ALLOWED_HOSTS"),
            },
        }
    }
}

fn text(key: &str, default: &str) -> String {
    std::env::var(key).map(|value| value.trim().to_owned()).unwrap_or_else(|_| default.to_owned())
}

fn number<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key).ok().and_then(|value| value.trim().parse().ok()).unwrap_or(default)
}

/// Development identity: both switches are required, and the name must pass the same filter the
/// userinfo reply has to. `make vos-dev` sets them; a deployed container sets neither, so an
/// operator cannot enable this by accident with a single variable.
fn dev_user(default: Option<String>) -> Option<String> {
    let enabled = matches!(text("PHOTOCRAFT_DEV", "0").to_ascii_lowercase().as_str(), "1" | "true" | "yes");
    if !enabled {
        return default;
    }
    let name = text("PHOTOCRAFT_DEV_USER", "");
    let valid = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || "_-@.".contains(c)) && !matches!(name.as_str(), "." | "..");
    if valid { Some(name) } else { default }
}

fn list(key: &str) -> Vec<String> {
    std::env::var(key).unwrap_or_default().split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_bridge_contract() {
        let config = Config::default();
        assert_eq!(config.exposed_root, PathBuf::from("/exposed"));
        assert_eq!(config.request_body_limit, 256 * 1024 * 1024);
        assert_eq!(config.bind_addr, "0.0.0.0");
        assert!(config.mcp.enabled);
        assert_eq!(config.mcp.max_request_bytes, 1 << 20);
    }
}
