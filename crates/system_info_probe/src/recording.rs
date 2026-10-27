//! Turning a sequence of [`Snapshot`]s into time series.  Pure: no I/O and no clocks, so the
//! blocking and the tokio recorders produce identical recordings from identical snapshots.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "disk")]
use crate::disk::{DiskIoRates, DiskSpace};
use crate::gpu::GpuReading;
#[cfg(feature = "network")]
use crate::network::{NetworkCounters, NetworkRates};
use crate::process::{
    GroupAccumulator, GroupUsage, IoCounters, ProcessInfo, ProcessInterval, ProcessKey, ProcessSnapshot, ProcessState, Selection, Selector,
    StatusDetail,
};
use crate::snapshot::{Snapshot, Sources};
use crate::system::{CpuUsage, MemoryInfo};

/// Which processes get their own series, beyond the group totals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProcessSeries {
    #[default]
    None,
    /// Members of any group.
    GroupMembers,
    /// Every process; a few hundred entries per tick on a desktop.
    All,
}

#[derive(Clone, Debug, Default)]
pub struct RecordingConfig {
    /// Named sets of processes totalled per tick; membership follows new and exiting processes.
    pub groups: Vec<(String, Selector)>,
    pub process_series: ProcessSeries,
}

/// One process within one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessTick {
    pub cpu_time: Duration,
    pub reaped_children_cpu_time: Option<Duration>,
    pub rss_bytes: u64,
    pub state: ProcessState,
    pub threads: Option<u32>,
    pub io: Option<IoCounters>,
    pub inherited_io: Option<IoCounters>,
    pub status: Option<StatusDetail>,
}

#[cfg(feature = "disk")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiskTick {
    pub space: Option<DiskSpace>,
    pub io: Option<DiskIoRates>,
}

#[cfg(feature = "network")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NetworkTick {
    pub totals: Option<NetworkCounters>,
    pub rates: Option<NetworkRates>,
}

/// The interval between two consecutive snapshots.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Tick {
    /// End of the interval, from the first snapshot.
    pub offset: Duration,
    pub elapsed: Duration,
    pub cpu: Option<CpuUsage>,
    pub memory: Option<MemoryInfo>,
    /// Aligned with [`Recording::groups`].
    pub groups: Vec<GroupUsage>,
    pub processes: BTreeMap<ProcessKey, ProcessTick>,
    #[cfg(feature = "disk")]
    pub disks: Vec<DiskTick>,
    #[cfg(feature = "network")]
    pub networks: Vec<NetworkTick>,
    pub gpus: Vec<Option<GpuReading>>,
}

fn add_io(a: Option<IoCounters>, b: Option<IoCounters>) -> Option<IoCounters> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    }
}

