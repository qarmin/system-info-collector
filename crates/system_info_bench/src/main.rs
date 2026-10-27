mod config;
mod duration;
mod metric;
mod services;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use system_info_probe::measure::{MeasureOptions, RunLength};
use system_info_probe::recording::Recording;
use system_info_report::benchmark::{Benchmark, BenchmarkResult, LogCapture, Variant};
use system_info_report::series::SeriesOptions;
use system_info_report::svg::{SvgOptions, charts, document};
use system_info_report::{Summary, TableFormat};

use crate::config::Scope;
use crate::metric::LogMetric;

/// Measures programs - CPU, memory, I/O of the whole process tree and numbers they print - and
/// compares them over repeated runs.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a program and measure it: `run --duration 10s -- ./app --flag`.
    Run(RunArgs),
    /// Measure an already running process and its children, without touching it.
    Attach(AttachArgs),
    /// Run the variants of a benchmark file, crossed with its matrix, and compare them.
    Matrix(MatrixArgs),
}

#[derive(Args)]
struct MeasureArgs {
    /// Time before measuring starts, e.g. `2s`.
    #[arg(long, default_value = "0s", value_parser = duration::parse)]
    warmup: Duration,
    /// How long to measure; the program is killed afterwards.
    #[arg(long, default_value = "10s", value_parser = duration::parse, conflicts_with = "until_exit")]
    duration: Duration,
    /// Measure until the program exits instead of for a fixed time.
    #[arg(long)]
    until_exit: bool,
    /// With --until-exit, kill the program after this long.
    #[arg(long, value_parser = duration::parse, requires = "until_exit")]
    timeout: Option<Duration>,
    /// Time between samples.
    #[arg(long, default_value = "250ms", value_parser = duration::parse)]
    interval: Duration,
    #[arg(long, value_enum, default_value_t = Scope::Tree)]
    scope: Scope,
}

