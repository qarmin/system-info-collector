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
use crate::model::{CollectedItemModels, Settings};

pub fn load_results_and_save_plot(settings: &Settings) -> Result<(), Error> {
    let time_start = SystemTime::now();
    let loaded_results = load_csv_results(settings)?;

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
    let dates = loaded_results.collected_data[0]
        .iter()
        .map(|&str_time| {
            if let Ok(time) = str_time.parse::<f64>() {
                NaiveDateTime::from_timestamp_millis((time * 1000.0) as i64)
            } else {
                None
            }
        })
        .collect::<Option<Vec<NaiveDateTime>>>()
        .context("Failed to parse unix timestamp")?;

    let mut plot = Plot::new();

    plot.set_layout(create_plot_layout(loaded_results.memory_total[0]));

    create_memory_plot(&mut plot, &dates, loaded_results.clone());
    create_cpu_plot(&mut plot, &dates, loaded_results.clone());

    // Only replace when using dark theme
    let html = plot
        .to_html()
        .replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>");
    fs::write(&settings.plot_path, html).context(format!("Failed to write html plot file - {}", settings.plot_path))?;
    Ok(())
}

pub fn create_plot_layout(memory_total: u64) -> Layout {
    Layout::new()
        .width(1700)
        .height(800)
        .template(&*PLOTLY_DARK)
        .grid(LayoutGrid::new().rows(2).columns(1).pattern(GridPattern::Independent))
        .y_axis2(Axis::new().range(vec![0, 100]).title(Title::new("CPU Usage[%]")))
        .x_axis2(Axis::new().title(Title::new("Time")))
        .y_axis(Axis::new().range(vec![0, memory_total]).title(Title::new("Memory Usage[MB]")))
        .x_axis(Axis::new().title(Title::new("Time")))
}

pub fn create_memory_plot(plot: &mut Plot, dates: &[NaiveDateTime], loaded_results: CollectedItemModels) {
    let trace = Scatter::new(dates.to_owned(), loaded_results.memory_used)
        .name("Memory Used")
        .y_axis("y1")
        .x_axis("x1");
    let trace2 = Scatter::new(dates.to_owned(), loaded_results.memory_free)
        .name("Memory Free")
        .y_axis("y1")
        .x_axis("x1");
    let trace3 = Scatter::new(dates.to_owned(), loaded_results.memory_available)
        .name("Memory Available")
        .y_axis("y1")
        .x_axis("x1");
    plot.add_trace(trace);
    plot.add_trace(trace2);
    plot.add_trace(trace3);
}

pub fn create_cpu_plot(plot: &mut Plot, dates: &[NaiveDateTime], loaded_results: CollectedItemModels) {
    let trace = Scatter::new(dates.to_owned(), loaded_results.cpu_total)
        .name("Cpu Used")
        .y_axis("y2")
        .x_axis("x2");
    plot.add_trace(trace);

    for (idx, cpu_core_usage) in loaded_results.cpu_core_usage.into_iter().enumerate() {
        let trace = Scatter::new(dates.to_owned(), cpu_core_usage)
            .name(format!("Core {idx}"))
            .y_axis("y2")
            .x_axis("x2");
        plot.add_trace(trace);
    }
}
