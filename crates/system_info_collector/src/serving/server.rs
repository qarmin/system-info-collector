use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Cursor, Write};
use std::sync::Arc;
use system_info_collector_core::enums::DataType;
use system_info_collector_core::model::CollectedItemModels;
use system_info_collector_core::settings::{ConvertSettings, SplitMode};
use tokio::sync::broadcast::error::RecvError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::data_buffer::{DataBuffer, DataPoint, TopDataPoint};
use crate::converting::csv_file_loader::{load_csv_results, scan_timestamps};
use crate::converting::ploty_creator::{build_report_html, subset_by_time};

/// Rows selected for an export are capped so a huge buffer cannot stall the
/// server while plotly renders.
const MAX_EXPORT_POINTS: usize = 100_000;

#[derive(Deserialize)]
struct RangeQuery {
    /// Only return points from the last N seconds (relative to the newest point).
    seconds: Option<f64>,
    limit: Option<usize>,
}

#[derive(Clone, Deserialize)]
struct ExportQuery {
    /// `full`, `last`, `day` or `week`.
    mode: Option<String>,
    seconds: Option<f64>,
    date: Option<String>,
    week: Option<String>,
    /// `day` or `week` splits the report into one file per period, delivered as a zip.
    split: Option<String>,
}

#[derive(Serialize)]
struct ExportDatesResponse {
    days: Vec<String>,
    weeks: Vec<String>,
}

#[derive(Serialize)]
struct DataPointResponse {
    timestamp: f64,
    data: Vec<String>,
}

#[derive(Serialize)]
struct SnapshotResponse {
    data: Vec<DataPointResponse>,
    top_data: Vec<TopDataPoint>,
    total_count: usize,
    max_buffer_size: usize,
    first_timestamp: Option<f64>,
    last_timestamp: Option<f64>,
}

/// Data files written by this run, used as the source for exported reports.
pub struct ExportPaths {
    pub data_path: String,
    /// Top-N process files, when `--top-n-processes` is active.
    pub extra_data_paths: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    buffer: Arc<DataBuffer>,
    export: Arc<ExportPaths>,
}

impl FromRef<AppState> for Arc<DataBuffer> {
    fn from_ref(state: &AppState) -> Self {
        Self::clone(&state.buffer)
    }
}

impl FromRef<AppState> for Arc<ExportPaths> {
    fn from_ref(state: &AppState) -> Self {
        Self::clone(&state.export)
    }
}

pub async fn start_server(port: u16, data_buffer: DataBuffer, export_paths: ExportPaths) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = AppState {
        buffer: Arc::new(data_buffer),
        export: Arc::new(export_paths),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/metadata", get(metadata_handler))
        .route("/api/snapshot", get(snapshot_handler))
        .route("/api/ws", get(ws_handler))
        .route("/api/export/html", get(export_html_handler))
        .route("/api/export/report", get(export_report_handler))
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
    (StatusCode::OK, Json(buffer.get_metadata()))
}

/// One-shot bootstrap for the web UI: history for the selected range plus the
/// buffer counters.  After this the browser only receives websocket deltas.
async fn snapshot_handler(Query(params): Query<RangeQuery>, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let data_points = buffer.get_range(params.seconds, params.limit);
    let top_data = buffer.get_range_top(params.seconds, params.limit);
    let (first, last) = buffer.get_first_and_last();

    let response = SnapshotResponse {
        data: data_points
            .into_iter()
            .map(|d| DataPointResponse {
                timestamp: d.timestamp,
                data: d.data,
            })
            .collect(),
        top_data,
        total_count: buffer.len(),
        max_buffer_size: buffer.get_max_size(),
        first_timestamp: first.map(|d| d.timestamp),
        last_timestamp: last.map(|d| d.timestamp),
    };
    (StatusCode::OK, Json(response))
}

async fn ws_handler(ws: WebSocketUpgrade, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| push_live_updates(socket, buffer))
}

