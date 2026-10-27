use std::time::Instant;

use crate::error::ProbeResult;

/// Busy and idle clock ticks of one CPU line of `/proc/stat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CoreTicks {
    pub busy: u64,
    pub idle: u64,
}

impl CoreTicks {
    /// `user nice system idle iowait irq softirq steal guest guest_nice`; guest time is already
    /// included in user/nice, and iowait counts as idle like `top` does.
    #[cfg(target_os = "linux")]
    pub(crate) fn from_proc_stat_fields(values: &[u64]) -> Option<Self> {
        if values.len() < 4 {
            return None;
        }
        let at = |i: usize| values.get(i).copied().unwrap_or(0);
        Some(Self {
            busy: at(0) + at(1) + at(2) + at(5) + at(6) + at(7),
            idle: at(3) + at(4),
        })
    }

    fn busy_percent_since(self, earlier: Self) -> f32 {
        let busy = self.busy.saturating_sub(earlier.busy);
        let total = busy + self.idle.saturating_sub(earlier.idle);
        if total == 0 {
            return 0.0;
        }
        (busy as f64 * 100.0 / total as f64) as f32
    }
}

/// Cumulative CPU ticks, machine-wide and per logical core.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CpuTimes {
    pub total: CoreTicks,
    pub cores: Vec<CoreTicks>,
}

impl CpuTimes {
    pub fn usage_since(&self, earlier: &Self) -> CpuUsage {
        CpuUsage {
            total_percent: self.total.busy_percent_since(earlier.total),
            per_core_percent: self
                .cores
                .iter()
                .zip(&earlier.cores)
                .map(|(now, before)| now.busy_percent_since(*before))
                .collect(),
        }
    }
}

/// Share of time the CPUs were busy over an interval, 0-100 each.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CpuUsage {
    pub total_percent: f32,
    pub per_core_percent: Vec<f32>,
}

/// Where cumulative ticks are unavailable the platform hands out percentages already computed
/// over its own window.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CpuReading {
    Ticks(CpuTimes),
    Usage(CpuUsage),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub available_bytes: u64,
    /// Total minus available - what `free` and task managers call used.
    pub used_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
}

impl MemoryInfo {
    pub fn swap_used_bytes(&self) -> u64 {
        self.swap_total_bytes.saturating_sub(self.swap_free_bytes)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SystemSample {
    pub taken_at: Instant,
    pub cpu: CpuReading,
    pub memory: MemoryInfo,
}

impl SystemSample {
    pub fn logical_cores(&self) -> usize {
        match &self.cpu {
            CpuReading::Ticks(times) => times.cores.len().max(1),
            CpuReading::Usage(usage) => usage.per_core_percent.len().max(1),
        }
    }

    /// `None` only when `earlier` came from a different kind of source.
    pub fn cpu_usage_since(&self, earlier: &Self) -> Option<CpuUsage> {
        match (&self.cpu, &earlier.cpu) {
            (CpuReading::Ticks(now), CpuReading::Ticks(before)) => Some(now.usage_since(before)),
            (CpuReading::Usage(now), CpuReading::Usage(_)) => Some(now.clone()),
            _ => None,
        }
    }
}

/// Static description of the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MachineInfo {
    pub hostname: Option<String>,
    /// Distribution and release, e.g. `Ubuntu 24.04`.
    pub os_name: Option<String>,
    pub kernel_version: Option<String>,
    pub cpu_model: Option<String>,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
    pub total_memory_bytes: u64,
    pub total_swap_bytes: u64,
}

impl MachineInfo {
    pub fn read() -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        Self {
            hostname: sysinfo::System::host_name(),
            // `long_os_version` spells this "Linux (Ubuntu 24.04)", which reads oddly in a panel.
            os_name: match (sysinfo::System::name(), sysinfo::System::os_version()) {
                (Some(name), Some(version)) => Some(format!("{name} {version}")),
                (Some(name), None) => Some(name),
                _ => sysinfo::System::long_os_version(),
            },
            kernel_version: sysinfo::System::kernel_version(),
            cpu_model: sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty()),
            logical_cores: sys.cpus().len().max(1),
            physical_cores: sysinfo::System::physical_core_count(),
            total_memory_bytes: sys.total_memory(),
            total_swap_bytes: sys.total_swap(),
        }
    }
}

/// Machine-wide CPU ticks and memory.
pub struct SystemSampler {
    #[cfg(target_os = "linux")]
    proc_fs: crate::backend::procfs::ProcFs,
    #[cfg(not(target_os = "linux"))]
    sys: sysinfo::System,
}

impl SystemSampler {
    pub fn new() -> ProbeResult<Self> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                proc_fs: crate::backend::procfs::ProcFs::system()?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut sys = sysinfo::System::new();
            sys.refresh_cpu_usage();
            Ok(Self { sys })
        }
    }

    #[cfg(target_os = "linux")]
    pub fn with_proc_fs(proc_fs: crate::backend::procfs::ProcFs) -> Self {
        Self { proc_fs }
    }

    #[cfg(target_os = "linux")]
    pub fn sample(&mut self) -> ProbeResult<SystemSample> {
        let taken_at = Instant::now();
        Ok(SystemSample {
            taken_at,
            cpu: CpuReading::Ticks(self.proc_fs.cpu_times()?),
            memory: self.proc_fs.memory()?,
        })
    }

    /// sysinfo rate-limits CPU refreshes to `MINIMUM_CPU_UPDATE_INTERVAL`, so sampling faster
    /// than that repeats the previous percentages.
    #[cfg(not(target_os = "linux"))]
    pub fn sample(&mut self) -> ProbeResult<SystemSample> {
        let taken_at = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        let usage = CpuUsage {
            total_percent: self.sys.global_cpu_usage(),
            per_core_percent: self.sys.cpus().iter().map(sysinfo::Cpu::cpu_usage).collect(),
        };
        let total = self.sys.total_memory();
        let available = self.sys.available_memory();
        Ok(SystemSample {
            taken_at,
            cpu: CpuReading::Usage(usage),
            memory: MemoryInfo {
                total_bytes: total,
                free_bytes: self.sys.free_memory(),
                available_bytes: available,
                used_bytes: self.sys.used_memory(),
                swap_total_bytes: self.sys.total_swap(),
                swap_free_bytes: self.sys.free_swap(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_busy_share_per_core() {
        let before = CpuTimes {
            total: CoreTicks { busy: 100, idle: 100 },
            cores: vec![CoreTicks { busy: 50, idle: 50 }, CoreTicks { busy: 50, idle: 50 }],
        };
        let after = CpuTimes {
            total: CoreTicks { busy: 175, idle: 125 },
            cores: vec![CoreTicks { busy: 100, idle: 50 }, CoreTicks { busy: 75, idle: 75 }],
        };
        let usage = after.usage_since(&before);
        assert!((usage.total_percent - 75.0).abs() < 0.01);
        assert!((usage.per_core_percent[0] - 100.0).abs() < 0.01);
        assert!((usage.per_core_percent[1] - 50.0).abs() < 0.01);
    }

    #[test]
    fn idle_interval_without_ticks_is_zero_not_nan() {
        let times = CpuTimes {
            total: CoreTicks { busy: 5, idle: 5 },
            cores: vec![],
        };
        assert!(times.usage_since(&times).total_percent.abs() < f32::EPSILON);
    }
}