impl MeasureArgs {
    fn options(&self) -> MeasureOptions {
        MeasureOptions {
            warmup: self.warmup,
            run: if self.until_exit {
                RunLength::UntilExit { timeout: self.timeout }
            } else {
                RunLength::Fixed(self.duration)
            },
            interval: self.interval,
            scope: self.scope.into(),
            ..MeasureOptions::default()
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Terminal,
    Markdown,
    Tsv,
    Csv,
}

impl From<Format> for TableFormat {
    fn from(format: Format) -> Self {
        match format {
            Format::Terminal => Self::Terminal,
            Format::Markdown => Self::Markdown,
            Format::Tsv => Self::Tsv,
            Format::Csv => Self::Csv,
        }
    }
}

#[derive(Args)]
struct OutputArgs {
    #[arg(long, value_enum, default_value_t = Format::Terminal)]
    format: Format,
    /// Write the tables here instead of to stdout.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Add min and max columns next to every median.
    #[arg(long)]
    spread: bool,
    /// Also list the processes that used the most CPU, this many of them.
    #[arg(long)]
    processes: Option<usize>,
    /// Charts as SVG: a file for a single program, a directory for a matrix.
    #[arg(long)]
    svg: Option<PathBuf>,
    /// Every sample as CSV: a file for a single program, a directory for a matrix.
    #[arg(long)]
    csv: Option<PathBuf>,
    /// Everything, including every sample of every run, as JSON.
    #[arg(long)]
    json: Option<PathBuf>,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    measure: MeasureArgs,
    #[command(flatten)]
    output: OutputArgs,
    /// Number of runs; the table shows medians.
    #[arg(long, default_value_t = 1)]
    repeat: usize,
    /// A number read from the program's output: `NAME=REGEX` with one capture group, or
    /// `NAME:EXTRACT=REGEX` (extract: last, first, min, max, mean, sum, count, rate).
    #[arg(long = "metric")]
    metrics: Vec<String>,
    /// Extra environment variable for the program, `KEY=VALUE`.
    #[arg(long = "env")]
    env: Vec<String>,
    /// Keep each run's output in this directory.
    #[arg(long)]
    logs: Option<PathBuf>,
    /// The program and its arguments.
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct AttachArgs {
    #[command(flatten)]
    measure: MeasureArgs,
    #[command(flatten)]
    output: OutputArgs,
    pid: u32,
}

#[derive(Args)]
struct MatrixArgs {
    #[command(flatten)]
    output: OutputArgs,
    /// Overrides `repeats` from the file.
    #[arg(long)]
    repeats: Option<usize>,
    /// Only variants with this name; may be given more than once.
    #[arg(long = "only")]
    only: Vec<String>,
    /// The benchmark file (TOML).
    file: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Run(args) => run(args),
        Command::Attach(args) => attach(&args),
        Command::Matrix(args) => matrix(&args),
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: RunArgs) -> Result<bool, String> {
    let (program, rest) = args.command.split_first().ok_or("no program given")?;
    let mut variant = Variant::new(
        Path::new(program).file_name().map_or(program.as_str(), |n| n.to_str().unwrap_or(program)),
        program,
    );
    variant.args = rest.to_vec();
    for pair in &args.env {
        let (key, value) = pair.split_once('=').ok_or_else(|| format!("--env {pair:?} is not KEY=VALUE"))?;
        variant = variant.env(key, value);
    }
    let mut benchmark = Benchmark::new(vec![variant], args.repeat, args.measure.options());
    for metric in &args.metrics {
        benchmark.metrics.push(Box::new(LogMetric::parse_argument(metric)?));
    }
    benchmark.logs = log_capture(args.logs, !benchmark.metrics.is_empty());
    let result = benchmark.run(progress);
    finish(&result, &args.output, false)
}

fn attach(args: &AttachArgs) -> Result<bool, String> {
    let measurement = system_info_probe::blocking::measure_pid(args.pid, &args.measure.options()).map_err(|e| e.to_string())?;
    let summary = Summary::of(&measurement.recording);
    let mut text = summary.groups_table().render(args.output.format.into());
    if let Some(limit) = Some(args.output.processes.unwrap_or(5)).filter(|l| *l > 0) {
        text.push('\n');
        text.push_str(&summary.processes_table(limit).render(args.output.format.into()));
    }
    emit(&text, args.output.output.as_deref())?;
    write_recording_files(&measurement.recording, &args.output, None)?;
    if let Some(path) = &args.output.json {
        write_json(path, &measurement.recording)?;
    }
    Ok(measurement
        .recording
        .ticks
        .iter()
        .any(|t| t.groups.first().is_some_and(|g| g.processes > 0)))
}

fn matrix(args: &MatrixArgs) -> Result<bool, String> {
    let file = config::load(&args.file)?;
    let mut variants = file.expand()?;
    if !args.only.is_empty() {
        variants.retain(|v| args.only.contains(&v.name));
        if variants.is_empty() {
            return Err(format!("no variant named {}", args.only.join(" or ")));
        }
    }
    let mut benchmark = Benchmark::new(variants, args.repeats.unwrap_or(file.repeats), file.measure_options()?);
    for metric in file.metrics()? {
        benchmark.metrics.push(Box::new(metric));
    }
    benchmark.logs = log_capture(file.logs.clone(), !benchmark.metrics.is_empty());
    if !file.services.is_empty() {
        benchmark.hooks = Some(Box::new(services::Services::new(file.services)));
    }
    let result = benchmark.run(progress);
    finish(&result, &args.output, true)
}

fn log_capture(dir: Option<PathBuf>, needed: bool) -> LogCapture {
    match (dir, needed) {
        (Some(dir), _) => LogCapture::KeepIn(dir),
        (None, true) => LogCapture::Capture,
        (None, false) => LogCapture::Discard,
    }
}

fn progress(progress: system_info_report::benchmark::Progress<'_>) {
    let dimensions: Vec<String> = progress.variant.dimensions.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let _ = writeln!(
        std::io::stderr(),
        "[{}/{}] {} {}run {}",
        progress.run + 1,
        progress.total_runs,
        progress.variant.name,
        dimensions.iter().map(|d| format!("{d} ")).collect::<String>(),
        progress.repeat + 1
    );
}

fn finish(result: &BenchmarkResult, output: &OutputArgs, per_variant_files: bool) -> Result<bool, String> {
    let format: TableFormat = output.format.into();
    let mut text = result.table(output.spread).render(format);
    let processes = output.processes.unwrap_or(if per_variant_files { 0 } else { 5 });
    if processes > 0
        && let Some(recording) = first_recording(result, 0)
    {
        text.push('\n');
        text.push_str(&Summary::of(recording).processes_table(processes).render(format));
    }
    emit(&text, output.output.as_deref())?;

    for (index, variant) in result.variants.iter().enumerate() {
        let Some(recording) = first_recording(result, index) else { continue };
        let name = per_variant_files.then(|| file_stem(&variant.variant));
        write_recording_files(recording, output, name.as_deref())?;
    }
    if let Some(path) = &output.json {
        write_json(path, result)?;
    }

    let problems = result.problems();
    let mut stderr = std::io::stderr();
    for problem in &problems {
        let _ = writeln!(stderr, "warning: {problem}");
    }
    Ok(problems.is_empty())
}

fn first_recording(result: &BenchmarkResult, variant: usize) -> Option<&Recording> {
    result.variants.get(variant)?.runs.iter().find_map(|r| r.recording.as_ref())
}

fn file_stem(variant: &Variant) -> String {
    let mut parts = vec![variant.name.clone()];
    parts.extend(variant.dimensions.iter().map(|(_, v)| v.clone()));
    parts
        .join("-")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') { c } else { '_' })
        .collect()
}

/// With `name`, the paths in `output` are directories and the file is named after the variant.
fn write_recording_files(recording: &Recording, output: &OutputArgs, name: Option<&str>) -> Result<(), String> {
    let target = |base: &Path, extension: &str| -> Result<PathBuf, String> {
        match name {
            Some(name) => {
                std::fs::create_dir_all(base).map_err(|e| format!("cannot create {}: {e}", base.display()))?;
                Ok(base.join(format!("{name}.{extension}")))
            }
            None => Ok(base.to_path_buf()),
        }
    };
    let series = SeriesOptions { processes: true };
    if let Some(base) = &output.svg {
        let path = target(base, "svg")?;
        let svg = document(&charts(recording, series), &SvgOptions::default());
        std::fs::write(&path, svg).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    if let Some(base) = &output.csv {
        let path = target(base, "csv")?;
        let mut out = Vec::new();
        system_info_report::export::write_wide_csv(recording, series, &mut out).map_err(|e| e.to_string())?;
        std::fs::write(&path, out).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    serde_json::to_writer(std::io::BufWriter::new(file), value).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn emit(text: &str, path: Option<&Path>) -> Result<(), String> {
    match path {
        Some(path) => std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display())),
        None => std::io::stdout().lock().write_all(text.as_bytes()).map_err(|e| e.to_string()),
    }
}
