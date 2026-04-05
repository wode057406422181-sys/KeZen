mod routes;

use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;

use crate::config::AppConfig;

pub async fn run_server(config: AppConfig, host: String, port: u16) -> anyhow::Result<()> {
    let addr_str = format!("{}:{}", host, port);
    let addr: SocketAddr = addr_str.parse()?;

    tracing::info!(addr = %addr, "HTTP server started");

    let app = Router::new()
        .nest("/health", routes::health_router())
        .nest("/v1", routes::v1_router())
        .with_state(config);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
