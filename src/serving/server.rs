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
        .route("/api/metadata", get(metadata_handler))
        .with_state(app_state);

    let addr = format!("0.0.0.0:{port}");
    info!("Starting HTTP server on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn index_handler() -> impl IntoResponse {
    Html(include_str!("server_index.html"))
}

async fn metadata_handler(State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let metadata = buffer.get_metadata().await;

    (StatusCode::OK, Json(json!({
        "system_info": {
            "total_memory_mb": metadata.system_info.total_memory_mb,
            "total_swap_mb": metadata.system_info.total_swap_mb,
            "cpu_cores": metadata.system_info.cpu_cores,
            "start_time": metadata.system_info.start_time,
            "app_version": metadata.system_info.app_version
        },
        "column_headers": metadata.column_headers,
        "max_buffer_size": metadata.max_buffer_size
    })))
}

async fn data_handler(State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let (first, last) = buffer.get_first_and_last().await;
    let total_count = buffer.len().await;
    let max_size = buffer.get_max_size().await;

    let response = json!({
        "total_count": total_count,
        "max_buffer_size": max_size,
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
    let max_size = buffer.get_max_size().await;
    let total_count = buffer.len().await;
    let limit = params.limit.unwrap_or(10).min(max_size).min(total_count); // Respect actual buffer size
    let data_points = buffer.get_last_n(limit).await;

    let response = json!({
        "data": data_points.iter().map(|d| json!({
            "timestamp": d.timestamp,
            "data": d.data
        })).collect::<Vec<_>>(),
        "count": data_points.len(),
        "max_available": total_count
    });

    (StatusCode::OK, Json(response))
}
