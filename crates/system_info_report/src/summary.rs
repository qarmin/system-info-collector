use std::time::Duration;

use system_info_probe::process::{IoCounters, ProcessKey};
use system_info_probe::recording::Recording;

use crate::stats::Stats;
use crate::table::{Cell, Table};

/// What one group of processes consumed over a whole recording.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct GroupSummary {
    pub name: String,
    pub duration: Duration,
    /// Own CPU of the members plus children they reaped.
    pub cpu_time: Duration,
    pub reaped_cpu_time: Duration,
    /// Per tick, 100 meaning one core fully busy.
    pub cpu_percent: Option<Stats>,
    /// Sum of the members' RSS, per tick.
    pub rss_bytes: Option<Stats>,
    /// Highest sum of the members' own peaks (`VmHWM`), an upper bound for the group's peak.
    pub peak_rss_bytes_sum: Option<u64>,
    pub io: Option<IoCounters>,
    pub inherited_io: Option<IoCounters>,
    pub max_processes: usize,
    pub started: usize,
    pub exited: usize,
}

impl GroupSummary {
    /// Over the whole recording, 100 meaning one core - exact even when ticks differ in length.
    pub fn average_cpu_percent(&self) -> Option<f64> {
        let secs = self.duration.as_secs_f64();
        (secs > 0.0).then(|| self.cpu_time.as_secs_f64() * 100.0 / secs)
    }
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct SystemSummary {
    /// 100 meaning every core busy.
    pub cpu_percent: Option<Stats>,
    pub memory_used_bytes: Option<Stats>,
    pub memory_total_bytes: Option<u64>,
}

/// One process with its own series in the recording.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct ProcessSummary {
    pub key: ProcessKey,
    pub name: String,
    pub cmd: String,
    pub cpu_time: Duration,
    /// Over the ticks it was alive in, 100 meaning one core.
    pub cpu_percent: Option<Stats>,
    pub max_rss_bytes: u64,
    pub peak_rss_bytes: Option<u64>,
    pub io: Option<IoCounters>,
    pub alive_at_end: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Summary {
    pub duration: Duration,
    pub ticks: usize,
    pub logical_cores: usize,
    pub system: Option<SystemSummary>,
    pub groups: Vec<GroupSummary>,
    /// Most CPU first.
    pub processes: Vec<ProcessSummary>,
}

impl Summary {
    pub fn of(recording: &Recording) -> Self {
        Self {
            duration: recording.duration(),
            ticks: recording.ticks.len(),
            logical_cores: recording.logical_cores,
            system: system(recording),
            groups: (0..recording.groups.len()).map(|index| group(recording, index)).collect(),
            processes: processes(recording),
        }
    }

