use anyhow::{Context, Result};
use axum::{
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use log::info;
use std::sync::Arc;

use super::data_buffer::DataBuffer;

pub async fn start_server(port: u16, data_buffer: DataBuffer) -> Result<()> {
    let app_state = Arc::new(data_buffer);

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/data", get(get_data))
        .with_state(app_state);

    let addr = format!("127.0.0.1:{}", port);
    info!("Server starting on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context(format!("Failed to bind to {}", addr))?;

    axum::serve(listener, app)
        .await
        .context("Server error")?;

    Ok(())
}

async fn serve_index() -> impl IntoResponse {
    Html(include_str!("server_index.html"))
}

async fn get_data(State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let data = buffer.get_all().await;
    Json(data)
}

