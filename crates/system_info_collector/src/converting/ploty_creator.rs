use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::time::Instant;

use anyhow::{Context, Error};
use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, Utc};
use log::info;
use system_info_collector_core::enums::{DataType, GeneralInfoGroup};
use system_info_collector_core::model::CollectedItemModels;
use system_info_collector_core::settings::{ConvertSettings, SplitMode};
use system_info_report::html::{self, HtmlChart, HtmlReport, HtmlTheme, HtmlTrace};
use time::UtcOffset;

use crate::converting::csv_file_loader::load_csv_results;

pub fn load_results_and_save_plot(settings: &ConvertSettings) -> Result<(), Error> {
    let time_start = Instant::now();
    let loaded_results = load_csv_results(settings)?;
    info!("Loading data took {:?}", time_start.elapsed());

    let timezone_ms = local_timezone_ms();

    let time_start = Instant::now();

    if settings.split_mode == SplitMode::Full {
        save_plot_into_file(&loaded_results, settings, timezone_ms)?;
        info!("Creating plot took {:?}", time_start.elapsed());

        if settings.open_plot_file {
            info!("Opening file {}", settings.plot_path);
            open::that(&settings.plot_path).context(format!("Failed to open {}", settings.plot_path))?;
        }
    } else {
        split_and_save_plots(&loaded_results, settings, timezone_ms)?;
        info!("Creating split plots took {:?}", time_start.elapsed());
    }

    Ok(())
}

// ── split-mode helpers ────────────────────────────────────────────────────────

fn abs_ts_to_naive_date(abs_ts: f64) -> Option<NaiveDate> {
    DateTime::from_timestamp(abs_ts as i64, 0).map(|dt: DateTime<Utc>| dt.date_naive())
}

fn period_plot_path(base_path: &str, period: &str) -> String {
    if let Some(stem) = base_path.strip_suffix(".html") {
        format!("{stem}_{period}.html")
    } else {
        format!("{base_path}_{period}.html")
    }
}

/// Return a new `CollectedItemModels` containing only the rows at `indices`.
///
/// `CPU_USAGE_PER_CORE` is stored transposed (one entry per core, semicolon-joined
/// timestamps), so it is handled separately.
fn slice_model_by_indices(model: &CollectedItemModels, indices: &[usize]) -> CollectedItemModels {
    let mut new_data: HashMap<DataType, Vec<String>> = HashMap::default();

    for (dt, values) in &model.collected_data {
        if *dt == DataType::CPU_USAGE_PER_CORE {
            // Each element is "v_ts0;v_ts1;..." for one core — slice the time axis.
            let sliced: Vec<String> = values
                .iter()
                .map(|core_str| {
                    let parts: Vec<&str> = core_str.split(';').collect();
                    indices.iter().filter_map(|&i| parts.get(i).copied()).collect::<Vec<_>>().join(";")
                })
                .collect();
            new_data.insert(dt.clone(), sliced);
        } else {
            let sliced: Vec<String> = indices.iter().filter_map(|&i| values.get(i).cloned()).collect();
            new_data.insert(dt.clone(), sliced);
        }
    }

    CollectedItemModels {
        collected_data: new_data,
        collected_groups: model.collected_groups.clone(),
        memory_total: model.memory_total,
        swap_total: model.swap_total,
        cpu_core_count: model.cpu_core_count,
        check_interval: model.check_interval,
        start_time: model.start_time,
        cpu_model: model.cpu_model.clone(),
        gpu_names: model.gpu_names.clone(),
        gpu_vram_mb: model.gpu_vram_mb.clone(),
        gpu_driver_versions: model.gpu_driver_versions.clone(),
        disk_labels: model.disk_labels.clone(),
        net_labels: model.net_labels.clone(),
    }
}

