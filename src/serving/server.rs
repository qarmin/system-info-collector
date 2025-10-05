use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use log::info;
use serde_json::json;
use std::sync::Arc;

use super::data_buffer::DataBuffer;

pub async fn start_server(port: u16, data_buffer: DataBuffer) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = Arc::new(data_buffer);

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/data", get(data_handler))
        .with_state(app_state);

    let addr = format!("0.0.0.0:{}", port);
    info!("Starting HTTP server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn index_handler() -> impl IntoResponse {
    Html(include_str!("server_index.html"))
}

async fn data_handler(State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let (first, last) = buffer.get_first_and_last().await;
    let total_count = buffer.len().await;

    let response = json!({
        "total_count": total_count,
        "first": first.as_ref().map(|d| json!({
            "timestamp": d.timestamp,
            "data": d.data
        })),
        "last": last.as_ref().map(|d| json!({
            "timestamp": d.timestamp,
            "data": d.data
        }))
    });

    (StatusCode::OK, Json(response))
}

