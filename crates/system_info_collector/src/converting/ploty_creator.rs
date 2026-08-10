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
use system_info_collector_core::model::{CollectedItemModels, TopProcessData};
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

        // Generate a separate HTML for each top-N process dataset.
        for (top, kind, file_suffix) in [
            (&loaded_results.top_cpu_processes, "CPU", "cpu"),
            (&loaded_results.top_ram_processes, "RAM", "ram"),
        ] {
            let Some(top) = top else { continue };
            let path = top_process_plot_path(&settings.plot_path, file_suffix);
            let Some(plot) = build_top_process_plot(top, loaded_results.start_time, timezone_ms, kind, settings) else {
                info!("No process data — skipping {path}");
                continue;
            };
            info!("Creating top-{kind} process plot: {path}");
            let html = minify_html(&apply_style(plot.to_html(), settings));
            fs::write(&path, html.as_bytes()).context(format!("Failed to write top-process plot: {path}"))?;
        }

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
        disk_names: model.disk_names.clone(),
        top_cpu_processes: None,
        top_ram_processes: None,
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

    let mut subset = slice_model_by_indices(model, &indices);
    subset.top_cpu_processes = model.top_cpu_processes.as_ref().map(|t| subset_top(t, keep, model.start_time, stride));
    subset.top_ram_processes = model.top_ram_processes.as_ref().map(|t| subset_top(t, keep, model.start_time, stride));
    subset
}