/// Build a model holding only the rows whose absolute timestamp is accepted by
/// `keep`.
///
/// When more than `max_points` rows survive they are thinned to evenly spaced
/// samples, so a long export still spans its whole period instead of covering
/// only the tail of it.
pub fn subset_by_time(model: &CollectedItemModels, keep: &dyn Fn(f64) -> bool, max_points: usize) -> CollectedItemModels {
    let Some(timestamps) = model.collected_data.get(&DataType::SECONDS_SINCE_START) else {
        return model.clone();
    };

    let mut indices: Vec<usize> = timestamps
        .iter()
        .enumerate()
        .filter(|(_, ts)| ts.parse::<f64>().is_ok_and(|t| keep(t + model.start_time)))
        .map(|(i, _)| i)
        .collect();

    let stride = indices.len().div_ceil(max_points.max(1)).max(1);
    if stride > 1 {
        info!(
            "Export covers {} points, keeping every {stride} to stay under {max_points}",
            indices.len()
        );
        indices = indices.into_iter().step_by(stride).collect();
    }

    slice_model_by_indices(model, &indices)
}

fn split_and_save_plots(model: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<(), Error> {
    let timestamps = model
        .collected_data
        .get(&DataType::SECONDS_SINCE_START)
        .context("Missing SECONDS_SINCE_START column")?;

    // Group row indices by the period key (day or week).
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, ts_str) in timestamps.iter().enumerate() {
        if let Ok(ts) = ts_str.parse::<f64>()
            && let Some(date) = abs_ts_to_naive_date(ts + model.start_time)
        {
            let key = match settings.split_mode {
                SplitMode::PerDay => date.format("%Y-%m-%d").to_string(),
                SplitMode::PerWeek => format!("{:04}-W{:02}", date.iso_week().year(), date.iso_week().week()),
                SplitMode::Full => unreachable!(),
            };
            groups.entry(key).or_default().push(i);
        }
    }

    info!("Split mode: generating {} file(s)", groups.len());

    for (period_key, indices) in &groups {
        let subset = slice_model_by_indices(model, indices);
        let period_path = period_plot_path(&settings.plot_path, period_key);
        let period_settings = ConvertSettings {
            plot_path: period_path.clone(),
            split_mode: SplitMode::Full,
            open_plot_file: false,
            ..settings.clone()
        };
        info!("  {period_path} ({} points)", indices.len());
        save_plot_into_file(&subset, &period_settings, timezone_ms)?;
    }

    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn local_timezone_ms() -> i64 {
    match UtcOffset::from_whole_seconds(chrono::offset::Local::now().offset().local_minus_utc()) {
        Ok(offset) => offset.whole_seconds() as i64 * 1000,
        Err(_) => 0,
    }
}

// ── main plot ─────────────────────────────────────────────────────────────────

pub fn save_plot_into_file(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<(), Error> {
    info!("Trying to create html file...");

    let html = build_report_html(loaded_results, settings, timezone_ms)?;
    fs::write(&settings.plot_path, html.as_bytes()).context(format!("Failed to write html plot file - {}", settings.plot_path))?;

    Ok(())
}

/// A single self-contained report holding the whole multi-chart plot.
pub fn build_report_html(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<String, Error> {
    let times = loaded_results
        .collected_data
        .get(&DataType::SECONDS_SINCE_START)
        .context("Missing SECONDS_SINCE_START column")?
        .iter()
        .map(|s| {
            let t = s.parse::<f64>().ok()?;
            let date = DateTime::from_timestamp_millis(((t + loaded_results.start_time) * 1000.0) as i64 + timezone_ms)?;
            Some(date.to_rfc3339_opts(SecondsFormat::AutoSi, true))
        })
        .collect::<Option<Vec<String>>>()
        .context("Failed to parse unix timestamp")?;

    Ok(html::render(&HtmlReport {
        times,
        charts: charts(loaded_results, settings.white_plot_mode),
        notes: notes(loaded_results),
        width: settings.plot_width,
        min_height: settings.plot_height,
        theme: if settings.white_plot_mode { HtmlTheme::White } else { HtmlTheme::Dark },
    }))
}

fn notes(loaded_results: &CollectedItemModels) -> Vec<String> {
    let cpu_label = if loaded_results.cpu_model.is_empty() {
        format!("CPU: {} cores", loaded_results.cpu_core_count)
    } else {
        format!("CPU: {} ({} cores)", loaded_results.cpu_model, loaded_results.cpu_core_count)
    };
    let mut notes_vec = vec![
        cpu_label,
        format!("Check interval: {}s", loaded_results.check_interval),
        format!(
            "Memory total: {}",
            humansize::format_size((loaded_results.memory_total * 1024.0 * 1024.0) as u64, humansize::BINARY)
        ),
        format!(
            "Swap total: {}",
            humansize::format_size((loaded_results.swap_total * 1024.0 * 1024.0) as u64, humansize::BINARY)
        ),
    ];
    for (idx, name) in loaded_results.gpu_names.iter().enumerate() {
        let vram_str = loaded_results
            .gpu_vram_mb
            .get(idx)
            .filter(|&&mb| mb > 0)
            .map(|&mb| format!(" ({} VRAM)", humansize::format_size(mb * 1024 * 1024, humansize::BINARY)))
            .unwrap_or_default();
        let driver_str = loaded_results
            .gpu_driver_versions
            .get(idx)
            .filter(|driver| !driver.is_empty())
            .map(|driver| format!(", driver {driver}"))
            .unwrap_or_default();
        notes_vec.push(format!("GPU {idx}: {name}{vram_str}{driver_str}"));
    }
    for (idx, label) in loaded_results.disk_labels.iter().enumerate() {
        notes_vec.push(format!("Disk {idx}: {label}"));
    }
    for (idx, label) in loaded_results.net_labels.iter().enumerate() {
        notes_vec.push(format!("Network {idx}: {label}"));
    }
    notes_vec
}

/// Values of one CSV column; empty cells and the `-1` the collector writes for an unreadable value
/// become gaps.
fn values(data: &[String]) -> Vec<Option<f64>> {
    data.iter().map(|v| if v == "-1" { None } else { v.parse().ok() }).collect()
}

/// One trace per column accepted by `belongs`, sorted by label, colored per device when `colored`.
fn traces(loaded_results: &CollectedItemModels, belongs: impl Fn(&DataType) -> bool, colored: Option<bool>) -> Vec<HtmlTrace> {
    let mut entries: Vec<_> = loaded_results.collected_data.iter().filter(|(dt, _)| belongs(dt)).collect();
    entries.sort_by_key(|(dt, _)| dt.pretty_print());
    entries
        .into_iter()
        .map(|(data_type, data)| HtmlTrace {
            name: data_type.pretty_print(),
            color: colored.and_then(|white| device_color(data_type, white)),
            values: values(data),
        })
        .collect()
}

fn cpu_traces(loaded_results: &CollectedItemModels) -> Vec<HtmlTrace> {
    let mut out = traces(loaded_results, |dt| dt.is_cpu() && *dt != DataType::CPU_USAGE_PER_CORE, None);
    if let Some(per_core) = loaded_results.collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        out.extend(per_core.iter().enumerate().map(|(index, core)| HtmlTrace {
            name: format!("Core {index}"),
            color: None,
            values: values(&core.split(';').map(ToString::to_string).collect::<Vec<_>>()),
        }));
    }
    out
}

/// Charts in a fixed order, each only when its data is present.
fn charts(loaded_results: &CollectedItemModels, white: bool) -> Vec<HtmlChart> {
    let groups = &loaded_results.collected_groups;
    let has_column = |test: fn(&DataType) -> bool| loaded_results.collected_data.keys().any(test);
    let has_disk = groups.contains(&GeneralInfoGroup::DISK);
    let chart = |y_title: &str, y_range: Option<(i64, i64)>, traces: Vec<HtmlTrace>| HtmlChart {
        y_title: y_title.to_string(),
        y_range,
        traces,
    };
    let device = Some(white);
    let percent = Some((-1, 100));
    let mut out = Vec::new();

    if groups.contains(&GeneralInfoGroup::MEMORY) {
        let range = Some((0, loaded_results.memory_total.ceil() as i64));
        out.push(chart("Memory Usage [MB]", range, traces(loaded_results, DataType::is_memory, None)));
    }
    if groups.contains(&GeneralInfoGroup::CPU) {
        out.push(chart("CPU Usage [%]", percent, cpu_traces(loaded_results)));
    }
    if groups.contains(&GeneralInfoGroup::SWAP) {
        let range = Some((0, loaded_results.swap_total.ceil() as i64));
        out.push(chart("Swap Usage [MB]", range, traces(loaded_results, DataType::is_swap, None)));
    }
    if groups.contains(&GeneralInfoGroup::NETWORK) {
        out.push(chart("Network [MB/s]", None, traces(loaded_results, DataType::is_network, device)));
    }
    if groups.contains(&GeneralInfoGroup::NETWORK_TOTAL) {
        out.push(chart(
            "Network Total [MB]",
            None,
            traces(loaded_results, DataType::is_network_total, device),
        ));
    }
    if has_column(DataType::is_gpu_util) {
        out.push(chart(
            "GPU Utilization [%]",
            percent,
            traces(loaded_results, DataType::is_gpu_util, device),
        ));
    }
    if has_column(DataType::is_gpu_vram) {
        // The largest GPU sets the ceiling; smaller ones just do not reach the top.
        let range = loaded_results
            .gpu_vram_mb
            .iter()
            .copied()
            .max()
            .filter(|&m| m > 0)
            .map(|max| (0, max as i64));
        out.push(chart("GPU VRAM [MB]", range, traces(loaded_results, DataType::is_gpu_vram, device)));
    }
    if has_column(DataType::is_gpu_temp) {
        out.push(chart("GPU Temperature [°C]", None, traces(loaded_results, DataType::is_gpu_temp, device)));
    }
    if has_disk && has_column(DataType::is_disk_used) {
        out.push(chart(
            "Disk Space Used [GB]",
            None,
            traces(loaded_results, DataType::is_disk_used, device),
        ));
    }
    if has_disk && has_column(DataType::is_disk_available) {
        out.push(chart(
            "Disk Space Available [GB]",
            None,
            traces(loaded_results, DataType::is_disk_available, device),
        ));
    }
    if groups.contains(&GeneralInfoGroup::DISK_BUSY) {
        out.push(chart("Disk Busy [%]", percent, traces(loaded_results, DataType::is_disk_busy, device)));
    }
    if groups.contains(&GeneralInfoGroup::DISK_IO) {
        out.push(chart("Disk I/O [MB/s]", None, traces(loaded_results, DataType::is_disk_io, device)));
    }
    if groups.contains(&GeneralInfoGroup::PING_LATENCY) {
        out.push(chart("Ping RTT [ms]", None, traces(loaded_results, DataType::is_ping_latency, device)));
    }
    if groups.contains(&GeneralInfoGroup::PING_LOSS) {
        out.push(chart("Ping Loss [%]", percent, traces(loaded_results, DataType::is_ping_loss, device)));
    }
    out
}

/// Hue assigned per device index, so disk 0 / GPU 0 / interface 0 keep one colour
/// across every chart they appear in.  Each device category starts from its own
/// base hue so the charts do not all end up in the same part of the wheel.
///
/// Paired series that share a chart (read/write, RX/TX) share the device hue and
/// differ only in intensity - the strong variant is read/RX, the muted one is
/// write/TX.
fn device_color(data_type: &DataType, white_mode: bool) -> Option<String> {
    const DISK_BASE: usize = 205;
    const GPU_BASE: usize = 25;
    const NET_BASE: usize = 105;
    const PING_BASE: usize = 320;
    const HUE_STEP: usize = 137;

    let (base, idx, strong) = match data_type {
        DataType::DISK_N_USED_GB((i, _))
        | DataType::DISK_N_AVAIL_GB((i, _))
        | DataType::DISK_N_BUSY_PCT((i, _))
        | DataType::DISK_N_READ_MBPS((i, _)) => (DISK_BASE, *i, true),
        DataType::DISK_N_WRITE_MBPS((i, _)) => (DISK_BASE, *i, false),
        DataType::GPU_N_UTIL((i, _)) | DataType::GPU_N_VRAM_MB((i, _)) | DataType::GPU_N_TEMP_C((i, _)) => (GPU_BASE, *i, true),
        DataType::GPU_UTILIZATION | DataType::GPU_MEMORY_USED | DataType::GPU_TEMPERATURE => (GPU_BASE, 0, true),
        DataType::NET_N_RX_BPS((i, _)) | DataType::NET_N_RX_TOTAL_MB((i, _)) => (NET_BASE, *i, true),
        DataType::NET_N_TX_BPS((i, _)) | DataType::NET_N_TX_TOTAL_MB((i, _)) => (NET_BASE, *i, false),
        DataType::NETWORK_RX_BYTES_PER_SEC => (NET_BASE, 0, true),
        DataType::NETWORK_TX_BYTES_PER_SEC => (NET_BASE, 0, false),
        DataType::PING_N_RTT_MS((i, _)) | DataType::PING_N_LOSS_PCT((i, _)) => (PING_BASE, *i, true),
        _ => return None,
    };

    // Strong and muted differ mainly in saturation - vivid vs washed-out reads as
    // the same colour at two intensities for every hue, whereas a lightness-only
    // split inverts on hues that are intrinsically dark (blues) or bright (yellows).
    //
    // The muted saturation floor is what keeps two devices apart on a shared chart:
    // pushed much below this it turns grey, and disk 0 write stops being tellable
    // from disk 1 write.
    let hue = (base + idx * HUE_STEP) % 360;
    Some(match (strong, white_mode) {
        (true, false) => format!("hsl({hue}, 100%, 65%)"),
        (false, false) => format!("hsl({hue}, 20%, 46%)"),
        (true, true) => format!("hsl({hue}, 95%, 38%)"),
        (false, true) => format!("hsl({hue}, 26%, 66%)"),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Map, Value};
    use system_info_collector_core::settings::ConvertSettings;

    use super::build_report_html;
    use crate::converting::csv_file_loader::load_csv_results;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// The figure plotly is handed, and the note lines under it.
    fn normalized(html: &str) -> Value {
        let start = html.find("Plotly.newPlot(graph_div, ").expect("plot call") + "Plotly.newPlot(graph_div, ".len();
        let mut stream = serde_json::Deserializer::from_str(html.get(start..).expect("in bounds")).into_iter::<Value>();
        let figure = stream.next().expect("figure").expect("valid JSON");

        let traces: Vec<Value> = figure["data"]
            .as_array()
            .expect("data")
            .iter()
            .map(|trace| {
                let y: Vec<Value> = trace["y"]
                    .as_array()
                    .expect("y")
                    .iter()
                    .map(|v| match v {
                        Value::String(s) if s.is_empty() => Value::Null,
                        Value::String(s) => s.parse::<f64>().map_or(Value::Null, Value::from),
                        other => other.clone(),
                    })
                    .collect();
                let mut out = Map::new();
                for key in ["name", "x", "xaxis", "yaxis"] {
                    out.insert(key.to_string(), trace[key].clone());
                }
                out.insert("y".to_string(), Value::Array(y));
                out.insert("color".to_string(), trace["line"]["color"].clone());
                Value::Object(out)
            })
            .collect();

        let notes: Vec<Value> = html
            .split("<div style=\"text-align: center;\">")
            .skip(1)
            .filter_map(|part| part.split_once("</div>").map(|(note, _)| Value::from(note)))
            .collect();

        serde_json::json!({ "traces": traces, "layout": figure["layout"], "notes": notes })
    }

    /// Set `UPDATE_PLOT_SNAPSHOT=1` to accept an intended change, then review the diff.
    #[test]
    fn report_matches_the_snapshot() {
        for (white, snapshot) in [(false, "all_metrics.dark.json"), (true, "all_metrics.white.json")] {
            let settings = ConvertSettings {
                data_path: fixture("all_metrics.csv").to_string_lossy().into_owned(),
                plot_width: 1700,
                plot_height: 800,
                white_plot_mode: white,
                ..ConvertSettings::default()
            };
            let model = load_csv_results(&settings).expect("fixture loads");
            let actual = normalized(&build_report_html(&model, &settings, 0).expect("report builds"));
            let path = fixture(snapshot);
            if std::env::var_os("UPDATE_PLOT_SNAPSHOT").is_some() {
                std::fs::write(&path, serde_json::to_string_pretty(&actual).expect("serializes")).expect("snapshot written");
            }
            let expected: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("snapshot exists")).expect("valid snapshot");
            assert!(
                actual == expected,
                "{snapshot} differs - rerun with UPDATE_PLOT_SNAPSHOT=1 and review the diff"
            );
        }
    }
}
