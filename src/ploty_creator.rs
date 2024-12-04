use std::collections::HashMap;
use std::fs;
use std::time::Instant;

use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use log::info;
use plotly::common::Title;
use plotly::layout::themes::PLOTLY_DARK;
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid};
use plotly::{Plot, Scatter};
use regex::Regex;
use time::UtcOffset;

use crate::csv_file_loader::load_csv_results;
use crate::enums::{DataType, GeneralInfoGroup};
use crate::model::{CollectedItemModels, Settings};

pub fn load_results_and_save_plot(settings: &Settings) -> Result<(), Error> {
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

pub fn save_plot_into_file(loaded_results: &CollectedItemModels, settings: &Settings) -> Result<(), Error> {
    info!("Trying to create html file...");

    let timezone_millis_offset = match UtcOffset::from_whole_seconds(chrono::offset::Local::now().offset().local_minus_utc()) {
        Ok(offset) => offset.whole_seconds() as i64 * 1000,
        Err(_) => 0,
    };

    let dates = loaded_results.collected_data[&DataType::SECONDS_SINCE_START]
        .iter()
        .map(|str_time| {
            if let Ok(time) = str_time.parse::<f64>() {
                DateTime::from_timestamp_millis(((time + loaded_results.start_time) * 1000.0) as i64 + timezone_millis_offset)
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
        create_memory_plot(
            &mut plot,
            &dates,
            loaded_results,
            settings,
            *layout_info.get(&GeneralInfoGroup::MEMORY).unwrap(),
        );
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::CPU) {
        create_cpu_plot(
            &mut plot,
            &dates,
            loaded_results,
            settings,
            *layout_info.get(&GeneralInfoGroup::CPU).unwrap(),
        );
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::SWAP) {
        create_swap_plot(
            &mut plot,
            &dates,
            loaded_results,
            settings,
            *layout_info.get(&GeneralInfoGroup::SWAP).unwrap(),
        );
    }

    // Only replace when using dark theme
    let mut html = plot.to_html();
    if !settings.white_plot_mode {
        html = html.replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>");
    }

    let notes = [
        format!("Cpu count: {}", loaded_results.cpu_core_count),
        format!("Check interval: {}s", loaded_results.check_interval),
        // format!("Start time: {}", loaded_results.start_time),
        format!(
            "Memory total: {}",
            humansize::format_size((loaded_results.memory_total * 1024.0 * 1024.0) as u64, humansize::BINARY)
        ),
        format!(
            "Swap total: {}",
            humansize::format_size((loaded_results.swap_total * 1024.0 * 1024.0) as u64, humansize::BINARY)
        ),
    ];

    #[allow(clippy::format_collect)]
    let notes = notes
        .iter()
        .map(|e| format!("<div style=\"text-align: center;\">{e}</div>"))
        .collect::<String>();
    html = html.replace("</body>", &format!("{}\n</body>", &notes));

    // Simple minify
    let regex = Regex::new(r"\n[ ]+").expect("Regex is invalid");
    let html = regex.replace_all(&html, "");
    fs::write(&settings.plot_path, html.as_bytes()).context(format!("Failed to write html plot file - {}", settings.plot_path))?;

    Ok(())
}

pub fn create_plot_layout(loaded_results: &CollectedItemModels, settings: &Settings) -> (Layout, HashMap<GeneralInfoGroup, u32>) {
    let contains_memory_group = loaded_results.collected_groups.contains(&GeneralInfoGroup::MEMORY);
    let contains_cpu_group = loaded_results.collected_groups.contains(&GeneralInfoGroup::CPU);
    let contains_swap_group = loaded_results.collected_groups.contains(&GeneralInfoGroup::SWAP);

    let mut layout = Layout::new()
        .width(settings.plot_width as usize)
        .height(settings.plot_height as usize)
        .grid(
            LayoutGrid::new()
                .rows(contains_cpu_group as usize + contains_memory_group as usize + contains_swap_group as usize)
                .columns(1)
                .pattern(GridPattern::Independent),
        );

    if !settings.white_plot_mode {
        layout = layout.template(&*PLOTLY_DARK);
    }

    let mut layout_idx_info = HashMap::default();
    let x_axis = Axis::new().title(Title::with_text("Time"));

    let mut current_axis_idx = 1;
    if contains_memory_group {
        layout_idx_info.insert(GeneralInfoGroup::MEMORY, current_axis_idx);
        let y_axis = Axis::new()
            .range(vec![0, loaded_results.memory_total.ceil() as usize])
            .title(Title::with_text("Memory Usage[MB]"));

        layout = set_axes_into_layout(&mut current_axis_idx, layout, x_axis.clone(), y_axis);
    }
    if contains_cpu_group {
        layout_idx_info.insert(GeneralInfoGroup::CPU, current_axis_idx);
        let y_axis = Axis::new().range(vec![-1, 100]).title(Title::with_text("CPU Usage[%]"));

        layout = set_axes_into_layout(&mut current_axis_idx, layout, x_axis.clone(), y_axis);
    }
    if contains_swap_group {
        layout_idx_info.insert(GeneralInfoGroup::SWAP, current_axis_idx);
        let y_axis = Axis::new()
            .range(vec![0, loaded_results.swap_total.ceil() as usize])
            .title(Title::with_text("Swap Usage[MB]"));

        layout = set_axes_into_layout(&mut current_axis_idx, layout, x_axis.clone(), y_axis);
    }

    (layout, layout_idx_info)
}

pub fn create_memory_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, _settings: &Settings, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_memory() {
            continue;
        }

        let trace = Scatter::new(dates.to_owned(), data.clone())
            // .web_gl_mode(settings.use_web_gl)
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}
pub fn create_swap_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, _settings: &Settings, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_swap() {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone())
            // .web_gl_mode(settings.use_web_gl)
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }
}

pub fn create_cpu_plot(plot: &mut Plot, dates: &[DateTime<Utc>], loaded_results: &CollectedItemModels, _settings: &Settings, i: u32) {
    for (data_type, data) in &loaded_results.collected_data {
        // CPU_USAGE_PER_CORE is handled differently below
        if !data_type.is_cpu() || data_type == &DataType::CPU_USAGE_PER_CORE {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.clone())
            // .web_gl_mode(settings.use_web_gl)
            .name(data_type.pretty_print())
            .y_axis(format!("y{i}"))
            .x_axis(format!("x{i}"));
        plot.add_trace(trace);
    }

    // CPU per core uses different way of collecting data
    if let Some(multiple_cpu_data) = loaded_results.collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        for (idx, single_cpu_data) in multiple_cpu_data.iter().enumerate() {
            let single_cpu_data = single_cpu_data.split(';').map(ToString::to_string).collect::<Vec<String>>();
            let trace = Scatter::new(dates.to_owned(), single_cpu_data)
                // .web_gl_mode(settings.use_web_gl)
                .name(format!("Core {idx}"))
                .y_axis(format!("y{i}"))
                .x_axis(format!("x{i}"));
            plot.add_trace(trace);
        }
    }
}

fn set_axes_into_layout(idx: &mut u32, layout: Layout, x_axis: Axis, y_axis: Axis) -> Layout {
    let new_layout = match idx {
        1 => layout.x_axis(x_axis).y_axis(y_axis),
        2 => layout.x_axis2(x_axis).y_axis2(y_axis),
        3 => layout.x_axis3(x_axis).y_axis3(y_axis),
        _ => panic!(),
    };
    *idx += 1;
    new_layout
}