fn subset_top(top: &TopProcessData, keep: &dyn Fn(f64) -> bool, start_time: f64, stride: usize) -> TopProcessData {
    let indices: Vec<usize> = top
        .timestamps
        .iter()
        .enumerate()
        .filter(|(_, ts)| keep(**ts + start_time))
        .map(|(i, _)| i)
        .step_by(stride)
        .collect();

    TopProcessData {
        n: top.n,
        start_time: top.start_time,
        timestamps: indices.iter().filter_map(|&i| top.timestamps.get(i).copied()).collect(),
        ranks: top
            .ranks
            .iter()
            .map(|rank| indices.iter().map(|&i| rank.get(i).cloned().flatten()).collect())
            .collect(),
    }
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

/// Derive the output path for a top-process plot from the main plot path.
/// `plot.html` → `plot_top_cpu.html` / `plot_top_ram.html`
fn top_process_plot_path(plot_path: &str, kind: &str) -> String {
    if let Some(base) = plot_path.strip_suffix(".html") {
        format!("{base}_top_{kind}.html")
    } else {
        format!("{plot_path}_top_{kind}.html")
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

/// Fine-grained chart groups.  GPU is split into three separate charts so that
/// utilisation (%), VRAM (MB), and temperature (°C) each get their own axis.
/// Top-N process data is rendered as separate standalone HTML files.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum ChartGroup {
    Memory,
    Cpu,
    Swap,
    Network,
    GpuUtil,
    GpuVram,
    GpuTemp,
    Disk,
}

pub fn save_plot_into_file(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<(), Error> {
    info!("Trying to create html file...");

    let html = build_main_document(loaded_results, settings, timezone_ms, "")?;
    fs::write(&settings.plot_path, html.as_bytes()).context(format!("Failed to write html plot file - {}", settings.plot_path))?;

    Ok(())
}

/// Build a single self-contained report: the main multi-chart plot plus, when
/// present, the top-N process charts embedded in the same document.
pub fn build_report_html(loaded_results: &CollectedItemModels, settings: &ConvertSettings, timezone_ms: i64) -> Result<String, Error> {
    let mut extra_body = String::new();

    for (top, kind, div_id) in [
        (&loaded_results.top_cpu_processes, "CPU", "top-cpu-plot"),
        (&loaded_results.top_ram_processes, "RAM", "top-ram-plot"),
    ] {
        let Some(top) = top else { continue };
        let Some(plot) = build_top_process_plot(top, loaded_results.start_time, timezone_ms, kind, settings) else {
            continue;
        };
        extra_body.push_str(&format!(
            "<h2 style=\"text-align: center;\">Top Processes - {kind}</h2>{}",
            plot.to_inline_html(Some(div_id))
        ));
    }

    build_main_document(loaded_results, settings, timezone_ms, &extra_body)
}

fn build_main_document(
    loaded_results: &CollectedItemModels,
    settings: &ConvertSettings,
    timezone_ms: i64,
    extra_body: &str,
) -> Result<String, Error> {
    let plot = build_main_plot(loaded_results, settings, timezone_ms)?;

    let html = apply_style(plot.to_html(), settings);
    let body_suffix = [notes_html(loaded_results), extra_body.to_string(), per_chart_legends_script().to_string()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
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
        create_network_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::GpuUtil) {
        create_gpu_util_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::GpuVram) {
        create_gpu_vram_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::GpuTemp) {
        create_gpu_temp_plot(&mut plot, &dates, loaded_results, i);
    }
    if let Some(&i) = layout_info.get(&ChartGroup::Disk) {
        create_disk_plot(&mut plot, &dates, loaded_results, i);
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

    // GPU split into three independent sub-charts based on what data is present.
    let has_gpu_util = loaded_results
        .collected_data
        .keys()
        .any(|dt| matches!(dt, DataType::GPU_UTILIZATION | DataType::GPU_N_UTIL(_)));
    let has_gpu_vram = loaded_results
        .collected_data
        .keys()
        .any(|dt| matches!(dt, DataType::GPU_MEMORY_USED | DataType::GPU_N_VRAM_MB(_)));
    let has_gpu_temp = loaded_results
        .collected_data
        .keys()
        .any(|dt| matches!(dt, DataType::GPU_TEMPERATURE | DataType::GPU_N_TEMP_C(_)));
    let has_disk = groups.contains(&GeneralInfoGroup::DISK);

    let rows = has_memory as usize
        + has_cpu as usize
        + has_swap as usize
        + has_network as usize
        + has_gpu_util as usize
        + has_gpu_vram as usize
        + has_gpu_temp as usize
        + has_disk as usize;

    // plotly 0.14 supports up to 8 named axes; cap the grid accordingly.
    let capped_rows = rows.min(8);
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
            if current <= 8 {
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
    if has_gpu_util {
        add_chart!(
            ChartGroup::GpuUtil,
            Axis::new().range(vec![-1, 100]).title(Title::with_text("GPU Utilization [%]"))
        );
    }
    if has_gpu_vram {
        add_chart!(ChartGroup::GpuVram, Axis::new().title(Title::with_text("GPU VRAM [MB]")));
    }
    if has_gpu_temp {
        add_chart!(ChartGroup::GpuTemp, Axis::new().title(Title::with_text("GPU Temperature [°C]")));
    }
    if has_disk {
        add_chart!(ChartGroup::Disk, Axis::new().title(Title::with_text("Disk Space [GB]")));
    }

    let _ = current;
    (layout, idx_info)
}

// ── top-N process standalone plot ────────────────────────────────────────────

/// For each unique process name that ever appeared in the top-N ranking,
/// build a value vector aligned to `top.timestamps`.  Slots where the process
/// was not in the ranking hold `None` (rendered as gaps in the chart).
/// Returns entries sorted by activity (most ticks in top-N first), then name.
fn build_process_traces(top: &TopProcessData) -> Vec<(String, Vec<Option<f64>>)> {
    let n_ts = top.timestamps.len();
    let mut map: HashMap<String, Vec<Option<f64>>> = HashMap::new();

    // ranks[rank_idx][ts_idx]
    for ts_idx in 0..n_ts {
        for rank_vec in &top.ranks {
            if let Some(Some((name, val))) = rank_vec.get(ts_idx) {
                map.entry(name.clone()).or_insert_with(|| vec![None; n_ts])[ts_idx] = Some(*val);
            }
        }
    }

    // Sort: most active (most non-None ticks) first, then alphabetically.
    let mut traces: Vec<(String, Vec<Option<f64>>)> = map.into_iter().collect();
    traces.sort_by(|(na, va), (nb, vb)| {
        let count_a = va.iter().filter(|v| v.is_some()).count();
        let count_b = vb.iter().filter(|v| v.is_some()).count();
        count_b.cmp(&count_a).then_with(|| na.cmp(nb))
    });

    traces
}

/// Convert a per-timestamp optional-value trace into (xs, ys) with -1 sentinels.
///
/// For each contiguous block where the process is present:
///   - emits a -1.0 point at the timestamp just BEFORE the block (if it exists)
///   - emits all actual values in the block
///   - emits a -1.0 point at the timestamp just AFTER the block (if it exists)
///
/// Between separate blocks a `None` y-value is inserted so plotly draws a gap
/// instead of a diagonal line spanning the absence.
fn build_sentinel_trace(values: &[Option<f64>], dates: &[Option<DateTime<Utc>>]) -> (Vec<DateTime<Utc>>, Vec<Option<f64>>) {
    let n = values.len();
    let mut xs: Vec<DateTime<Utc>> = Vec::new();
    let mut ys: Vec<Option<f64>> = Vec::new();

    let mut i = 0;
    while i < n {
        if values[i].is_none() {
            i += 1;
            continue;
        }

        // Found the start of a contiguous present block.
        let block_start = i;
        while i < n && values[i].is_some() {
            i += 1;
        }
        let block_end = i - 1; // inclusive

        // Gap separator between consecutive blocks so plotly doesn't draw a line.
        if !xs.is_empty()
            && let Some(Some(dt)) = dates.get(block_start)
        {
            xs.push(*dt);
            ys.push(None);
        }

        // -1 sentinel one tick before the block.
        if block_start > 0
            && let Some(Some(dt)) = dates.get(block_start - 1)
        {
            xs.push(*dt);
            ys.push(Some(-1.0));
        }

        // Actual block values.
        for j in block_start..=block_end {
            if let Some(Some(dt)) = dates.get(j) {
                xs.push(*dt);
                ys.push(values[j]); // Option<f64>
            }
        }

        // -1 sentinel one tick after the block.
        if block_end + 1 < n
            && let Some(Some(dt)) = dates.get(block_end + 1)
        {
            xs.push(*dt);
            ys.push(Some(-1.0));
        }
    }

    (xs, ys)
}

/// Build a standalone process plot.  `kind` is "CPU" or "RAM" and drives the
/// Y-axis label.  Returns `None` when there is no process data to render.
fn build_top_process_plot(top: &TopProcessData, start_time: f64, timezone_ms: i64, kind: &str, settings: &ConvertSettings) -> Option<Plot> {
    let traces = build_process_traces(top);
    if traces.is_empty() {
        return None;
    }

    let y_title = if kind == "CPU" { "CPU Usage [%]" } else { "RAM [MB]" };
    let n_traces = traces.len();

    // Build timestamp X-axis values once (aligned to absolute time).
    let dates: Vec<Option<DateTime<Utc>>> = top
        .timestamps
        .iter()
        .map(|&ts| DateTime::from_timestamp_millis(((ts + start_time) * 1000.0) as i64 + timezone_ms))
        .collect();

    let mut plot = Plot::new();

    // Single-chart layout; height grows slightly with many traces for the legend.
    let height = settings.plot_height.max(600 + (n_traces as u32).saturating_sub(10) * 20);
    let y_axis = if kind == "CPU" {
        Axis::new().range(vec![-2.0_f64, 100.0]).title(Title::with_text(y_title))
    } else {
        Axis::new().title(Title::with_text(y_title))
    };
    let mut layout = Layout::new()
        .width(settings.plot_width as usize)
        .height(height as usize)
        .x_axis(Axis::new().title(Title::with_text("Time")))
        .y_axis(y_axis);

    if !settings.white_plot_mode {
        layout = layout.template(&*PLOTLY_DARK);
    }
    plot.set_layout(layout);

    // One trace per unique process; color spread evenly around the hue wheel.
    for (trace_idx, (name, values)) in traces.iter().enumerate() {
        let (xs, ys) = build_sentinel_trace(values, &dates);

        if xs.is_empty() {
            continue;
        }

        let hue = (trace_idx * 360) / n_traces.max(1);
        let color = format!("hsl({hue}, 75%, 60%)");

        let trace = Scatter::new(xs, ys)
            .name(name.clone())
            .web_gl_mode(false)
            .connect_gaps(false)
            .line(plotly::common::Line::new().color(color));
        plot.add_trace(trace);
    }

    Some(plot)
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

fn create_network_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results.collected_data.iter().filter(|(dt, _)| dt.is_network()).collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_gpu_util_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results
        .collected_data
        .iter()
        .filter(|(dt, _)| matches!(dt, DataType::GPU_UTILIZATION | DataType::GPU_N_UTIL(_)))
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_gpu_vram_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results
        .collected_data
        .iter()
        .filter(|(dt, _)| matches!(dt, DataType::GPU_MEMORY_USED | DataType::GPU_N_VRAM_MB(_)))
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_gpu_temp_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results
        .collected_data
        .iter()
        .filter(|(dt, _)| matches!(dt, DataType::GPU_TEMPERATURE | DataType::GPU_N_TEMP_C(_)))
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_disk_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    let mut entries: Vec<_> = loaded_results.collected_data.iter().filter(|(dt, _)| dt.is_disk()).collect();
    entries.sort_by(|(a, _), (b, _)| a.pretty_print().cmp(&b.pretty_print()));
    for (data_type, data) in entries {
        let trace = Scatter::new(dates.to_owned(), data.clone())
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
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
        _ => layout,
    }
}
