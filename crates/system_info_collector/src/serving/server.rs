use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

use super::data_buffer::{DataBuffer, TopDataPoint};

#[derive(Deserialize)]
struct DataQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ExportQuery {
    mode: Option<String>,
    date: Option<String>,
    week: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct ExportDatesResponse {
    days: Vec<String>,
    weeks: Vec<String>,
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
    cpu_physical_cores: usize,
    cpu_model: String,
    gpu_names: Vec<String>,
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

#[derive(Serialize)]
struct RecentTopResponse {
    data: Vec<TopDataPoint>,
    count: usize,
}

pub async fn start_server(port: u16, data_buffer: DataBuffer) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = Arc::new(data_buffer);

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/data", get(data_handler))
        .route("/api/data/recent", get(recent_data_handler))
        .route("/api/top/recent", get(recent_top_handler))
        .route("/api/metadata", get(metadata_handler))
        .route("/api/export/html", get(export_html_handler))
        .route("/api/export/dates", get(export_dates_handler))
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
            cpu_physical_cores: metadata.system_info.cpu_physical_cores,
            cpu_model: metadata.system_info.cpu_model,
            gpu_names: metadata.system_info.gpu_names,
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
        first: first.map(|d| DataPointResponse {
            timestamp: d.timestamp,
            data: d.data,
        }),
        last: last.map(|d| DataPointResponse {
            timestamp: d.timestamp,
            data: d.data,
        }),
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
            .map(|d| DataPointResponse {
                timestamp: d.timestamp,
                data: d.data,
            })
            .collect(),
        count: limit,
        max_available: total_count,
    };
    (StatusCode::OK, Json(response))
}

async fn recent_top_handler(Query(params): Query<DataQuery>, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let max_size = buffer.get_max_size();
    let limit = params.limit.unwrap_or(10000).min(max_size);
    let data_points = buffer.get_last_n_top(limit);
    let count = data_points.len();

    let response = RecentTopResponse { data: data_points, count };
    (StatusCode::OK, Json(response))
}

fn abs_ts_to_naive_date(abs_ts: f64) -> Option<NaiveDate> {
    DateTime::from_timestamp(abs_ts as i64, 0).map(|dt: DateTime<Utc>| dt.date_naive())
}

fn parse_week_str(s: &str) -> Option<(i32, u32)> {
    let (year_part, week_part) = s.split_once('-')?;
    let year: i32 = year_part.parse().ok()?;
    let week: u32 = week_part.strip_prefix('W')?.parse().ok()?;
    Some((year, week))
}

async fn export_dates_handler(State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let metadata = buffer.get_metadata();
    let start_time = metadata.system_info.start_time;
    let all_data = buffer.get_last_n(buffer.len());

    let mut days: BTreeSet<String> = BTreeSet::new();
    let mut weeks: BTreeSet<String> = BTreeSet::new();

    for p in &all_data {
        if let Some(date) = abs_ts_to_naive_date(start_time + p.timestamp) {
            days.insert(date.format("%Y-%m-%d").to_string());
            weeks.insert(format!("{:04}-W{:02}", date.iso_week().year(), date.iso_week().week()));
        }
    }

    (
        StatusCode::OK,
        Json(ExportDatesResponse {
            days: days.into_iter().collect(),
            weeks: weeks.into_iter().collect(),
        }),
    )
}

async fn export_html_handler(Query(params): Query<ExportQuery>, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let metadata = buffer.get_metadata();
    let start_time = metadata.system_info.start_time;
    let mode = params.mode.as_deref().unwrap_or("full");
    let total = buffer.len();

    let all_data = buffer.get_last_n(total);

    // Filter data to the requested period.
    let filtered: Vec<_> = match mode {
        "day" => {
            let target = params.date.as_deref().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            match target {
                Some(td) => all_data
                    .into_iter()
                    .filter(|p| abs_ts_to_naive_date(start_time + p.timestamp) == Some(td))
                    .collect(),
                None => all_data,
            }
        }
        "week" => {
            let target = params.week.as_deref().and_then(parse_week_str);
            match target {
                Some((yr, wk)) => all_data
                    .into_iter()
                    .filter(|p| {
                        abs_ts_to_naive_date(start_time + p.timestamp).is_some_and(|d| d.iso_week().year() == yr && d.iso_week().week() == wk)
                    })
                    .collect(),
                None => all_data,
            }
        }
        _ => all_data,
    };

    // Apply limit (take most-recent N).
    let limit = params.limit.unwrap_or(filtered.len());
    let data_to_use: Vec<_> = if filtered.len() > limit {
        filtered.into_iter().rev().take(limit).collect::<Vec<_>>().into_iter().rev().collect()
    } else {
        filtered
    };

    let top_data = buffer.get_last_n_top(limit);

    // Build JSON payload embedded in the HTML.
    let data_json: Vec<serde_json::Value> = data_to_use
        .iter()
        .map(|p| serde_json::json!({ "timestamp": p.timestamp, "data": p.data }))
        .collect();

    let top_json: Vec<serde_json::Value> = top_data
        .iter()
        .map(|p| {
            serde_json::json!({
                "timestamp": p.timestamp,
                "cpu": p.cpu.iter().map(|e| serde_json::json!({"name": e.name, "value": e.value})).collect::<Vec<_>>(),
                "ram": p.ram.iter().map(|e| serde_json::json!({"name": e.name, "value": e.value})).collect::<Vec<_>>(),
            })
        })
        .collect();

    let static_data = serde_json::json!({
        "metadata": {
            "system_info": {
                "total_memory_mb": metadata.system_info.total_memory_mb,
                "total_swap_mb": metadata.system_info.total_swap_mb,
                "cpu_cores": metadata.system_info.cpu_cores,
                "cpu_physical_cores": metadata.system_info.cpu_physical_cores,
                "cpu_model": metadata.system_info.cpu_model,
                "gpu_names": metadata.system_info.gpu_names,
                "start_time": metadata.system_info.start_time,
                "app_version": metadata.system_info.app_version,
            },
            "column_headers": metadata.column_headers,
            "max_buffer_size": metadata.max_buffer_size,
        },
        "data": data_json,
        "top_data": top_json,
    });

    let chart_js = include_str!("./chart.min.js");
    let static_script = format!(
        "<script>window.STATIC_DATA={};</script>",
        serde_json::to_string(&static_data).unwrap_or_default()
    );

    let mut html = include_str!("server_index.html").to_string();
    html = html.replace(r#"<script src="/static/chart.min.js"></script>"#, &format!("<script>{chart_js}</script>"));
    html = html.replace("</head>", &format!("{static_script}</head>"));

    ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
}

async fn chartjs_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_bytes!("./chart.min.js") as &'static [u8],
    )
}
