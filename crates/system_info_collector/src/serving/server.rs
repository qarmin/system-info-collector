use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use log::info;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::data_buffer::DataBuffer;

#[derive(Deserialize)]
struct DataQuery {
    limit: Option<usize>,
}

#[derive(Serialize)]
struct MetadataResponse {
    system_info: SystemInfoResponse,
    column_headers: Vec<String>,
    max_buffer_size: usize,
}

#[derive(Serialize)]
struct SystemInfoResponse {
    total_memory_mb: f64,
    total_swap_mb: f64,
    cpu_cores: usize,
    start_time: f64,
    app_version: String,
}

#[derive(Serialize)]
struct DataPointResponse {
    timestamp: f64,
    data: Vec<String>,
}

#[derive(Serialize)]
struct DataResponse {
    total_count: usize,
    max_buffer_size: usize,
    first: Option<DataPointResponse>,
    last: Option<DataPointResponse>,
}

#[derive(Serialize)]
struct RecentDataResponse {
    data: Vec<DataPointResponse>,
    count: usize,
    max_available: usize,
}

pub async fn start_server(port: u16, data_buffer: DataBuffer) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = Arc::new(data_buffer);

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/data", get(data_handler))
        .route("/api/data/recent", get(recent_data_handler))
        .route("/api/metadata", get(metadata_handler))
        .route("/static/chart.min.js", get(chartjs_handler))
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
    let metadata = buffer.get_metadata();
    let response = MetadataResponse {
        system_info: SystemInfoResponse {
            total_memory_mb: metadata.system_info.total_memory_mb,
            total_swap_mb: metadata.system_info.total_swap_mb,
            cpu_cores: metadata.system_info.cpu_cores,
            start_time: metadata.system_info.start_time,
            app_version: metadata.system_info.app_version,
        },
        column_headers: metadata.column_headers,
        max_buffer_size: metadata.max_buffer_size,
    };
    (StatusCode::OK, Json(response))
}

async fn data_handler(State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let (first, last) = buffer.get_first_and_last();
    let total_count = buffer.len();
    let max_size = buffer.get_max_size();

    let response = DataResponse {
        total_count,
        max_buffer_size: max_size,
        first: first.map(|d| DataPointResponse { timestamp: d.timestamp, data: d.data }),
        last: last.map(|d| DataPointResponse { timestamp: d.timestamp, data: d.data }),
    };
    (StatusCode::OK, Json(response))
}

async fn recent_data_handler(Query(params): Query<DataQuery>, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let max_size = buffer.get_max_size();
    let total_count = buffer.len();
    let limit = params.limit.unwrap_or(10000).min(max_size).min(total_count);
    let data_points = buffer.get_last_n(limit);

    let response = RecentDataResponse {
        data: data_points
            .into_iter()
            .map(|d| DataPointResponse { timestamp: d.timestamp, data: d.data })
            .collect(),
        count: limit,
        max_available: total_count,
    };
    (StatusCode::OK, Json(response))
}

async fn chartjs_handler() -> impl IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, "application/javascript")], include_bytes!("./chart.min.js") as &'static [u8])
}