fn add_duration(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

/// Time-weighted mean of two rates over intervals of `a_secs` and `b_secs`.
fn weighted(a: f64, a_secs: f64, b: f64, b_secs: f64) -> f64 {
    let total = a_secs + b_secs;
    if total <= 0.0 { b } else { (a * a_secs + b * b_secs) / total }
}

impl Tick {
    /// Folds the following tick into this one: amounts add up, rates are weighted by time,
    /// levels (memory, process counts) come from the later tick.
    pub fn absorb(&mut self, later: Self) {
        let (a, b) = (self.elapsed.as_secs_f64(), later.elapsed.as_secs_f64());
        self.cpu = match (self.cpu.take(), later.cpu) {
            (Some(mut mine), Some(theirs)) => {
                mine.total_percent = weighted(f64::from(mine.total_percent), a, f64::from(theirs.total_percent), b) as f32;
                for (core, other) in mine.per_core_percent.iter_mut().zip(&theirs.per_core_percent) {
                    *core = weighted(f64::from(*core), a, f64::from(*other), b) as f32;
                }
                Some(mine)
            }
            (mine, theirs) => theirs.or(mine),
        };
        self.memory = later.memory.or(self.memory);
        for (mine, theirs) in self.groups.iter_mut().zip(later.groups) {
            mine.cpu_time += theirs.cpu_time;
            mine.reaped_cpu_time += theirs.reaped_cpu_time;
            mine.io = add_io(mine.io, theirs.io);
            mine.inherited_io = add_io(mine.inherited_io, theirs.inherited_io);
            mine.peak_rss_bytes_sum = mine.peak_rss_bytes_sum.max(theirs.peak_rss_bytes_sum);
            mine.started += theirs.started;
            mine.exited += theirs.exited;
            mine.rss_bytes = theirs.rss_bytes;
            mine.threads = theirs.threads;
            mine.processes = theirs.processes;
        }
        for (key, theirs) in later.processes {
            match self.processes.get_mut(&key) {
                Some(mine) => {
                    mine.cpu_time += theirs.cpu_time;
                    mine.reaped_children_cpu_time = add_duration(mine.reaped_children_cpu_time, theirs.reaped_children_cpu_time);
                    mine.io = add_io(mine.io, theirs.io);
                    mine.inherited_io = add_io(mine.inherited_io, theirs.inherited_io);
                    mine.rss_bytes = theirs.rss_bytes;
                    mine.state = theirs.state;
                    mine.threads = theirs.threads;
                    mine.status = theirs.status.or(mine.status);
                }
                None => {
                    self.processes.insert(key, theirs);
                }
            }
        }
        #[cfg(feature = "disk")]
        for (mine, theirs) in self.disks.iter_mut().zip(later.disks) {
            mine.space = theirs.space.or(mine.space);
            mine.io = match (mine.io, theirs.io) {
                (Some(x), Some(y)) => Some(DiskIoRates {
                    busy_percent: weighted(x.busy_percent, a, y.busy_percent, b),
                    read_bytes_per_sec: weighted(x.read_bytes_per_sec, a, y.read_bytes_per_sec, b),
                    write_bytes_per_sec: weighted(x.write_bytes_per_sec, a, y.write_bytes_per_sec, b),
                }),
                (x, y) => y.or(x),
            };
        }
        #[cfg(feature = "network")]
        for (mine, theirs) in self.networks.iter_mut().zip(later.networks) {
            mine.totals = theirs.totals.or(mine.totals);
            mine.rates = match (mine.rates, theirs.rates) {
                (Some(x), Some(y)) => Some(NetworkRates {
                    rx_bytes_per_sec: weighted(x.rx_bytes_per_sec, a, y.rx_bytes_per_sec, b),
                    tx_bytes_per_sec: weighted(x.tx_bytes_per_sec, a, y.tx_bytes_per_sec, b),
                }),
                (x, y) => y.or(x),
            };
        }
        if !later.gpus.is_empty() {
            self.gpus = later.gpus;
        }
        self.offset = later.offset;
        self.elapsed += later.elapsed;
    }

    /// 100 means one core fully busy.
    pub fn cpu_percent_of_core(&self, cpu_time: Duration) -> f64 {
        let elapsed = self.elapsed.as_secs_f64();
        if elapsed <= 0.0 { 0.0 } else { cpu_time.as_secs_f64() * 100.0 / elapsed }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecordedProcess {
    pub info: Arc<ProcessInfo>,
    /// Index of the first tick the process appears in.
    pub first_tick: usize,
    pub last_tick: usize,
    pub alive_at_end: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Recording {
    /// Wall clock of the first snapshot, which only sets the baseline.
    pub start_unix: f64,
    pub logical_cores: usize,
    pub groups: Vec<String>,
    pub sources: Sources,
    /// Every process that appears in some tick's `processes`.
    pub processes: BTreeMap<ProcessKey, RecordedProcess>,
    pub ticks: Vec<Tick>,
}

impl Recording {
    pub fn duration(&self) -> Duration {
        self.ticks.last().map_or(Duration::ZERO, |t| t.offset)
    }

    pub fn group_index(&self, name: &str) -> Option<usize> {
        self.groups.iter().position(|g| g == name)
    }
}

/// Feeds snapshots in, gets ticks out.
pub struct RecordingBuilder {
    process_series: ProcessSeries,
    selections: Vec<Selection>,
    accumulators: Vec<GroupAccumulator>,
    previous: Option<Snapshot>,
    first_taken_at: Option<std::time::Instant>,
    recording: Recording,
}

impl RecordingBuilder {
    pub fn new(config: RecordingConfig) -> Self {
        let (names, selectors): (Vec<String>, Vec<Selector>) = config.groups.into_iter().unzip();
        Self {
            process_series: config.process_series,
            accumulators: vec![GroupAccumulator::new(); selectors.len()],
            selections: selectors.into_iter().map(Selection::new).collect(),
            previous: None,
            first_taken_at: None,
            recording: Recording {
                groups: names,
                ..Recording::default()
            },
        }
    }

    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    /// A last tick much shorter than the interval - the final sample right after a program
    /// exited - turns small amounts into absurd rates, so it is folded into the tick before.
    pub fn merge_short_last_tick(&mut self, shorter_than: Duration) {
        let ticks = &mut self.recording.ticks;
        if ticks.len() >= 2 && ticks.last().is_some_and(|t| t.elapsed < shorter_than) {
            let last = ticks.pop().expect("checked above");
            ticks.last_mut().expect("checked above").absorb(last);
            let merged = ticks.len() - 1;
            for process in self.recording.processes.values_mut() {
                process.first_tick = process.first_tick.min(merged);
                process.last_tick = process.last_tick.min(merged);
            }
        }
    }

    pub fn set_sources(&mut self, sources: Sources) {
        self.recording.sources = sources;
    }

    /// The first snapshot only sets the baseline; every later one adds a tick, returned here.
    pub fn push(&mut self, snapshot: Snapshot) -> Option<&Tick> {
        let members: Vec<HashSet<ProcessKey>> = match &snapshot.processes {
            Some(processes) => self.selections.iter_mut().map(|s| s.update(processes)).collect(),
            None => vec![HashSet::new(); self.selections.len()],
        };
        let Some(previous) = self.previous.replace(snapshot) else {
            let first = self.previous.as_ref()?;
            self.first_taken_at = Some(first.taken_at);
            self.recording.start_unix = first.unix_time;
            if let Some(processes) = &first.processes {
                self.recording.logical_cores = processes.logical_cores;
                // Accumulators learn the baseline members so the first tick does not count them as new.
                for (accumulator, members) in self.accumulators.iter_mut().zip(&members) {
                    let interval = ProcessInterval::between(processes, processes);
                    accumulator.add(processes, processes, &interval, members);
                }
            } else if let Some(system) = &first.system {
                self.recording.logical_cores = system.logical_cores();
            }
            return None;
        };
        let current = self.previous.take()?;
        let tick = self.tick(&previous, &current, &members);
        self.previous = Some(current);
        self.recording.ticks.push(tick);
        self.recording.ticks.last()
    }

    /// Adds what `pid` consumed after it was last sampled, given its final total as reported by
    /// whoever reaped it (`wait4` for a spawned command).  That tail, including children it reaped
    /// in the meantime, is otherwise lost with the process.
    pub fn credit_final_cpu(&mut self, group: usize, pid: u32, final_total: Duration) {
        let alive = self.previous.as_ref().and_then(|s| s.processes.as_ref()).and_then(|p| p.by_pid(pid));
        let (Some(accumulator), Some(tick)) = (self.accumulators.get(group), self.recording.ticks.last_mut()) else {
            return;
        };
        let Some(counted) = accumulator.counted_for(pid, alive) else { return };
        if let Some(usage) = tick.groups.get_mut(group) {
            usage.reaped_cpu_time += final_total.saturating_sub(counted);
        }
    }

    pub fn finish(mut self) -> Recording {
        let last = self.recording.ticks.len().saturating_sub(1);
        let alive: HashSet<ProcessKey> = self
            .previous
            .as_ref()
            .and_then(|s| s.processes.as_ref())
            .map(|p| p.processes.keys().copied().collect())
            .unwrap_or_default();
        for process in self.recording.processes.values_mut() {
            process.alive_at_end = process.last_tick == last && alive.contains(&process.info.key);
        }
        self.recording
    }

    fn tick(&mut self, earlier: &Snapshot, later: &Snapshot, members: &[HashSet<ProcessKey>]) -> Tick {
        let index = self.recording.ticks.len();
        let mut tick = Tick {
            offset: later.taken_at.saturating_duration_since(self.first_taken_at.unwrap_or(earlier.taken_at)),
            elapsed: later.taken_at.saturating_duration_since(earlier.taken_at),
            cpu: match (&earlier.system, &later.system) {
                (Some(before), Some(now)) => now.cpu_usage_since(before),
                _ => None,
            },
            memory: later.system.as_ref().map(|s| s.memory),
            groups: Vec::with_capacity(members.len()),
            processes: BTreeMap::new(),
            #[cfg(feature = "disk")]
            disks: disk_ticks(earlier, later),
            #[cfg(feature = "network")]
            networks: network_ticks(earlier, later),
            gpus: later.gpus.as_ref().map(|g| g.readings.clone()).unwrap_or_default(),
        };

        if let (Some(before), Some(now)) = (&earlier.processes, &later.processes) {
            let interval = ProcessInterval::between(before, now);
            if let Some(cores) = (now.logical_cores > 0).then_some(now.logical_cores) {
                self.recording.logical_cores = cores;
            }
            for (accumulator, members) in self.accumulators.iter_mut().zip(members) {
                tick.groups.push(accumulator.add(before, now, &interval, members));
            }
            self.record_processes(&mut tick, now, &interval, members, index);
        }
        tick
    }

    fn record_processes(
        &mut self,
        tick: &mut Tick,
        now: &ProcessSnapshot,
        interval: &ProcessInterval,
        members: &[HashSet<ProcessKey>],
        index: usize,
    ) {
        let in_series = |key: &ProcessKey| match self.process_series {
            ProcessSeries::None => false,
            ProcessSeries::GroupMembers => members.iter().any(|m| m.contains(key)),
            ProcessSeries::All => true,
        };
        for (key, delta) in &interval.deltas {
            if !in_series(key) {
                continue;
            }
            let process = &now.processes[key];
            tick.processes.insert(
                *key,
                ProcessTick {
                    cpu_time: delta.cpu_time,
                    reaped_children_cpu_time: delta.reaped_children_cpu_time,
                    rss_bytes: process.rss_bytes,
                    state: process.state,
                    threads: process.threads,
                    io: delta.io,
                    inherited_io: delta.inherited_io,
                    status: process.status,
                },
            );
            self.recording
                .processes
                .entry(*key)
                .and_modify(|p| {
                    p.last_tick = index;
                    p.info = Arc::clone(&process.info);
                })
                .or_insert_with(|| RecordedProcess {
                    info: Arc::clone(&process.info),
                    first_tick: index,
                    last_tick: index,
                    alive_at_end: false,
                });
        }
    }
}

#[cfg(feature = "disk")]
fn disk_ticks(earlier: &Snapshot, later: &Snapshot) -> Vec<DiskTick> {
    let Some(now) = &later.disks else { return Vec::new() };
    let rates = earlier.disks.as_ref().map(|before| now.io_since(before)).unwrap_or_default();
    now.space
        .iter()
        .enumerate()
        .map(|(i, space)| DiskTick {
            space: *space,
            io: rates.get(i).copied().flatten(),
        })
        .collect()
}

#[cfg(feature = "network")]
fn network_ticks(earlier: &Snapshot, later: &Snapshot) -> Vec<NetworkTick> {
    let Some(now) = &later.networks else { return Vec::new() };
    let rates = earlier.networks.as_ref().map(|before| now.rates_since(before)).unwrap_or_default();
    now.counters
        .iter()
        .enumerate()
        .map(|(i, totals)| NetworkTick {
            totals: *totals,
            rates: rates.get(i).copied().flatten(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::process::ProcessSample;

    fn process(pid: u32, ppid: u32, cpu_ms: u64) -> ProcessSample {
        ProcessSample {
            info: Arc::new(ProcessInfo {
                key: ProcessKey { pid, start: u64::from(pid) },
                name: format!("p{pid}"),
                cmd: vec![format!("p{pid}")],
                exe: None,
                uid: None,
                kernel_thread: false,
                start_unix: 0.0,
            }),
            ppid: Some(ppid),
            state: ProcessState::Sleeping,
            cpu_time: Duration::from_millis(cpu_ms),
            reaped_children_cpu_time: Some(Duration::ZERO),
            rss_bytes: 1 << 20,
            threads: Some(1),
            io: None,
            status: None,
        }
    }

    fn snapshot(at: Instant, processes: Vec<ProcessSample>) -> Snapshot {
        Snapshot {
            taken_at: at,
            unix_time: 1000.0,
            processes: Some(ProcessSnapshot {
                taken_at: at,
                unix_time: 1000.0,
                logical_cores: 2,
                processes: processes.into_iter().map(|p| (p.key(), p)).collect(),
            }),
            system: None,
            #[cfg(feature = "disk")]
            disks: None,
            #[cfg(feature = "network")]
            networks: None,
            gpus: None,
        }
    }

    #[test]
    fn first_snapshot_is_a_baseline_and_groups_follow_new_children() {
        let t = Instant::now();
        let s = |ms| t + Duration::from_millis(ms);
        let mut builder = RecordingBuilder::new(RecordingConfig {
            groups: vec![("app".to_string(), Selector::Tree(10))],
            process_series: ProcessSeries::GroupMembers,
        });
        assert!(builder.push(snapshot(s(0), vec![process(1, 0, 0), process(10, 1, 100)])).is_none());

        let tick = builder
            .push(snapshot(s(500), vec![process(1, 0, 0), process(10, 1, 200), process(11, 10, 50)]))
            .expect("second snapshot makes a tick")
            .clone();
        assert_eq!(tick.offset, Duration::from_millis(500));
        assert_eq!(tick.groups[0].cpu_time, Duration::from_millis(150));
        assert_eq!(tick.groups[0].started, 1);
        assert!((tick.cpu_percent_of_core(tick.groups[0].cpu_time) - 30.0).abs() < 1e-9);
        assert_eq!(tick.processes.len(), 2);

        builder.push(snapshot(s(1000), vec![process(1, 0, 0), process(10, 1, 200)]));
        let recording = builder.finish();
        assert_eq!(recording.ticks.len(), 2);
        assert!((recording.start_unix - 1000.0).abs() < f64::EPSILON);
        assert_eq!(recording.logical_cores, 2);
        let child = recording.processes.values().find(|p| p.info.key.pid == 11).expect("child recorded");
        assert!(!child.alive_at_end);
        assert_eq!((child.first_tick, child.last_tick), (0, 0));
        let root = recording.processes.values().find(|p| p.info.key.pid == 10).expect("root recorded");
        assert!(root.alive_at_end);
        assert_eq!(recording.duration(), Duration::from_secs(1));
    }

    #[test]
    fn a_short_last_tick_is_folded_into_the_one_before() {
        let t = Instant::now();
        let s = |ms| t + Duration::from_millis(ms);
        let mut builder = RecordingBuilder::new(RecordingConfig {
            groups: vec![("app".to_string(), Selector::Tree(10))],
            process_series: ProcessSeries::GroupMembers,
        });
        builder.push(snapshot(s(0), vec![process(10, 1, 0)]));
        builder.push(snapshot(s(500), vec![process(10, 1, 100)]));
        builder.push(snapshot(s(1000), vec![process(10, 1, 300)]));
        builder.push(snapshot(s(1002), vec![process(10, 1, 310)]));
        builder.merge_short_last_tick(Duration::from_millis(250));
        let recording = builder.finish();
        assert_eq!(recording.ticks.len(), 2);
        let last = &recording.ticks[1];
        assert_eq!(last.elapsed, Duration::from_millis(502));
        assert_eq!(last.offset, Duration::from_millis(1002));
        assert_eq!(last.groups[0].cpu_time, Duration::from_millis(210));
        assert!(recording.processes.values().all(|p| p.last_tick == 1));
    }
}
