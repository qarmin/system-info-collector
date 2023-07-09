use std::fs;
use std::time::SystemTime;

use anyhow::{Context, Error};
use chrono::NaiveDateTime;
use log::info;
use plotly::common::Title;
use plotly::layout::themes::PLOTLY_DARK;
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid};
use plotly::{Plot, Scatter};

use crate::csv_file_loader::load_csv_results;
use crate::enums::{DataType, GeneralInfoGroup};
use crate::model::{CollectedItemModels, Settings};

pub fn load_results_and_save_plot(settings: &Settings) -> Result<(), Error> {
    let time_start = SystemTime::now();
    let loaded_results = load_csv_results(settings)?;
    info!("Loading data took {:?}", time_start.elapsed().unwrap());

    let time_start = SystemTime::now();
    save_plot_into_file(&loaded_results, settings)?;
    info!("Creating plot took {:?}", time_start.elapsed().unwrap());
    if settings.open_plot_file {
        info!("Opening file {}", settings.plot_path);
        open::that(&settings.plot_path).context(format!("Failed to open {}", settings.plot_path))?;
    }

    Ok(())
}

pub fn save_plot_into_file(loaded_results: &CollectedItemModels, settings: &Settings) -> Result<(), Error> {
    info!("Trying to create html file...");
    let dates = loaded_results.collected_data[&DataType::UNIX_TIMESTAMP]
        .iter()
        .map(|str_time| {
            if let Ok(time) = str_time.parse::<f64>() {
                NaiveDateTime::from_timestamp_millis((time * 1000.0) as i64)
            } else {
                None
            }
        })
        .collect::<Option<Vec<NaiveDateTime>>>()
        .context("Failed to parse unix timestamp")?;

    let mut plot = Plot::new();

    plot.set_layout(create_plot_layout(&loaded_results, settings)?);

    if loaded_results.collected_groups.contains(&GeneralInfoGroup::MEMORY) {
        create_memory_plot(&mut plot, &dates, loaded_results.clone());
    }
    if loaded_results.collected_groups.contains(&GeneralInfoGroup::CPU) {
        create_cpu_plot(&mut plot, &dates, loaded_results.clone());
    }

    // Only replace when using dark theme
    let mut html = plot.to_html();
    if !settings.white_plot_mode {
        html = html.replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>")
    }
    fs::write(&settings.plot_path, html).context(format!("Failed to write html plot file - {}", settings.plot_path))?;
    Ok(())
}

pub fn create_plot_layout(loaded_results: &CollectedItemModels, settings: &Settings) -> Result<Layout, Error> {
    let contains_memory_group = loaded_results.collected_groups.contains(&GeneralInfoGroup::MEMORY);
    let contains_cpu_group = loaded_results.collected_groups.contains(&GeneralInfoGroup::CPU);

    let mut layout = Layout::new()
        .width(settings.plot_width as usize)
        .height(settings.plot_height as usize)
        .grid(
            LayoutGrid::new()
                .rows(contains_cpu_group as usize + contains_cpu_group as usize)
                .columns(1)
                .pattern(GridPattern::Independent),
        );

    if !settings.white_plot_mode {
        layout = layout.template(&*PLOTLY_DARK);
    }
    if contains_memory_group {
        layout = layout
            .y_axis(
                Axis::new()
                    .range(vec![0, loaded_results.memory_total])
                    .title(Title::new("Memory Usage[MB]")),
            )
            .x_axis(Axis::new().title(Title::new("Time")));
    }
    if contains_cpu_group {
        layout = layout
            .y_axis2(Axis::new().range(vec![0, 100]).title(Title::new("CPU Usage[%]")))
            .x_axis2(Axis::new().title(Title::new("Time")));
    }

    Ok(layout)
}

pub fn create_memory_plot(plot: &mut Plot, dates: &[NaiveDateTime], loaded_results: CollectedItemModels) {
    for (data_type, data) in &loaded_results.collected_data {
        if !data_type.is_memory() {
            continue;
        }
        let trace = Scatter::new(dates.to_owned(), data.to_owned())
            .name(data_type.pretty_print())
            .y_axis("y1")
            .x_axis("x1");
        plot.add_trace(trace);
    }
}

pub fn create_cpu_plot(plot: &mut Plot, dates: &[NaiveDateTime], loaded_results: CollectedItemModels) {
    if let Some(data) = loaded_results.collected_data.get(&DataType::CPU_USAGE_TOTAL) {
        let trace = Scatter::new(dates.to_owned(), data.to_owned())
            .name(DataType::CPU_USAGE_TOTAL.pretty_print())
            .y_axis("y2")
            .x_axis("x2");
        plot.add_trace(trace);
    }

    // TODO not implemented yet, CPU per core uses different way of collecting data
    if let Some(data) = loaded_results.collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        for (idx, _cpu_data) in data.iter().enumerate() {
            let trace = Scatter::new(dates.to_owned(), data.to_owned())
                .name(format!("Core {idx}"))
                .y_axis("y2")
                .x_axis("x2");
            plot.add_trace(trace);
        }
    }
}
