use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::time::Instant;

use anyhow::{Context, Error};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use log::info;
use plotly::common::Title;
use plotly::layout::themes::PLOTLY_DARK;
use plotly::layout::{Axis, AxisRange, GridPattern, Layout, LayoutGrid};
use plotly::{Plot, Scatter};
use regex::Regex;
use system_info_collector_core::enums::{DataType, GeneralInfoGroup};
use system_info_collector_core::model::CollectedItemModels;
use system_info_collector_core::settings::{ConvertSettings, SplitMode};
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

fn minify_html(html: &str) -> String {
    let regex = Regex::new(r"\n[ ]+").expect("Regex is invalid");
    regex.replace_all(html, "").into_owned()
}

fn apply_style(html: String, settings: &ConvertSettings) -> String {
    if settings.white_plot_mode {
        html
    } else {
        html.replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>")
    }
}

// ── per-chart legend injection ────────────────────────────────────────────────

/// Returns a `<script type="module">` block that redistributes the single
/// Plotly legend into one legend per subplot, each positioned at the vertical
/// mid-point of its subplot.
///
/// Plotly.js ≥ 2.16 supports multiple legends (`legend`, `legend2`, …) in the
/// layout and the `legend` property on each trace.  plotly-rs 0.14 does not
/// expose this API directly, so we inject the post-processing via JavaScript.
fn per_chart_legends_script() -> &'static str {
    r#"<script type="module">
    (async () => {
        const plotDiv = document.getElementById('plotly-html-element');
        while (!plotDiv._fullLayout) await new Promise(r => setTimeout(r, 50));
        const fl = plotDiv._fullLayout;
        const data = plotDiv.data;
        const yaxisOrder = [];
        const yaxisSeen = new Set();
        data.forEach(t => {
            const y = t.yaxis || 'y';
            if (!yaxisSeen.has(y)) { yaxisSeen.add(y); yaxisOrder.push(y); }
        });
        if (yaxisOrder.length <= 1) return;
        const legendMap = {};
        yaxisOrder.forEach((y, i) => { legendMap[y] = i === 0 ? 'legend' : 'legend' + (i + 1); });
        const layoutUpdate = {};
        yaxisOrder.forEach(y => {
            const axisKey = y === 'y' ? 'yaxis' : 'yaxis' + y.slice(1);
            const domain = fl[axisKey]?.domain ?? [0, 1];
            const yMid = (domain[0] + domain[1]) / 2;
            layoutUpdate[legendMap[y]] = { y: yMid, yanchor: 'middle', x: 1.02, xanchor: 'left', tracegroupgap: 0 };
        });
        const legendRefs = data.map(t => legendMap[t.yaxis || 'y']);
        await Plotly.relayout(plotDiv, layoutUpdate);
        await Plotly.restyle(plotDiv, { legend: legendRefs });
    })();
</script>"#
}

// ── main plot ─────────────────────────────────────────────────────────────────

/// vendor/plotly patches the upstream `Layout` struct (capped at axis 8) up to
/// axis 12 - see vendor/plotly/src/layout/mod.rs and `set_axes` below.
const MAX_CHART_ROWS: usize = 12;

/// Fine-grained chart groups.  GPU is split into three separate charts so that
/// utilisation (%), VRAM (MB), and temperature (°C) each get their own axis.
/// Top-N process data is rendered as separate standalone HTML files.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum ChartGroup {
    Memory,
    Cpu,
    Swap,
    Network,
    NetworkTotal,
    GpuUtil,
    GpuVram,
    GpuTemp,
    DiskUsed,
    DiskAvailable,
    DiskBusy,
    DiskIo,
}

pub fn save_plot_into_file(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<(), Error> {
    info!("Trying to create html file...");

    let html = build_report_html(loaded_results, settings, timezone_ms)?;
    fs::write(&settings.plot_path, html.as_bytes()).context(format!("Failed to write html plot file - {}", settings.plot_path))?;

    Ok(())
}

/// A single self-contained report holding the whole multi-chart plot.
pub fn build_report_html(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<String, Error> {
    let plot = build_main_plot(loaded_results, settings, timezone_ms)?;

    let html = apply_style(plot.to_html(), settings);
    let body_suffix = format!("{}\n{}", notes_html(loaded_results), per_chart_legends_script());
    let html = html.replace("</body>", &format!("{body_suffix}\n</body>"));

    Ok(minify_html(&html))
}

fn build_main_plot(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<Plot, Error> {
    let dates = loaded_results
        .collected_data
        .get(&DataType::SECONDS_SINCE_START)
        .context("Missing SECONDS_SINCE_START column")?
        .iter()
        .map(|s| {
            if let Ok(t) = s.parse::<f64>() {
                DateTime::from_timestamp_millis(((t + loaded_results.start_time) * 1000.0) as i64 + timezone_ms)
            } else {
                None
            }
        })
        .collect::<Option<Vec<DateTime<Utc>>>>()
        .context("Failed to parse unix timestamp")?;

    let mut plot = Plot::new();
    let (layout, layout_info) = create_plot_layout(loaded_results, settings);
    plot.set_layout(layout);

    if let Some(&i) = layout_info.get(&ChartGroup::Memory) {
        create_memory_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::Cpu) {
        create_cpu_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::Swap) {
        create_swap_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::Network) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_network, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::NetworkTotal) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_network_total, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::GpuUtil) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_gpu_util, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::GpuVram) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_gpu_vram, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::GpuTemp) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_gpu_temp, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::DiskUsed) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_disk_used, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::DiskAvailable) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_disk_available, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::DiskBusy) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_disk_busy, settings);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::DiskIo) {
        create_traces(&mut plot, &dates, loaded_results, i, DataType::is_disk_io, settings);
    }

    Ok(plot)
}

