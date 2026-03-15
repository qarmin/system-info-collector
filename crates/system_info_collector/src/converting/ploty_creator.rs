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
use system_info_collector_core::model::CollectedItemModels;
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

    if loaded_results.collected_groups.contains(&GeneralInfoGroup::MEMORY) {
        create_memory_plot(&mut plot, &dates, loaded_results, layout_info[&GeneralInfoGroup::MEMORY]);
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::CPU) {
        create_cpu_plot(&mut plot, &dates, loaded_results, layout_info[&GeneralInfoGroup::CPU]);
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::SWAP) {
        create_swap_plot(&mut plot, &dates, loaded_results, layout_info[&GeneralInfoGroup::SWAP]);
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::NETWORK) {
        create_network_plot(&mut plot, &dates, loaded_results, layout_info[&GeneralInfoGroup::NETWORK]);
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::GPU) {
        create_gpu_plot(&mut plot, &dates, loaded_results, layout_info[&GeneralInfoGroup::GPU]);
    }

    let mut html = plot.to_html();
    if !settings.white_plot_mode {
        html = html.replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>");
    }

    let notes = [
        format!("Cpu count: {}", loaded_results.cpu_core_count),
        format!("Check interval: {}s", loaded_results.check_interval),
        format!("Memory total: {}", humansize::format_size((loaded_results.memory_total * 1024.0 * 1024.0) as u64, humansize::BINARY)),
        format!("Swap total: {}", humansize::format_size((loaded_results.swap_total * 1024.0 * 1024.0) as u64, humansize::BINARY)),
    ];

    #[expect(clippy::format_collect)]
    let notes = notes.iter().map(|e| format!("<div style=\"text-align: center;\">{e}</div>")).collect::<String>();
    html = html.replace("</body>", &format!("{}\n</body>", &notes));

    let regex = Regex::new(r"\n[ ]+").expect("Regex is invalid");
    let html = regex.replace_all(&html, "");
    fs::write(&settings.plot_path, html.as_bytes()).context(format!("Failed to write html plot file - {}", settings.plot_path))?;

    Ok(())
}

pub fn create_plot_layout(loaded_results: &CollectedItemModels, settings: &ConvertSettings) -> (Layout, HashMap<GeneralInfoGroup, u32>) {
    let has_memory = loaded_results.collected_groups.contains(&GeneralInfoGroup::MEMORY);
    let has_cpu = loaded_results.collected_groups.contains(&GeneralInfoGroup::CPU);
    let has_swap = loaded_results.collected_groups.contains(&GeneralInfoGroup::SWAP);
    let has_network = loaded_results.collected_groups.contains(&GeneralInfoGroup::NETWORK);
    let has_gpu = loaded_results.collected_groups.contains(&GeneralInfoGroup::GPU);

    let rows = has_cpu as usize + has_memory as usize + has_swap as usize + has_network as usize + has_gpu as usize;

    let mut layout = Layout::new()
        .width(settings.plot_width as usize)
        .height(settings.plot_height as usize)
        .grid(LayoutGrid::new().rows(rows).columns(1).pattern(GridPattern::Independent));

    if !settings.white_plot_mode {
        layout = layout.template(&*PLOTLY_DARK);
    }

    let mut idx_info: HashMap<GeneralInfoGroup, u32> = HashMap::default();
    let x_axis = Axis::new().title(Title::with_text("Time"));
    let mut current = 1u32;

    if has_memory {
        idx_info.insert(GeneralInfoGroup::MEMORY, current);
        let y = Axis::new().range(AxisRange::new(0, loaded_results.memory_total.ceil() as usize)).title(Title::with_text("Memory Usage [MB]"));
        layout = set_axes(current, layout, x_axis.clone(), y);
        current += 1;
    }
    if has_cpu {
        idx_info.insert(GeneralInfoGroup::CPU, current);
        let y = Axis::new().range(vec![-1, 100]).title(Title::with_text("CPU Usage [%]"));
        layout = set_axes(current, layout, x_axis.clone(), y);
        current += 1;
    }
    if has_swap {
        idx_info.insert(GeneralInfoGroup::SWAP, current);
        let y = Axis::new().range(AxisRange::new(0, loaded_results.swap_total.ceil() as usize)).title(Title::with_text("Swap Usage [MB]"));
        layout = set_axes(current, layout, x_axis.clone(), y);
        current += 1;
    }
    if has_network {
        idx_info.insert(GeneralInfoGroup::NETWORK, current);
        let y = Axis::new().title(Title::with_text("Network [bytes/s]"));
        layout = set_axes(current, layout, x_axis.clone(), y);
        current += 1;
    }
    if has_gpu {
        idx_info.insert(GeneralInfoGroup::GPU, current);
        let y = Axis::new().range(vec![-1, 100]).title(Title::with_text("GPU [% / MB / °C]"));
        layout = set_axes(current, layout, x_axis, y);
    }

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

fn create_gpu_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_gpu() {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone()).name(data_type.pretty_print()).y_axis(format!("y{i}")).x_axis(format!("x{i}"));
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
        _ => panic!("too many plot groups"),
    }
}
