use std::collections::HashMap;
use std::fs;
use std::time::Instant;

use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use log::info;
use plotly::common::Title;
use plotly::layout::themes::PLOTLY_DARK;
use plotly::layout::{Axis, AxisRange, GridPattern, Layout, LayoutGrid};
use plotly::{Plot, Scatter};
use regex::Regex;
use system_info_collector_core::enums::{DataType, GeneralInfoGroup};
use system_info_collector_core::model::{CollectedItemModels, TopProcessData};
use system_info_collector_core::settings::ConvertSettings;
use time::UtcOffset;

use crate::converting::csv_file_loader::load_csv_results;

pub fn load_results_and_save_plot(settings: &ConvertSettings) -> Result<(), Error> {
    let time_start = Instant::now();
    let loaded_results = load_csv_results(settings)?;
    info!("Loading data took {:?}", time_start.elapsed());

    let time_start = Instant::now();
    save_plot_into_file(&loaded_results, settings)?;
    info!("Creating plot took {:?}", time_start.elapsed());

    if settings.open_plot_file {
        info!("Opening file {}", settings.plot_path);
        open::that(&settings.plot_path).context(format!("Failed to open {}", settings.plot_path))?;
    }

    Ok(())
}

/// Fine-grained chart groups.  GPU is split into three separate charts so that
/// utilisation (%), VRAM (MB), and temperature (°C) each get their own axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum ChartGroup {
    Memory,
    Cpu,
    Swap,
    Network,
    GpuUtil,
    GpuVram,
    GpuTemp,
    TopCpu,
    TopRam,
}

pub fn save_plot_into_file(loaded_results: &CollectedItemModels, settings: &ConvertSettings) -> Result<(), Error> {
    info!("Trying to create html file...");

    let timezone_ms = match UtcOffset::from_whole_seconds(chrono::offset::Local::now().offset().local_minus_utc()) {
        Ok(offset) => offset.whole_seconds() as i64 * 1000,
        Err(_) => 0,
    };

    let dates = loaded_results.collected_data[&DataType::SECONDS_SINCE_START]
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
    if let (Some(&i), Some(top)) = (layout_info.get(&ChartGroup::TopCpu), loaded_results.top_cpu_processes.as_ref()) {
        create_top_process_plot(&mut plot, top, loaded_results.start_time, timezone_ms, i);
    }
    if let (Some(&i), Some(top)) = (layout_info.get(&ChartGroup::TopRam), loaded_results.top_ram_processes.as_ref()) {
        create_top_process_plot(&mut plot, top, loaded_results.start_time, timezone_ms, i);
    }

    let mut html = plot.to_html();
    if !settings.white_plot_mode {
        html = html.replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>");
    }

    let mut notes_vec = vec![
        format!("Cpu count: {}", loaded_results.cpu_core_count),
        format!("Check interval: {}s", loaded_results.check_interval),
        format!("Memory total: {}", humansize::format_size((loaded_results.memory_total * 1024.0 * 1024.0) as u64, humansize::BINARY)),
        format!("Swap total: {}", humansize::format_size((loaded_results.swap_total * 1024.0 * 1024.0) as u64, humansize::BINARY)),
    ];
    for (idx, name) in loaded_results.gpu_names.iter().enumerate() {
        notes_vec.push(format!("GPU {}: {name}", idx));
    }

    #[expect(clippy::format_collect)]
    let notes = notes_vec.iter().map(|e| format!("<div style=\"text-align: center;\">{e}</div>")).collect::<String>();
    html = html.replace("</body>", &format!("{}\n</body>", &notes));

    let regex = Regex::new(r"\n[ ]+").expect("Regex is invalid");
    let html = regex.replace_all(&html, "");
    fs::write(&settings.plot_path, html.as_bytes()).context(format!("Failed to write html plot file - {}", settings.plot_path))?;

    Ok(())
}