/// Forward each tick to one connected browser.  The frame arrives already
/// serialized, so this only bumps a refcount and writes it to the socket.
async fn push_live_updates(mut socket: WebSocket, buffer: Arc<DataBuffer>) {
    let mut updates = buffer.subscribe();
    loop {
        tokio::select! {
            received = updates.recv() => match received {
                Ok(frame) => {
                    if socket.send(Message::Text(frame)).await.is_err() {
                        break;
                    }
                }
                // A lagging client just skips ahead; it will catch up on the next reload.
                Err(RecvError::Lagged(skipped)) => info!("Websocket client lagged, skipped {skipped} updates"),
                Err(RecvError::Closed) => break,
            },
            // Only used to notice a disconnect - the client never sends commands.
            incoming = socket.recv() => if incoming.is_none() { break },
        }
    }
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

/// Days and weeks offered in the export dialog.  Scanned from the CSV (only its
/// timestamp column) rather than the buffer, so periods older than
/// `--max-results` are selectable too.
async fn export_dates_handler(State(paths): State<Arc<ExportPaths>>) -> impl IntoResponse {
    let data_path = paths.data_path.clone();
    let scanned = tokio::task::spawn_blocking(move || scan_timestamps(&data_path)).await;

    let (start_time, timestamps) = match scanned {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            error!("Failed to scan {} for export dates: {e}", paths.data_path);
            (0.0, vec![])
        }
        Err(e) => {
            error!("Export date scan failed: {e}");
            (0.0, vec![])
        }
    };

    let mut days: BTreeSet<String> = BTreeSet::new();
    let mut weeks: BTreeSet<String> = BTreeSet::new();
    for timestamp in timestamps {
        if let Some(date) = abs_ts_to_naive_date(start_time + timestamp) {
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

/// Which rows of the *live buffer* to include in a dashboard snapshot.
fn select_buffer_data(buffer: &DataBuffer, params: &ExportQuery, start_time: f64) -> (Vec<DataPoint>, Vec<TopDataPoint>) {
    match params.mode.as_deref().unwrap_or("full") {
        "last" => {
            let seconds = params.seconds.unwrap_or(3600.0);
            (
                buffer.get_range(Some(seconds), Some(MAX_EXPORT_POINTS)),
                buffer.get_range_top(Some(seconds), Some(MAX_EXPORT_POINTS)),
            )
        }
        "day" | "week" => {
            let keep = period_filter(params);
            (
                buffer
                    .get_range(None, None)
                    .into_iter()
                    .filter(|p| keep(start_time + p.timestamp))
                    .collect(),
                buffer
                    .get_range_top(None, None)
                    .into_iter()
                    .filter(|p| keep(start_time + p.timestamp))
                    .collect(),
            )
        }
        _ => (
            buffer.get_range(None, Some(MAX_EXPORT_POINTS)),
            buffer.get_range_top(None, Some(MAX_EXPORT_POINTS)),
        ),
    }
}

/// Predicate over absolute timestamps describing the requested export period.
fn period_filter(params: &ExportQuery) -> Box<dyn Fn(f64) -> bool + Send> {
    match params.mode.as_deref().unwrap_or("full") {
        "day" => {
            let target = params.date.as_deref().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            Box::new(move |ts| target.is_none_or(|td| abs_ts_to_naive_date(ts) == Some(td)))
        }
        "week" => {
            let target = params.week.as_deref().and_then(parse_week_str);
            Box::new(move |ts| {
                target.is_none_or(|(yr, wk)| abs_ts_to_naive_date(ts).is_some_and(|d| d.iso_week().year() == yr && d.iso_week().week() == wk))
            })
        }
        _ => Box::new(|_| true),
    }
}

fn export_slug(params: &ExportQuery) -> String {
    match params.mode.as_deref().unwrap_or("full") {
        "last" => format!("last_{}s", params.seconds.unwrap_or(3600.0) as i64),
        "day" | "week" => params
            .date
            .clone()
            .or_else(|| params.week.clone())
            .unwrap_or_else(|| "period".to_string())
            .replace(['/', '\\', ' '], "_"),
        _ => "full".to_string(),
    }
}

fn download_headers(file_name: &str) -> [(axum::http::HeaderName, String); 2] {
    [
        (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
        (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file_name}\"")),
    ]
}

fn plain_text(status: StatusCode, body: String) -> axum::response::Response {
    (status, [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

/// Self-contained copy of the live dashboard (Chart.js) with the data baked in.
///
/// Unlike the plotly report this is a snapshot of what the page currently shows,
/// so it is bounded by the live buffer (`--max-results`).
async fn export_html_handler(Query(params): Query<ExportQuery>, State(buffer): State<Arc<DataBuffer>>) -> impl IntoResponse {
    let metadata = buffer.get_metadata();
    let (data_to_use, top_data) = select_buffer_data(&buffer, &params, metadata.system_info.start_time);

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
        "metadata": metadata,
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

    (download_headers(&export_file_name("dashboard", &export_slug(&params), "html")), html)
}

/// Plotly report - the same format the `convert` command produces.
///
/// Rendered from the CSV file on disk, not from the live buffer, so it covers
/// the whole recorded history no matter how small `--max-results` is.
async fn export_report_handler(Query(params): Query<ExportQuery>, State(paths): State<Arc<ExportPaths>>) -> axum::response::Response {
    // Parsing the CSV and rendering plotly are both heavy and fully synchronous,
    // so the whole job goes to a blocking thread.
    match tokio::task::spawn_blocking(move || build_report_export(&paths, &params)).await {
        Ok(Ok(file)) => (
            [
                (axum::http::header::CONTENT_TYPE, file.content_type.to_string()),
                (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", file.name)),
            ],
            file.bytes,
        )
            .into_response(),
        Ok(Err(e)) => {
            error!("Export failed: {e}");
            plain_text(StatusCode::INTERNAL_SERVER_ERROR, format!("Export failed: {e}"))
        }
        Err(e) => {
            error!("Export task failed: {e}");
            plain_text(StatusCode::INTERNAL_SERVER_ERROR, "Export task failed".to_string())
        }
    }
}

struct ExportFile {
    name: String,
    content_type: &'static str,
    bytes: Vec<u8>,
}

fn build_report_export(paths: &ExportPaths, params: &ExportQuery) -> Result<ExportFile, anyhow::Error> {
    let settings = ConvertSettings {
        data_path: paths.data_path.clone(),
        extra_data_paths: paths.extra_data_paths.clone(),
        plot_width: 1700,
        plot_height: 800,
        split_mode: SplitMode::Full,
        ..ConvertSettings::default()
    };
    let model = load_csv_results(&settings)?;
    let timezone_ms = i64::from(Local::now().offset().local_minus_utc()) * 1000;

    // "last N seconds" is relative to the newest row in the file.
    let newest = model
        .collected_data
        .get(&DataType::SECONDS_SINCE_START)
        .and_then(|ts| ts.last())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|t| t + model.start_time);

    let keep: Box<dyn Fn(f64) -> bool + Send> = match (params.mode.as_deref(), newest) {
        (Some("last"), Some(newest)) => {
            let cutoff = newest - params.seconds.unwrap_or(3600.0);
            Box::new(move |ts| ts >= cutoff)
        }
        _ => period_filter(params),
    };

    let Some(split) = params.split.as_deref().filter(|s| *s == "day" || *s == "week") else {
        let subset = subset_by_time(&model, &keep, MAX_EXPORT_POINTS);
        if row_count(&subset) == 0 {
            return Err(anyhow::Error::msg("No data in the selected period"));
        }
        info!("Building plotly report for {} points", row_count(&subset));
        return Ok(ExportFile {
            name: export_file_name("report", &export_slug(params), "html"),
            content_type: "text/html; charset=utf-8",
            bytes: build_report_html(&subset, &settings, timezone_ms)?.into_bytes(),
        });
    };

    let periods = distinct_periods(&model, &keep, split);
    if periods.is_empty() {
        return Err(anyhow::Error::msg("No data in the selected period"));
    }
    info!("Building {} split report file(s) by {split}", periods.len());

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for period in &periods {
        let subset = subset_by_time(
            &model,
            &|ts| keep(ts) && period_key(ts, split).as_ref() == Some(period),
            MAX_EXPORT_POINTS,
        );
        if row_count(&subset) == 0 {
            continue;
        }
        zip.start_file(format!("system_info_report_{period}.html"), options)?;
        zip.write_all(build_report_html(&subset, &settings, timezone_ms)?.as_bytes())?;
    }

    Ok(ExportFile {
        name: export_file_name("reports", &format!("per_{split}"), "zip"),
        content_type: "application/zip",
        bytes: zip.finish()?.into_inner(),
    })
}

fn row_count(model: &CollectedItemModels) -> usize {
    model.collected_data.get(&DataType::SECONDS_SINCE_START).map_or(0, Vec::len)
}

/// Calendar key an absolute timestamp belongs to, for split exports.
fn period_key(abs_ts: f64, split: &str) -> Option<String> {
    let date = abs_ts_to_naive_date(abs_ts)?;
    Some(match split {
        "week" => format!("{:04}-W{:02}", date.iso_week().year(), date.iso_week().week()),
        _ => date.format("%Y-%m-%d").to_string(),
    })
}

fn distinct_periods(model: &CollectedItemModels, keep: &dyn Fn(f64) -> bool, split: &str) -> Vec<String> {
    let Some(timestamps) = model.collected_data.get(&DataType::SECONDS_SINCE_START) else {
        return vec![];
    };
    timestamps
        .iter()
        .filter_map(|s| s.parse::<f64>().ok())
        .map(|t| t + model.start_time)
        .filter(|ts| keep(*ts))
        .filter_map(|ts| period_key(ts, split))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn export_file_name(kind: &str, slug: &str, extension: &str) -> String {
    format!("system_info_{kind}_{slug}_{}.{extension}", Local::now().format("%Y-%m-%d_%H-%M-%S"))
}

async fn chartjs_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_bytes!("./chart.min.js") as &'static [u8],
    )
}