fn notes_html(loaded_results: &CollectedItemModels) -> String {
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
        notes_vec.push(format!("GPU {idx}: {name}{vram_str}"));
    }
    for (idx, label) in loaded_results.disk_labels.iter().enumerate() {
        notes_vec.push(format!("Disk {idx}: {label}"));
    }
    for (idx, label) in loaded_results.net_labels.iter().enumerate() {
        notes_vec.push(format!("Network {idx}: {label}"));
    }

    #[expect(clippy::format_collect)]
    let notes = notes_vec
        .iter()
        .map(|e| format!("<div style=\"text-align: center;\">{e}</div>"))
        .collect::<String>();
    notes
}

fn create_plot_layout(loaded_results: &CollectedItemModels, settings: &ConvertSettings) -> (Layout, HashMap<ChartGroup, u32>) {
    let groups = &loaded_results.collected_groups;
    let has_memory = groups.contains(&GeneralInfoGroup::MEMORY);
    let has_cpu = groups.contains(&GeneralInfoGroup::CPU);
    let has_swap = groups.contains(&GeneralInfoGroup::SWAP);
    let has_network = groups.contains(&GeneralInfoGroup::NETWORK);
    let has_network_total = groups.contains(&GeneralInfoGroup::NETWORK_TOTAL);

    // GPU split into three independent sub-charts based on what data is present.
    let has_gpu_util = loaded_results.collected_data.keys().any(DataType::is_gpu_util);
    let has_gpu_vram = loaded_results.collected_data.keys().any(DataType::is_gpu_vram);
    let has_gpu_temp = loaded_results.collected_data.keys().any(DataType::is_gpu_temp);
    let has_disk = groups.contains(&GeneralInfoGroup::DISK);
    let has_disk_used = has_disk && loaded_results.collected_data.keys().any(DataType::is_disk_used);
    let has_disk_available = has_disk && loaded_results.collected_data.keys().any(DataType::is_disk_available);
    let has_disk_busy = groups.contains(&GeneralInfoGroup::DISK_BUSY);
    let has_disk_io = groups.contains(&GeneralInfoGroup::DISK_IO);

    let rows = has_memory as usize
        + has_cpu as usize
        + has_swap as usize
        + has_network as usize
        + has_network_total as usize
        + has_gpu_util as usize
        + has_gpu_vram as usize
        + has_gpu_temp as usize
        + has_disk_used as usize
        + has_disk_available as usize
        + has_disk_busy as usize
        + has_disk_io as usize;

    // Upstream plotly caps out at 8 named axes; vendor/plotly patches in xaxis9-12
    // (see set_axes below), so the grid can go up to MAX_CHART_ROWS.
    let capped_rows = rows.min(MAX_CHART_ROWS);
    let dynamic_height = settings.plot_height.max(capped_rows as u32 * 380);

    let mut layout = Layout::new()
        .width(settings.plot_width as usize)
        .height(dynamic_height as usize)
        .grid(LayoutGrid::new().rows(capped_rows).columns(1).pattern(GridPattern::Independent));

    if !settings.white_plot_mode {
        layout = layout.template(&*PLOTLY_DARK);
    }

    let mut idx_info: HashMap<ChartGroup, u32> = HashMap::default();
    let x_axis = Axis::new().title(Title::with_text("Time"));
    let mut current = 1u32;

    macro_rules! add_chart {
        ($group:expr, $y:expr) => {
            if current <= MAX_CHART_ROWS as u32 {
                idx_info.insert($group, current);
                layout = set_axes(current, layout, x_axis.clone(), $y);
                current += 1;
            }
        };
    }

    if has_memory {
        add_chart!(
            ChartGroup::Memory,
            Axis::new()
                .range(AxisRange::new(0, loaded_results.memory_total.ceil() as usize))
                .title(Title::with_text("Memory Usage [MB]"))
        );
    }
    if has_cpu {
        add_chart!(ChartGroup::Cpu, Axis::new().range(vec![-1, 100]).title(Title::with_text("CPU Usage [%]")));
    }
    if has_swap {
        add_chart!(
            ChartGroup::Swap,
            Axis::new()
                .range(AxisRange::new(0, loaded_results.swap_total.ceil() as usize))
                .title(Title::with_text("Swap Usage [MB]"))
        );
    }
    if has_network {
        add_chart!(ChartGroup::Network, Axis::new().title(Title::with_text("Network [MB/s]")));
    }
    if has_network_total {
        add_chart!(ChartGroup::NetworkTotal, Axis::new().title(Title::with_text("Network Total [MB]")));
    }
    if has_gpu_util {
        add_chart!(
            ChartGroup::GpuUtil,
            Axis::new().range(vec![-1, 100]).title(Title::with_text("GPU Utilization [%]"))
        );
    }
    if has_gpu_vram {
        // Largest VRAM among GPUs, used as the chart ceiling - individual GPUs with less
        // VRAM just won't reach the top of the shared axis.
        let mut axis = Axis::new().title(Title::with_text("GPU VRAM [MB]"));
        if let Some(max_mb) = loaded_results.gpu_vram_mb.iter().copied().max().filter(|&m| m > 0) {
            axis = axis.range(AxisRange::new(0, max_mb as usize));
        }
        add_chart!(ChartGroup::GpuVram, axis);
    }
    if has_gpu_temp {
        add_chart!(ChartGroup::GpuTemp, Axis::new().title(Title::with_text("GPU Temperature [°C]")));
    }
    if has_disk_used {
        add_chart!(ChartGroup::DiskUsed, Axis::new().title(Title::with_text("Disk Space Used [GB]")));
    }
    if has_disk_available {
        add_chart!(
            ChartGroup::DiskAvailable,
            Axis::new().title(Title::with_text("Disk Space Available [GB]"))
        );
    }
    if has_disk_busy {
        add_chart!(
            ChartGroup::DiskBusy,
            Axis::new().range(vec![-1, 100]).title(Title::with_text("Disk Busy [%]"))
        );
    }
    if has_disk_io {
        add_chart!(ChartGroup::DiskIo, Axis::new().title(Title::with_text("Disk I/O [MB/s]")));
    }

    let _ = current;
    (layout, idx_info)
}