    pub fn group(&self, name: &str) -> Option<&GroupSummary> {
        self.groups.iter().find(|g| g.name == name)
    }
}

fn sum_io(values: impl Iterator<Item = Option<IoCounters>>) -> Option<IoCounters> {
    values.flatten().reduce(IoCounters::saturating_add)
}

fn group(recording: &Recording, index: usize) -> GroupSummary {
    let usages = || recording.ticks.iter().filter_map(move |t| t.groups.get(index).map(|g| (t, g)));
    GroupSummary {
        name: recording.groups[index].clone(),
        duration: recording.duration(),
        cpu_time: usages().map(|(_, g)| g.total_cpu_time()).sum(),
        reaped_cpu_time: usages().map(|(_, g)| g.reaped_cpu_time).sum(),
        cpu_percent: Stats::of(usages().map(|(t, g)| t.cpu_percent_of_core(g.total_cpu_time()))),
        rss_bytes: Stats::of(usages().filter(|(_, g)| g.processes > 0).map(|(_, g)| g.rss_bytes as f64)),
        peak_rss_bytes_sum: usages().filter_map(|(_, g)| g.peak_rss_bytes_sum).max(),
        io: sum_io(usages().map(|(_, g)| g.io)),
        inherited_io: sum_io(usages().map(|(_, g)| g.inherited_io)),
        max_processes: usages().map(|(_, g)| g.processes).max().unwrap_or(0),
        started: usages().map(|(_, g)| g.started).sum(),
        exited: usages().map(|(_, g)| g.exited).sum(),
    }
}

fn system(recording: &Recording) -> Option<SystemSummary> {
    let cpu = Stats::of(recording.ticks.iter().filter_map(|t| t.cpu.as_ref()).map(|c| f64::from(c.total_percent)));
    let memory = Stats::of(recording.ticks.iter().filter_map(|t| t.memory).map(|m| m.used_bytes as f64));
    (cpu.is_some() || memory.is_some()).then(|| SystemSummary {
        cpu_percent: cpu,
        memory_used_bytes: memory,
        memory_total_bytes: recording.ticks.iter().rev().find_map(|t| t.memory).map(|m| m.total_bytes),
    })
}

fn processes(recording: &Recording) -> Vec<ProcessSummary> {
    let mut out: Vec<ProcessSummary> = recording
        .processes
        .values()
        .map(|process| {
            let key = process.info.key;
            let ticks = || recording.ticks.iter().filter_map(move |t| t.processes.get(&key).map(|p| (t, p)));
            ProcessSummary {
                key,
                name: process.info.name.clone(),
                cmd: process.info.cmd_line(),
                cpu_time: ticks().map(|(_, p)| p.cpu_time).sum(),
                cpu_percent: Stats::of(ticks().map(|(t, p)| t.cpu_percent_of_core(p.cpu_time))),
                max_rss_bytes: ticks().map(|(_, p)| p.rss_bytes).max().unwrap_or(0),
                peak_rss_bytes: ticks().filter_map(|(_, p)| p.status).map(|s| s.peak_rss_bytes).max(),
                io: sum_io(ticks().map(|(_, p)| p.io)),
                alive_at_end: process.alive_at_end,
            }
        })
        .collect();
    out.sort_by(|a, b| b.cpu_time.cmp(&a.cpu_time).then(a.key.cmp(&b.key)));
    out
}

fn stat_cell(stats: Option<Stats>, pick: fn(&Stats) -> f64, to_cell: fn(f64) -> Cell) -> Cell {
    stats.map_or(Cell::Empty, |s| to_cell(pick(&s)))
}

fn bytes(value: f64) -> Cell {
    Cell::Bytes(value.max(0.0) as u64)
}

fn written(io: Option<IoCounters>) -> Cell {
    io.map_or(Cell::Empty, |io| Cell::Bytes(io.wchar))
}

impl Summary {
    /// One row per group.  CPU 100% means one core; `peak rss` sums each member's own peak.
    pub fn groups_table(&self) -> Table {
        let mut table = Table::new([
            "group", "cpu avg", "cpu p95", "cpu max", "cpu time", "rss avg", "rss max", "peak rss", "written", "procs", "started", "exited",
        ])
        .with_title(format!("Groups over {}", crate::table::human_duration(self.duration)));
        for group in &self.groups {
            table.push(vec![
                Cell::text(&group.name),
                group.average_cpu_percent().map_or(Cell::Empty, Cell::Percent),
                stat_cell(group.cpu_percent, |s| s.p95, Cell::Percent),
                stat_cell(group.cpu_percent, |s| s.max, Cell::Percent),
                Cell::Duration(group.cpu_time),
                stat_cell(group.rss_bytes, |s| s.mean, bytes),
                stat_cell(group.rss_bytes, |s| s.max, bytes),
                group.peak_rss_bytes_sum.map_or(Cell::Empty, Cell::Bytes),
                written(group.io),
                Cell::Integer(group.max_processes as i64),
                Cell::Integer(group.started as i64),
                Cell::Integer(group.exited as i64),
            ]);
        }
        table
    }

    /// The `limit` processes that used the most CPU.
    pub fn processes_table(&self, limit: usize) -> Table {
        let mut table = Table::new([
            "pid", "name", "cpu time", "cpu avg", "cpu max", "rss max", "peak rss", "written", "alive", "command",
        ])
        .with_title("Processes by CPU time");
        for process in self.processes.iter().take(limit) {
            table.push(vec![
                Cell::Integer(i64::from(process.key.pid)),
                Cell::text(&process.name),
                Cell::Duration(process.cpu_time),
                stat_cell(process.cpu_percent, |s| s.mean, Cell::Percent),
                stat_cell(process.cpu_percent, |s| s.max, Cell::Percent),
                Cell::Bytes(process.max_rss_bytes),
                process.peak_rss_bytes.map_or(Cell::Empty, Cell::Bytes),
                written(process.io),
                Cell::text(if process.alive_at_end { "yes" } else { "no" }),
                Cell::text(crate::table::shorten(&process.cmd, 60)),
            ]);
        }
        table
    }

    /// Machine-wide CPU (100% = every core) and memory.
    pub fn system_table(&self) -> Option<Table> {
        let system = self.system.as_ref()?;
        let mut table = Table::new(["metric", "mean", "p95", "max"]).with_title("Machine");
        if let Some(cpu) = system.cpu_percent {
            table.push(vec![
                Cell::text("cpu"),
                Cell::Percent(cpu.mean),
                Cell::Percent(cpu.p95),
                Cell::Percent(cpu.max),
            ]);
        }
        if let Some(memory) = system.memory_used_bytes {
            table.push(vec![Cell::text("memory used"), bytes(memory.mean), bytes(memory.p95), bytes(memory.max)]);
        }
        Some(table)
    }
}
