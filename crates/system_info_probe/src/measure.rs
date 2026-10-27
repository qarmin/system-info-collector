//! Measuring one program: what both the blocking and the tokio runner share.

use std::time::{Duration, Instant};

pub use crate::child::ChildExit;
use crate::process::{DetailRefresh, ProcessDetail, ProcessSamplerConfig, Selection, Selector};
use crate::recording::{ProcessSeries, Recording, RecordingBuilder, RecordingConfig};
use crate::schedule::next_deadline;
use crate::snapshot::SamplerConfig;

/// Name of the group holding the measured process in [`Measurement::recording`].
pub const TARGET_GROUP: &str = "target";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureScope {
    Process,
    /// The process and all its descendants, including ones started during the measurement.  A
    /// spawned command gets a process group of its own so the whole tree is killed at the end.
    Tree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunLength {
    /// Measure this long after the warm-up, then kill a spawned command.
    Fixed(Duration),
    /// Measure until the process exits; a spawned command is killed at the timeout.
    UntilExit { timeout: Option<Duration> },
}

#[derive(Clone, Debug)]
pub struct MeasureOptions {
    /// Not sampled at all - startup cost is usually not what is being measured.
    pub warmup: Duration,
    pub run: RunLength,
    pub interval: Duration,
    pub scope: MeasureScope,
    pub detail: ProcessDetail,
    pub refresh: DetailRefresh,
    /// Also record machine-wide CPU and memory.
    pub system: bool,
    pub process_series: ProcessSeries,
}

impl Default for MeasureOptions {
    fn default() -> Self {
        Self {
            warmup: Duration::ZERO,
            run: RunLength::Fixed(Duration::from_secs(10)),
            interval: Duration::from_millis(250),
            scope: MeasureScope::Tree,
            detail: ProcessDetail::Full,
            refresh: DetailRefresh::default(),
            system: true,
            process_series: ProcessSeries::GroupMembers,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Ran for the whole [`RunLength::Fixed`] time.
    Completed,
    /// Exited by itself while being measured.
    Exited,
    /// Exited before the warm-up ended, so nothing was measured.
    ExitedDuringWarmup,
    /// [`RunLength::UntilExit`] reached its timeout.
    TimedOut,
}

#[derive(Clone, Debug)]
pub struct Measurement {
    pub recording: Recording,
    pub ending: Ending,
    /// For spawned commands; `None` when an existing process was attached to.
    pub exit: Option<ChildExit>,
}

impl MeasureOptions {
    pub(crate) fn selector(&self, pid: u32) -> Selector {
        match self.scope {
            MeasureScope::Process => Selector::Pid(pid),
            MeasureScope::Tree => Selector::Tree(pid),
        }
    }

    pub(crate) fn sampler_config(&self, pid: u32) -> SamplerConfig {
        SamplerConfig {
            processes: Some(ProcessSamplerConfig {
                detail: self.detail,
                detail_scope: Some(Selection::new(self.selector(pid))),
                refresh: self.refresh,
            }),
            system: self.system,
            ..SamplerConfig::default()
        }
    }

    pub(crate) fn recording_config(&self, pid: u32) -> RecordingConfig {
        RecordingConfig {
            groups: vec![(TARGET_GROUP.to_string(), self.selector(pid))],
            process_series: self.process_series.clone(),
        }
    }

    /// Absolute limit on the measured phase, if any.
    pub(crate) fn limit(&self) -> Option<Duration> {
        match self.run {
            RunLength::Fixed(duration) => Some(duration),
            RunLength::UntilExit { timeout } => timeout,
        }
    }

    pub(crate) fn ending_at_limit(&self) -> Ending {
        match self.run {
            RunLength::Fixed(_) => Ending::Completed,
            RunLength::UntilExit { .. } => Ending::TimedOut,
        }
    }

    pub(crate) fn validate(&self) -> crate::ProbeResult<()> {
        if self.interval.is_zero() {
            return Err(crate::ProbeError::InvalidConfig("the sampling interval must be above zero".to_string()));
        }
        Ok(())
    }
}

/// The decisions of the measuring loop, shared by both runners; they only differ in how they wait.
pub(crate) struct MeasureLoop {
    interval: Duration,
    deadline: Instant,
    end: Option<Instant>,
    ending_at_limit: Ending,
    attached: bool,
}

impl MeasureLoop {
    /// Call right after the baseline sample.
    pub fn start(options: &MeasureOptions, attached: bool) -> Self {
        let start = Instant::now();
        Self {
            interval: options.interval,
            deadline: start,
            end: options.limit().map(|limit| start + limit),
            ending_at_limit: options.ending_at_limit(),
            attached,
        }
    }

    /// When the next sample is due, never past the end of the run.
    pub fn next_wake(&mut self) -> Instant {
        self.deadline = next_deadline(self.deadline, self.interval, Instant::now());
        self.end.map_or(self.deadline, |end| self.deadline.min(end))
    }

    /// After the sample taken at `woke_at`: how the run ends, if it does now.  An attached process
    /// is followed until nothing of the target group is left.
    pub fn after_sample(&self, woke_at: Instant, child_exited: bool, builder: &RecordingBuilder) -> Option<Ending> {
        let group_empty = || {
            builder
                .recording()
                .ticks
                .last()
                .is_some_and(|tick| tick.groups.first().is_none_or(|g| g.processes == 0))
        };
        if child_exited || (self.attached && group_empty()) {
            return Some(Ending::Exited);
        }
        self.end.is_some_and(|end| woke_at >= end).then_some(self.ending_at_limit)
    }
}

/// Where a measuring loop stopped, before the spawned command is reaped.
pub(crate) struct Stopped {
    pub builder: RecordingBuilder,
    pub ending: Ending,
}

impl Stopped {
    /// Credits what the command did after its last sample, which only its `wait4` totals show.
    pub fn into_measurement(mut self, pid: u32, exit: Option<ChildExit>) -> Measurement {
        if let Some(total) = exit.and_then(|e| e.cpu_time) {
            self.builder.credit_final_cpu(0, pid, total);
        }
        Measurement {
            recording: self.builder.finish(),
            ending: self.ending,
            exit,
        }
    }
}
