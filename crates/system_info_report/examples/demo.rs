//! Records a short workload and writes every report format into the directory given as the
//! first argument: `cargo run -p system_info_report --example demo -- /tmp/report`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use system_info_probe::measure::{MeasureOptions, MeasureScope, RunLength};
use system_info_probe::recording::ProcessSeries;
use system_info_report::series::SeriesOptions;
use system_info_report::svg::{SvgOptions, Theme, charts, document};
use system_info_report::{Summary, TableFormat};

const WORKLOAD: &str = "\
    (timeout 3 sh -c 'while :; do :; done') & \
    sleep 1; (timeout 2 sh -c 'while :; do :; done') & \
    sleep 1; for i in $(seq 200); do sh -c 'i=0; while [ $i -lt 400 ]; do i=$((i+1)); done'; done; wait";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("usage: demo <output dir>")?);
    std::fs::create_dir_all(&dir)?;
    let options = MeasureOptions {
        run: RunLength::UntilExit {
            timeout: Some(Duration::from_secs(20)),
        },
        interval: Duration::from_millis(100),
        scope: MeasureScope::Tree,
        process_series: ProcessSeries::GroupMembers,
        ..MeasureOptions::default()
    };
    let mut command = Command::new("sh");
    command.arg("-c").arg(WORKLOAD).stdout(Stdio::null());
    let recording = system_info_probe::blocking::measure_command(&mut command, &options)?.recording;

    let summary = Summary::of(&recording);
    let mut markdown = summary.groups_table().render(TableFormat::Markdown);
    markdown.push('\n');
    markdown.push_str(&summary.processes_table(10).render(TableFormat::Markdown));
    std::fs::write(dir.join("summary.md"), markdown)?;

    let series = SeriesOptions { processes: true };
    let mut csv = Vec::new();
    system_info_report::export::write_wide_csv(&recording, series, &mut csv)?;
    std::fs::write(dir.join("recording.csv"), csv)?;

    let all = charts(&recording, series);
    for (name, theme) in [("light", Theme::Light), ("dark", Theme::Dark), ("auto", Theme::Auto)] {
        std::fs::write(
            dir.join(format!("charts-{name}.svg")),
            document(
                &all,
                &SvgOptions {
                    theme,
                    ..SvgOptions::default()
                },
            ),
        )?;
    }
    Ok(())
}
