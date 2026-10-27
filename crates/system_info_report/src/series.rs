//! A recording as named time series, the common ground of CSV export and charts.

use system_info_probe::process::GroupUsage;
use system_info_probe::recording::{Recording, Tick};

const MIB: f64 = 1_048_576.0;
#[cfg(feature = "disk")]
const GIB: f64 = 1_073_741_824.0;

/// Units are picked so values need few digits, which is most of what keeps exports small.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// 100 meaning one core for processes, every core for the machine.
    Percent,
    Mib,
    Gib,
    MibPerSec,
    Celsius,
    Count,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Percent => "pct",
            Self::Mib => "mib",
            Self::Gib => "gib",
            Self::MibPerSec => "mibps",
            Self::Celsius => "c",
            Self::Count => "n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Percent => "%",
            Self::Mib => "MiB",
            Self::Gib => "GiB",
            Self::MibPerSec => "MiB/s",
            Self::Celsius => "°C",
            Self::Count => "count",
        }
    }

    /// Enough to tell real differences apart and no more.
    pub fn decimals(self) -> u8 {
        match self {
            Self::Percent | Self::Mib | Self::Gib => 1,
            Self::MibPerSec => 2,
            Self::Celsius | Self::Count => 0,
        }
    }
}

/// Which chart a series belongs on - series sharing a kind share an axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SeriesKind {
    SystemCpu,
    SystemMemory,
    GroupCpu,
    GroupMemory,
    GroupProcesses,
    ProcessCpu,
    ProcessMemory,
    DiskBusy,
    DiskIo,
    DiskUsed,
    Network,
    GpuUtilization,
    GpuMemory,
    GpuTemperature,
}

impl SeriesKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::SystemCpu => "CPU",
            Self::SystemMemory => "Memory",
            Self::GroupCpu => "CPU by group",
            Self::GroupMemory => "Memory by group",
            Self::GroupProcesses => "Processes by group",
            Self::ProcessCpu => "CPU by process",
            Self::ProcessMemory => "Memory by process",
            Self::DiskBusy => "Disk busy",
            Self::DiskIo => "Disk I/O",
            Self::DiskUsed => "Disk used",
            Self::Network => "Network",
            Self::GpuUtilization => "GPU utilization",
            Self::GpuMemory => "GPU memory",
            Self::GpuTemperature => "GPU temperature",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// Short machine name, e.g. `app.cpu_pct`; also the CSV column.
    pub name: String,
    /// For legends, e.g. `app`.
    pub label: String,
    pub kind: SeriesKind,
    pub unit: Unit,
    /// One per tick; `None` where nothing was measured.
    pub values: Vec<Option<f64>>,
}

impl Series {
    fn new(kind: SeriesKind, unit: Unit, name: &str, label: impl Into<String>, values: Vec<Option<f64>>) -> Self {
        Self {
            name: format!("{}.{}", sanitize(name), unit.suffix()),
            label: label.into(),
            kind,
            unit,
            values,
        }
    }

    fn has_values(&self) -> bool {
        self.values.iter().any(Option::is_some)
    }
}

/// Lowercase ASCII with `_` for anything else, so names work as CSV columns and in scripts.
pub fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeriesOptions {
    /// Add a CPU and an RSS series for every process with its own series in the recording.
    pub processes: bool,
}

/// Seconds from the start of the recording to the end of each tick.
pub fn times(recording: &Recording) -> Vec<f64> {
    recording.ticks.iter().map(|t| t.offset.as_secs_f64()).collect()
}

