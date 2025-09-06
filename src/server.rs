use anyhow::Result;
use axum::{
    response::Html,
    routing::{get, get_service},
    Router,
};
use log::info;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

use crate::builder::SphinxBuilder;
use crate::watcher::FileWatcher;

pub struct LiveReloadServer {
    host: String,
    port: u16,
    output_dir: PathBuf,
    builder: SphinxBuilder,
    watcher: FileWatcher,
}

impl LiveReloadServer {
    pub async fn new(
        host: String,
        port: u16,
        output_dir: PathBuf,
        builder: SphinxBuilder,
        watcher: FileWatcher,
    ) -> Result<Self> {
        Ok(Self {
            host,
            port,
            output_dir,
            builder,
            watcher,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run(self) -> Result<()> {
        // Initial build
        info!("Performing initial build...");
        let stats = self.builder.build().await?;
        info!("Initial build completed in {:?}", stats.build_time);

        // Set up routes
        let app = Router::new()
            .route("/", get(index_handler))
            .route("/_live-reload", get(websocket_handler))
            .nest_service("/", get_service(ServeDir::new(&self.output_dir)));

        // Start server
        let addr = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&addr).await?;

        info!("Live reload server listening on http://{}", addr);

        // TODO: Start file watcher and rebuild on changes

        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn index_handler() -> Html<&'static str> {
    Html(
        r#"
<!DOCTYPE html>
<html>
<head>
    <title>Sphinx Ultra Live Reload</title>
    <script>
        const ws = new WebSocket(`ws://${location.host}/_live-reload`);
        ws.onmessage = function(event) {
            if (event.data === 'reload') {
                location.reload();
            }
        };
        ws.onopen = function() {
            console.log('Live reload connected');
        };
        ws.onerror = function() {
            console.log('Live reload connection error');
        };
    </script>
</head>
<body>
    <h1>Sphinx Ultra Documentation</h1>
    <p>Live reload server is active. Files will automatically refresh when changed.</p>
</body>
</html>
    "#,
    )
}

async fn websocket_handler() -> Result<axum::response::Response, axum::http::StatusCode> {
    // TODO: Implement WebSocket handler for live reload
    Err(axum::http::StatusCode::NOT_IMPLEMENTED)
}
