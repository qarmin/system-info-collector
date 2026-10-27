//! Running programs repeatedly under measurement and comparing them: variants x repeats, with the
//! median of each figure and, optionally, its range.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use system_info_probe::ProbeResult;
use system_info_probe::measure::{Ending, MeasureOptions, Measurement, TARGET_GROUP};
use system_info_probe::recording::Recording;

use crate::stats::Stats;
use crate::summary::{GroupSummary, Summary};
use crate::table::{Cell, Table};

/// One program to run, plus labels that become table columns (e.g. `size=1024x768`, `mode=idle`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub dimensions: Vec<(String, String)>,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

impl Variant {
    pub fn new(name: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn dimension(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.dimensions.push((key.into(), value.into()));
        self
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    fn dimension_value(&self, key: &str) -> Option<&str> {
        self.dimensions.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// What a metric can look at after one run.
pub struct RunOutput<'a> {
    pub variant: &'a Variant,
    pub measurement: &'a Measurement,
    pub summary: &'a GroupSummary,
    /// Stdout and stderr of the run, interleaved; empty when logs are not captured.
    pub log: &'a str,
}

/// A figure computed per run, beyond the built-in CPU and memory ones - typically parsed from
/// the program's own output.
pub trait Metric: Send + Sync {
    fn name(&self) -> &str;

    fn decimals(&self) -> u8 {
        1
    }

    fn extract(&self, run: &RunOutput<'_>) -> Option<f64>;
}

/// A [`Metric`] from a closure.
pub struct FnMetric<F> {
    name: String,
    decimals: u8,
    extract: F,
}

impl<F: Fn(&RunOutput<'_>) -> Option<f64> + Send + Sync> FnMetric<F> {
    pub fn new(name: impl Into<String>, decimals: u8, extract: F) -> Self {
        Self {
            name: name.into(),
            decimals,
            extract,
        }
    }
}

impl<F: Fn(&RunOutput<'_>) -> Option<f64> + Send + Sync> Metric for FnMetric<F> {
    fn name(&self) -> &str {
        &self.name
    }

    fn decimals(&self) -> u8 {
        self.decimals
    }

    fn extract(&self, run: &RunOutput<'_>) -> Option<f64> {
        (self.extract)(run)
    }
}

/// Set-up and tear-down around every run, e.g. starting a virtual display.
pub trait RunHooks: Send + Sync {
    fn before(&self, variant: &Variant, repeat: usize) -> Result<(), String>;

    fn after(&self, variant: &Variant, repeat: usize);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LogCapture {
    /// Output goes nowhere; metrics see an empty log.
    #[default]
    Discard,
    /// Into a temporary file, deleted after the metrics read it.
    Capture,
    /// Into `<dir>/<variant>-<repeat>.log`, kept.
    KeepIn(PathBuf),
}

pub struct Benchmark {
    pub variants: Vec<Variant>,
    pub repeats: usize,
    pub options: MeasureOptions,
    pub metrics: Vec<Box<dyn Metric>>,
    pub logs: LogCapture,
    pub hooks: Option<Box<dyn RunHooks>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    pub repeat: usize,
    pub ending: Ending,
    pub exit_success: Option<bool>,
    /// Over the whole run, 100 meaning one core.
    pub cpu_percent: Option<f64>,
    pub cpu_time: Duration,
    pub max_rss_bytes: Option<u64>,
    /// Sum of each process's own peak RSS, what `VmHWM` summed over the tree reports.
    pub peak_rss_bytes_sum: Option<u64>,
    /// Aligned with [`BenchmarkResult::metrics`].
    pub metrics: Vec<Option<f64>>,
    /// Why the run produced nothing, e.g. a failing hook.
    pub error: Option<String>,
    pub recording: Option<Recording>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariantResult {
    pub variant: Variant,
    pub runs: Vec<RunResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkResult {
    /// Name and decimals of each custom metric.
    pub metrics: Vec<(String, u8)>,
    pub variants: Vec<VariantResult>,
}

/// Progress notification before each run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress<'a> {
    pub variant: &'a Variant,
    pub repeat: usize,
    pub run: usize,
    pub total_runs: usize,
}

/// Per-run plumbing shared by the blocking and the tokio runner.
struct Prepared {
    command: Command,
    log: Option<RunLog>,
}

struct RunLog {
    path: PathBuf,
    keep: bool,
}

/// Distinguishes the temporary logs of benchmarks running at the same time in one process.
static TEMP_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);

impl Benchmark {
    pub fn new(variants: Vec<Variant>, repeats: usize, options: MeasureOptions) -> Self {
        Self {
            variants,
            repeats: repeats.max(1),
            options,
            metrics: Vec::new(),
            logs: LogCapture::Discard,
            hooks: None,
        }
    }

    pub fn metric(mut self, metric: impl Metric + 'static) -> Self {
        self.metrics.push(Box::new(metric));
        self
    }

    fn prepare(&self, variant: &Variant, repeat: usize) -> std::io::Result<Prepared> {
        let mut command = variant.command();
        command.stdin(Stdio::null());
        let log = match &self.logs {
            LogCapture::Discard => None,
            LogCapture::Capture => Some(RunLog {
                path: std::env::temp_dir().join(format!(
                    "bench-{}-{}-{}.log",
                    std::process::id(),
                    TEMP_LOG_COUNTER.fetch_add(1, Ordering::Relaxed),
                    crate::series::sanitize(&variant.name)
                )),
                keep: false,
            }),
            LogCapture::KeepIn(dir) => {
                std::fs::create_dir_all(dir)?;
                Some(RunLog {
                    path: dir.join(format!("{}-{repeat}.log", crate::series::sanitize(&variant.name))),
                    keep: true,
                })
            }
        };
        match &log {
            Some(RunLog { path, .. }) => {
                let file = std::fs::File::create(path)?;
                command.stdout(file.try_clone()?).stderr(file);
            }
            None => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        Ok(Prepared { command, log })
    }

    fn finish(&self, variant: &Variant, repeat: usize, log: Option<&RunLog>, measured: ProbeResult<Measurement>) -> RunResult {
        let text = log
            .and_then(|log| std::fs::read(&log.path).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        if let Some(log) = log
            && !log.keep
        {
            let _ = std::fs::remove_file(&log.path);
        }
        if let Some(hooks) = &self.hooks {
            hooks.after(variant, repeat);
        }
        let measurement = match measured {
            Ok(measurement) => measurement,
            Err(e) => return failed(repeat, e.to_string(), self.metrics.len()),
        };
        let summary = Summary::of(&measurement.recording);
        let Some(group) = summary.group(TARGET_GROUP) else {
            return failed(repeat, "no target group in the recording".to_string(), self.metrics.len());
        };
        let output = RunOutput {
            variant,
            measurement: &measurement,
            summary: group,
            log: &text,
        };
        RunResult {
            repeat,
            ending: measurement.ending,
            exit_success: measurement.exit.map(|e| e.status.success()),
            cpu_percent: group.average_cpu_percent(),
            cpu_time: group.cpu_time,
            max_rss_bytes: group.rss_bytes.map(|s| s.max as u64),
            peak_rss_bytes_sum: group.peak_rss_bytes_sum,
            metrics: self.metrics.iter().map(|m| m.extract(&output)).collect(),
            error: None,
            recording: Some(measurement.recording.clone()),
        }
    }

    fn plan(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.variants.len()).flat_map(move |v| (0..self.repeats).map(move |r| (v, r)))
    }

    fn result(&self, runs: Vec<Vec<RunResult>>) -> BenchmarkResult {
        BenchmarkResult {
            metrics: self.metrics.iter().map(|m| (m.name().to_string(), m.decimals())).collect(),
            variants: self
                .variants
                .iter()
                .cloned()
                .zip(runs)
                .map(|(variant, runs)| VariantResult { variant, runs })
                .collect(),
        }
    }

    /// Runs every variant `repeats` times, one after another.
    pub fn run(&self, mut progress: impl FnMut(Progress<'_>)) -> BenchmarkResult {
        let total_runs = self.variants.len() * self.repeats;
        let mut runs: Vec<Vec<RunResult>> = vec![Vec::new(); self.variants.len()];
        for (run, (index, repeat)) in self.plan().enumerate() {
            let variant = &self.variants[index];
            progress(Progress {
                variant,
                repeat,
                run,
                total_runs,
            });
            runs[index].push(self.run_once(variant, repeat, |command| {
                system_info_probe::blocking::measure_command(command, &self.options)
            }));
        }
        self.result(runs)
    }

    fn run_once(&self, variant: &Variant, repeat: usize, measure: impl FnOnce(&mut Command) -> ProbeResult<Measurement>) -> RunResult {
        if let Some(hooks) = &self.hooks
            && let Err(e) = hooks.before(variant, repeat)
        {
            return failed(repeat, format!("before hook: {e}"), self.metrics.len());
        }
        match self.prepare(variant, repeat) {
            Ok(mut prepared) => {
                let measured = measure(&mut prepared.command);
                self.finish(variant, repeat, prepared.log.as_ref(), measured)
            }
            Err(e) => self.finish(variant, repeat, None, Err(e.into())),
        }
    }

    /// [`Self::run`] on tokio.
    #[cfg(feature = "tokio")]
    pub async fn run_async(&self, mut progress: impl FnMut(Progress<'_>)) -> BenchmarkResult {
        let total_runs = self.variants.len() * self.repeats;
        let mut runs: Vec<Vec<RunResult>> = vec![Vec::new(); self.variants.len()];
        for (run, (index, repeat)) in self.plan().enumerate() {
            let variant = &self.variants[index];
            progress(Progress {
                variant,
                repeat,
                run,
                total_runs,
            });
            if let Some(hooks) = &self.hooks
                && let Err(e) = hooks.before(variant, repeat)
            {
                runs[index].push(failed(repeat, format!("before hook: {e}"), self.metrics.len()));
                continue;
            }
            let result = match self.prepare(variant, repeat) {
                Ok(mut prepared) => {
                    let measured = system_info_probe::tokio::measure_command(&mut prepared.command, &self.options).await;
                    self.finish(variant, repeat, prepared.log.as_ref(), measured)
                }
                Err(e) => self.finish(variant, repeat, None, Err(e.into())),
            };
            runs[index].push(result);
        }
        self.result(runs)
    }
}

fn failed(repeat: usize, error: String, metrics: usize) -> RunResult {
    RunResult {
        repeat,
        ending: Ending::Exited,
        exit_success: None,
        cpu_percent: None,
        cpu_time: Duration::ZERO,
        max_rss_bytes: None,
        peak_rss_bytes_sum: None,
        metrics: vec![None; metrics],
        error: Some(error),
        recording: None,
    }
}

impl BenchmarkResult {
    /// One row per variant with the median of each figure over its runs; with `spread`, every
    /// figure also gets its min and max columns.  Failed runs are left out and counted.
    pub fn table(&self, spread: bool) -> Table {
        let mut dimensions: Vec<&str> = Vec::new();
        for result in &self.variants {
            for (key, _) in &result.variant.dimensions {
                if !dimensions.contains(&key.as_str()) {
                    dimensions.push(key);
                }
            }
        }
        type Figure = (String, fn(f64) -> Cell, Box<dyn Fn(&RunResult) -> Option<f64>>);
        let mut figures: Vec<Figure> = vec![
            ("cpu".to_string(), Cell::Percent, Box::new(|r: &RunResult| r.cpu_percent)),
            (
                "cpu time".to_string(),
                |v| Cell::Duration(Duration::from_secs_f64(v.max(0.0))),
                Box::new(|r: &RunResult| r.error.is_none().then_some(r.cpu_time.as_secs_f64())),
            ),
            (
                "rss max".to_string(),
                |v| Cell::Bytes(v as u64),
                Box::new(|r: &RunResult| r.max_rss_bytes.map(|b| b as f64)),
            ),
            (
                "peak rss".to_string(),
                |v| Cell::Bytes(v as u64),
                Box::new(|r: &RunResult| r.peak_rss_bytes_sum.map(|b| b as f64)),
            ),
        ];
        for (index, (name, decimals)) in self.metrics.iter().enumerate() {
            let to_cell: fn(f64) -> Cell = match decimals {
                0 => |v| Cell::number(v, 0),
                1 => |v| Cell::number(v, 1),
                2 => |v| Cell::number(v, 2),
                _ => |v| Cell::number(v, 3),
            };
            figures.push((
                name.clone(),
                to_cell,
                Box::new(move |r: &RunResult| r.metrics.get(index).copied().flatten()),
            ));
        }

        let mut headers: Vec<String> = vec!["variant".to_string()];
        headers.extend(dimensions.iter().map(ToString::to_string));
        for (name, _, _) in &figures {
            headers.push(name.clone());
            if spread {
                headers.push(format!("{name} min"));
                headers.push(format!("{name} max"));
            }
        }
        headers.push("runs".to_string());
        let mut table = Table::new(headers);
        for result in &self.variants {
            let mut row = vec![Cell::text(&result.variant.name)];
            row.extend(
                dimensions
                    .iter()
                    .map(|d| result.variant.dimension_value(d).map_or(Cell::Empty, Cell::text)),
            );
            for (_, to_cell, pick) in &figures {
                let stats = Stats::of(result.runs.iter().filter_map(pick));
                row.push(stats.map_or(Cell::Empty, |s| to_cell(s.median)));
                if spread {
                    row.push(stats.map_or(Cell::Empty, |s| to_cell(s.min)));
                    row.push(stats.map_or(Cell::Empty, |s| to_cell(s.max)));
                }
            }
            let ok = result.runs.iter().filter(|r| r.error.is_none()).count();
            row.push(Cell::text(if ok == result.runs.len() {
                ok.to_string()
            } else {
                format!("{ok}/{}", result.runs.len())
            }));
            table.push(row);
        }
        table
    }

    /// Runs that failed or whose program exited unsuccessfully, for reporting next to the table.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for result in &self.variants {
            for run in &result.runs {
                if let Some(error) = &run.error {
                    out.push(format!("{} run {}: {error}", result.variant.name, run.repeat + 1));
                } else if run.ending == Ending::ExitedDuringWarmup {
                    out.push(format!("{} run {}: exited during the warm-up", result.variant.name, run.repeat + 1));
                }
            }
        }
        out
    }
}