/// Every series with at least one value, in a stable order.
pub fn series(recording: &Recording, options: SeriesOptions) -> Vec<Series> {
    let ticks = &recording.ticks;
    let per_tick = |f: &dyn Fn(&Tick) -> Option<f64>| -> Vec<Option<f64>> { ticks.iter().map(f).collect() };
    let mut out = vec![
        Series::new(
            SeriesKind::SystemCpu,
            Unit::Percent,
            "cpu",
            "CPU",
            per_tick(&|t| t.cpu.as_ref().map(|c| f64::from(c.total_percent))),
        ),
        Series::new(
            SeriesKind::SystemMemory,
            Unit::Mib,
            "mem_used",
            "used",
            per_tick(&|t| t.memory.map(|m| m.used_bytes as f64 / MIB)),
        ),
        Series::new(
            SeriesKind::SystemMemory,
            Unit::Mib,
            "swap_used",
            "swap used",
            per_tick(&|t| t.memory.map(|m| m.swap_used_bytes() as f64 / MIB)),
        ),
    ];

    for (index, name) in recording.groups.iter().enumerate() {
        out.push(Series::new(
            SeriesKind::GroupCpu,
            Unit::Percent,
            name,
            name.clone(),
            per_tick(&|t| group_at(t, index).map(|g| t.cpu_percent_of_core(g.total_cpu_time()))),
        ));
        out.push(Series::new(
            SeriesKind::GroupMemory,
            Unit::Mib,
            &format!("{name}.rss"),
            name.clone(),
            per_tick(&|t| group_at(t, index).filter(|g| g.processes > 0).map(|g| g.rss_bytes as f64 / MIB)),
        ));
        out.push(Series::new(
            SeriesKind::GroupProcesses,
            Unit::Count,
            &format!("{name}.procs"),
            name.clone(),
            per_tick(&|t| group_at(t, index).map(|g| g.processes as f64)),
        ));
    }

    if options.processes {
        for (key, process) in &recording.processes {
            let name = format!("{}.{}", process.info.name, key.pid);
            out.push(Series::new(
                SeriesKind::ProcessCpu,
                Unit::Percent,
                &name,
                name.clone(),
                per_tick(&|t| t.processes.get(key).map(|p| t.cpu_percent_of_core(p.cpu_time))),
            ));
            out.push(Series::new(
                SeriesKind::ProcessMemory,
                Unit::Mib,
                &format!("{name}.rss"),
                name.clone(),
                per_tick(&|t| t.processes.get(key).map(|p| p.rss_bytes as f64 / MIB)),
            ));
        }
    }

    #[cfg(feature = "disk")]
    for (index, disk) in recording.sources.disks.iter().enumerate() {
        let label = disk.filesystem.mount_point.clone();
        let name = format!("disk.{}", if label == "/" { "root" } else { &label });
        let at = |t: &Tick| t.disks.get(index).copied();
        out.push(Series::new(
            SeriesKind::DiskBusy,
            Unit::Percent,
            &format!("{name}.busy"),
            label.clone(),
            per_tick(&|t| at(t)?.io.map(|io| io.busy_percent)),
        ));
        out.push(Series::new(
            SeriesKind::DiskIo,
            Unit::MibPerSec,
            &format!("{name}.read"),
            format!("{label} read"),
            per_tick(&|t| at(t)?.io.map(|io| io.read_bytes_per_sec / MIB)),
        ));
        out.push(Series::new(
            SeriesKind::DiskIo,
            Unit::MibPerSec,
            &format!("{name}.write"),
            format!("{label} write"),
            per_tick(&|t| at(t)?.io.map(|io| io.write_bytes_per_sec / MIB)),
        ));
        out.push(Series::new(
            SeriesKind::DiskUsed,
            Unit::Gib,
            &format!("{name}.used"),
            label.clone(),
            per_tick(&|t| at(t)?.space.map(|s| s.used_bytes() as f64 / GIB)),
        ));
    }

    #[cfg(feature = "network")]
    for (index, interface) in recording.sources.interfaces.iter().enumerate() {
        let name = format!("net.{}", interface.name);
        let at = |t: &Tick| t.networks.get(index).and_then(|n| n.rates);
        out.push(Series::new(
            SeriesKind::Network,
            Unit::MibPerSec,
            &format!("{name}.rx"),
            format!("{} rx", interface.name),
            per_tick(&|t| at(t).map(|r| r.rx_bytes_per_sec / MIB)),
        ));
        out.push(Series::new(
            SeriesKind::Network,
            Unit::MibPerSec,
            &format!("{name}.tx"),
            format!("{} tx", interface.name),
            per_tick(&|t| at(t).map(|r| r.tx_bytes_per_sec / MIB)),
        ));
    }

    for (index, gpu) in recording.sources.gpus.iter().enumerate() {
        let name = format!("gpu{index}");
        let at = |t: &Tick| t.gpus.get(index).copied().flatten();
        out.push(Series::new(
            SeriesKind::GpuUtilization,
            Unit::Percent,
            &format!("{name}.util"),
            gpu.name.clone(),
            per_tick(&|t| at(t)?.utilization_percent.map(f64::from)),
        ));
        out.push(Series::new(
            SeriesKind::GpuMemory,
            Unit::Mib,
            &format!("{name}.vram"),
            gpu.name.clone(),
            per_tick(&|t| at(t)?.memory_used_bytes.map(|b| b as f64 / MIB)),
        ));
        out.push(Series::new(
            SeriesKind::GpuTemperature,
            Unit::Celsius,
            &format!("{name}.temp"),
            gpu.name.clone(),
            per_tick(&|t| at(t)?.temperature_celsius.map(f64::from)),
        ));
    }

    out.retain(Series::has_values);
    out
}

fn group_at(tick: &Tick, index: usize) -> Option<&GroupUsage> {
    tick.groups.get(index)
}