fn create_plot_layout(loaded_results: &CollectedItemModels, settings: &ConvertSettings) -> (Layout, HashMap<ChartGroup, u32>) {
    let groups = &loaded_results.collected_groups;
    let has_memory = groups.contains(&GeneralInfoGroup::MEMORY);
    let has_cpu = groups.contains(&GeneralInfoGroup::CPU);
    let has_swap = groups.contains(&GeneralInfoGroup::SWAP);
    let has_network = groups.contains(&GeneralInfoGroup::NETWORK);

    // GPU split into three independent sub-charts based on what data is present.
    let has_gpu_util = loaded_results.collected_data.keys().any(|dt| matches!(dt, DataType::GPU_UTILIZATION | DataType::GPU_N_UTIL(_)));
    let has_gpu_vram = loaded_results.collected_data.keys().any(|dt| matches!(dt, DataType::GPU_MEMORY_USED | DataType::GPU_N_VRAM_MB(_)));
    let has_gpu_temp = loaded_results.collected_data.keys().any(|dt| matches!(dt, DataType::GPU_TEMPERATURE | DataType::GPU_N_TEMP_C(_)));

    let has_top_cpu = loaded_results.top_cpu_processes.is_some();
    let has_top_ram = loaded_results.top_ram_processes.is_some();

    let rows = has_memory as usize
        + has_cpu as usize
        + has_swap as usize
        + has_network as usize
        + has_gpu_util as usize
        + has_gpu_vram as usize
        + has_gpu_temp as usize
        + has_top_cpu as usize
        + has_top_ram as usize;

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

    // Helper closure: register a chart group and advance the counter.
    // Stops silently once we reach the axis limit (8).
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
            Axis::new().range(AxisRange::new(0, loaded_results.memory_total.ceil() as usize)).title(Title::with_text("Memory Usage [MB]"))
        );
    }
    if has_cpu {
        add_chart!(ChartGroup::Cpu, Axis::new().range(vec![-1, 100]).title(Title::with_text("CPU Usage [%]")));
    }
    if has_swap {
        add_chart!(
            ChartGroup::Swap,
            Axis::new().range(AxisRange::new(0, loaded_results.swap_total.ceil() as usize)).title(Title::with_text("Swap Usage [MB]"))
        );
    }
    if has_network {
        add_chart!(ChartGroup::Network, Axis::new().title(Title::with_text("Network [bytes/s]")));
    }
    if has_gpu_util {
        add_chart!(ChartGroup::GpuUtil, Axis::new().range(vec![-1, 100]).title(Title::with_text("GPU Utilization [%]")));
    }
    if has_gpu_vram {
        add_chart!(ChartGroup::GpuVram, Axis::new().title(Title::with_text("GPU VRAM [MB]")));
    }
    if has_gpu_temp {
        add_chart!(ChartGroup::GpuTemp, Axis::new().title(Title::with_text("GPU Temperature [°C]")));
    }
    if has_top_cpu {
        add_chart!(ChartGroup::TopCpu, Axis::new().title(Title::with_text("Top CPU Processes [%]")));
    }
    if has_top_ram {
        add_chart!(ChartGroup::TopRam, Axis::new().title(Title::with_text("Top RAM Processes [MB]")));
    }

    let _ = current; // last increment in the macro is intentionally unused
    (layout, idx_info)
}

fn create_memory_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_memory() {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_swap_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_swap() {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_cpu_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_cpu() || data_type == &DataType::CPU_USAGE_PER_CORE {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }

    if let Some(per_core) = loaded_results.collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        for (idx, core_data) in per_core.iter().enumerate() {
            let values: Vec<String> = core_data.split(';').map(ToString::to_string).collect();
            let trace = Scatter::new(dates.to_owned(), values).name(format!("Core {idx}")).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
            plot.add_trace(trace);
        }
    }
}

fn create_network_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_network() {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_gpu_util_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !matches!(data_type, DataType::GPU_UTILIZATION | DataType::GPU_N_UTIL(_)) {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_gpu_vram_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !matches!(data_type, DataType::GPU_MEMORY_USED | DataType::GPU_N_VRAM_MB(_)) {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

fn create_gpu_temp_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !matches!(data_type, DataType::GPU_TEMPERATURE | DataType::GPU_N_TEMP_C(_)) {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

/// Render a top-N process dataset.  Each rank slot becomes its own trace so
/// the viewer can toggle individual ranks on/off.
fn create_top_process_plot(plot: &mut Plot, top: &TopProcessData, start_time: f64, timezone_ms: i64, i: u32) {
    for (rank_idx, rank_vec) in top.ranks.iter().enumerate() {
        let mut xs: Vec<DateTime<Utc>> = Vec::new();
        let mut ys: Vec<String> = Vec::new();
        let mut label = format!("#{}", rank_idx + 1);

        for (row_idx, entry) in rank_vec.iter().enumerate() {
            if let Some((name, val)) = entry {
                if xs.is_empty() {
                    label = format!("#{} ({}...)", rank_idx + 1, name.chars().take(12).collect::<String>());
                }
                let ts = top.timestamps.get(row_idx).copied().unwrap_or(0.0);
                if let Some(dt) = DateTime::from_timestamp_millis(((ts + start_time) * 1000.0) as i64 + timezone_ms) {
                    xs.push(dt);
                    ys.push(format!("{val:.2}"));
                }
            }
        }

        if xs.is_empty() {
            continue;
        }

        let trace = Scatter::new(xs, ys).name(label).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
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
        _ => layout, // capped at 8 above; unreachable in normal operation
    }
}
