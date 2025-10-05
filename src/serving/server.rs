use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use log::info;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use super::data_buffer::DataBuffer;

#[derive(Deserialize)]
struct DataQuery {
    limit: Option<usize>,
}

pub async fn start_server(port: u16, data_buffer: DataBuffer) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = Arc::new(data_buffer);

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/data", get(data_handler))
        .route("/api/data/recent", get(recent_data_handler))
        .with_state(app_state);

    let addr = format!("0.0.0.0:{port}");
    info!("Starting HTTP server on {addr}");

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

async fn recent_data_handler(Query(params): Query<DataQuery>, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(10).min(100); // Maksymalnie 100 wyników na raz
    let data_points = buffer.get_last_n(limit).await;

    let response = json!({
        "data": data_points.iter().map(|d| json!({
            "timestamp": d.timestamp,
            "data": d.data
        })).collect::<Vec<_>>(),
        "count": data_points.len()
    });

    (StatusCode::OK, Json(response))
}