// ── per-metric trace builders ─────────────────────────────────────────────────

fn create_memory_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results.collected_data.iter().filter(|(dt, _)| dt.is_memory()).collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_swap_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results.collected_data.iter().filter(|(dt, _)| dt.is_swap()).collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_cpu_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results
        .collected_data
        .iter()
        .filter(|(dt, _)| dt.is_cpu() && *dt != &DataType::CPU_USAGE_PER_CORE)
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }

    if let Some(per_core) = loaded_results.collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        for (idx, core_data) in per_core.iter().enumerate() {
            let values: Vec<String> = core_data.split(';').map(ToString::to_string).collect();
            let trace = Scatter::new(dates.to_owned(), values)
                .name(format!("Core {idx}"))
                .y_axis(format!("y{i}"))
                .x_axis(format!("x{i}"));
            plot.add_trace(trace);
        }
    }
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

/// One trace per column accepted by `belongs_to_chart`, sorted by label.
fn create_traces(
    plot: &mut Plot,
    dates: &[DateTime<Utc>],
    loaded_results: &CollectedItemModels,
    i: u32,
    belongs_to_chart: fn(&DataType) -> bool,
    settings: &ConvertSettings,
) {
    let mut entries: Vec<_> = loaded_results.collected_data.iter().filter(|(dt, _)| belongs_to_chart(dt)).collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let mut trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        if let Some(color) = device_color(data_type, settings.white_plot_mode) {
            trace = trace.line(plotly::common::Line::new().color(color));
        }
        plot.add_trace(trace);
    }
}

fn set_axes(idx: u32, layout: Layout, x: Axis, y: Axis) -> Layout {
    match idx {
        1 => layout.x_axis(x).y_axis(y),
        2 => layout.x_axis2(x).y_axis2(y),
        3 => layout.x_axis3(x).y_axis3(y),
        4 => layout.x_axis4(x).y_axis4(y),
        5 => layout.x_axis5(x).y_axis5(y),
        6 => layout.x_axis6(x).y_axis6(y),
        7 => layout.x_axis7(x).y_axis7(y),
        8 => layout.x_axis8(x).y_axis8(y),
        9 => layout.x_axis9(x).y_axis9(y),
        10 => layout.x_axis10(x).y_axis10(y),
        11 => layout.x_axis11(x).y_axis11(y),
        12 => layout.x_axis12(x).y_axis12(y),
        _ => layout,
    }
}
