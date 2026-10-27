//! Real programs measured, summarized, exported and charted.
#![cfg(target_os = "linux")]

use std::time::Duration;

use system_info_probe::measure::{MeasureOptions, RunLength};
use system_info_report::benchmark::{Benchmark, FnMetric, LogCapture, Variant};
use system_info_report::series::SeriesOptions;
use system_info_report::svg::{SvgOptions, charts, document};
use system_info_report::{Summary, TableFormat};

fn busy(workers: usize) -> Variant {
    let loops = vec!["timeout 1.5 sh -c 'while :; do :; done' &"; workers].join(" ");
    Variant::new(format!("busy{workers}"), "sh")
        .arg("-c")
        .arg(format!("{loops} echo passes=42 seconds=1.5; wait"))
        .dimension("workers", workers.to_string())
}

fn options() -> MeasureOptions {
    MeasureOptions {
        warmup: Duration::from_millis(200),
        run: RunLength::Fixed(Duration::from_millis(1000)),
        interval: Duration::from_millis(100),
        ..MeasureOptions::default()
    }
}

fn passes(log: &str) -> Option<f64> {
    log.lines()
        .rev()
        .find_map(|line| line.split_whitespace().find_map(|w| w.strip_prefix("passes=")?.parse().ok()))
}

#[test]
fn benchmark_compares_variants_and_reads_metrics_from_logs() {
    let mut benchmark = Benchmark::new(vec![busy(1), busy(2)], 2, options()).metric(FnMetric::new("passes", 0, |run| passes(run.log)));
    benchmark.logs = LogCapture::Capture;
    let mut seen = 0;
    let result = benchmark.run(|_| seen += 1);
    assert_eq!(seen, 4);
    assert!(result.problems().is_empty(), "{:?}", result.problems());

    let cpu = |index: usize| {
        let runs = &result.variants[index].runs;
        runs.iter().filter_map(|r| r.cpu_percent).sum::<f64>() / runs.len() as f64
    };
    assert!((80.0..=110.0).contains(&cpu(0)), "one busy loop is one core: {}", cpu(0));
    assert!((170.0..=210.0).contains(&cpu(1)), "two busy loops are two cores: {}", cpu(1));
    assert!(result.variants.iter().flat_map(|v| &v.runs).all(|r| r.metrics == vec![Some(42.0)]));

    let table = result.table(true);
    let csv = table.render(TableFormat::Csv);
    let header = csv.lines().next().expect("header");
    assert!(
        header.starts_with("variant,workers,cpu [%],cpu min [%],cpu max [%],cpu time [s]"),
        "{header}"
    );
    assert!(header.ends_with("passes,passes min,passes max,runs"), "{header}");
    assert!(csv.lines().nth(2).expect("second variant").starts_with("busy2,2,"));
    let terminal = result.table(false).render(TableFormat::Terminal);
    assert!(terminal.contains("busy1") && terminal.contains("42"), "{terminal}");
}

#[test]
fn recordings_export_and_chart() {
    let measurement = system_info_probe::blocking::measure_command(
        busy(1).command().stdout(std::process::Stdio::null()),
        &MeasureOptions {
            warmup: Duration::ZERO,
            ..options()
        },
    )
    .expect("measures");
    let recording = measurement.recording;
    let summary = Summary::of(&recording);
    assert!(summary.groups_table().render(TableFormat::Markdown).contains("| target |"));
    assert!(summary.processes_table(5).rows.len() >= 2);
    assert!(summary.system_table().is_some());

    let mut wide = Vec::new();
    system_info_report::export::write_wide_csv(&recording, SeriesOptions { processes: true }, &mut wide).expect("csv");
    let wide = String::from_utf8(wide).expect("utf8");
    let columns = wide.lines().next().expect("header").split(',').count();
    assert!(wide.lines().all(|line| line.split(',').count() == columns));
    assert_eq!(wide.lines().count(), recording.ticks.len() + 1);
    assert!(!wide.contains("-1"), "no sentinels");
    assert!(wide.lines().next().expect("header").contains("target.pct"));

    let mut long = Vec::new();
    system_info_report::export::write_long_csv(&recording, SeriesOptions::default(), &mut long).expect("csv");
    assert!(
        String::from_utf8(long)
            .expect("utf8")
            .lines()
            .skip(1)
            .all(|line| line.split(',').count() == 3)
    );

    #[cfg(feature = "json")]
    {
        let mut json = Vec::new();
        system_info_report::export::write_json(&recording, &mut json).expect("json");
        let back = system_info_report::export::read_json(json.as_slice()).expect("parses back");
        assert_eq!(back, recording, "JSON round trip is exact, floats included");
    }

    let svg = document(&charts(&recording, SeriesOptions::default()), &SvgOptions::default());
    assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
    assert!(svg.contains("CPU by group"));
    if let Some(dir) = std::env::var_os("REPORT_SAMPLE_DIR") {
        std::fs::write(std::path::Path::new(&dir).join("sample.svg"), &svg).expect("write sample");
        std::fs::write(std::path::Path::new(&dir).join("sample.csv"), &wide).expect("write sample");
    }
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "multi_thread")]
async fn async_benchmark_matches_the_blocking_one() {
    let mut benchmark = Benchmark::new(vec![busy(1)], 1, options()).metric(FnMetric::new("passes", 0, |run| passes(run.log)));
    benchmark.logs = LogCapture::Capture;
    let result = benchmark.run_async(|_| {}).await;
    assert!(result.problems().is_empty(), "{:?}", result.problems());
    let run = &result.variants[0].runs[0];
    assert!(run.cpu_percent.is_some_and(|cpu| (80.0..=110.0).contains(&cpu)), "{:?}", run.cpu_percent);
    assert_eq!(run.metrics, vec![Some(42.0)]);
}
