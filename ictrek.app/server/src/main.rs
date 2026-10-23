//! Entry point: build the configured service and serve it on the container's internal port.
//!
//! All behaviour lives in the library so it can be tested without binding a port.

use photocraft_storage::{App, Config, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    let address = format!("{}:{}", config.bind_addr, config.storage_port);
    let app = App::new(config)?;
    let listener = tokio::net::TcpListener::bind(&address).await?;
    axum::serve(listener, router(app)).await?;
    Ok(())
}
