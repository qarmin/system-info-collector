use crate::enums::{DataType, GeneralInfoGroup};
use serde::Deserialize;
use std::collections::hash_set::Iter;
use std::collections::{HashMap, HashSet};
use std::process;
use sysinfo::{Process, System};

#[derive(Default, Clone, Debug, Deserialize)]
pub struct CollectedItemModels {
    pub collected_data: HashMap<DataType, Vec<String>>,
    pub collected_groups: Vec<GeneralInfoGroup>,
    pub memory_total: f64,
    pub swap_total: f64,
    pub cpu_core_count: usize,
    pub check_interval: f32,
    pub start_time: f64,
    /// CPU model string from CSV metadata (empty on old files).
    pub cpu_model: String,
    /// GPU names parsed from CSV metadata (GPU_0=name, GPU_1=name, …).
    pub gpu_names: Vec<String>,
    /// Total VRAM per GPU in MB, parallel to `gpu_names` (0 if unknown).
    pub gpu_vram_mb: Vec<u64>,
    /// Top N processes by CPU%, loaded from an optional extra file.
    pub top_cpu_processes: Option<TopProcessData>,
    /// Top N processes by RAM (MB), loaded from an optional extra file.
    pub top_ram_processes: Option<TopProcessData>,
    /// Disk mount points parsed from CSV metadata (DISK_0=path, …).
    pub disk_names: Vec<String>,
}

/// Data loaded from a top-N-processes file (CPU or RAM).
#[derive(Default, Clone, Debug, Deserialize)]
pub struct TopProcessData {
    pub n: usize,
    pub start_time: f64,
    /// Timestamps (seconds since start_time) for each row.
    pub timestamps: Vec<f64>,
    /// `ranks[rank_index][row_index]` = `Some((process_name, value))`.
    /// `None` when fewer than N processes were running at that moment.
    pub ranks: Vec<Vec<Option<(String, f64)>>>,
}

#[derive(Default, Debug, Clone)]
pub struct CustomProcessData {
    pub pid: usize,
    pub name: String,
    pub cmd_string: String,
    pub memory_usage: u64,
    pub cpu_usage: f32,
}

impl CustomProcessData {
    pub fn from_process(process: &Process) -> Self {
        Self {
            pid: process.pid().into(),
            name: process.name().to_string_lossy().to_string(),
            cmd_string: process
                .cmd()
                .iter()
                .map(|e| e.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" "),
            memory_usage: process.memory(),
            cpu_usage: process.cpu_usage(),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ProcessCache {
    pub processes_usage_updated: HashSet<usize>,
    pub processes_checked_to_be_used: HashSet<usize>,
    pub process_used: Vec<Option<CustomProcessData>>,
}

impl ProcessCache {
    pub fn new_with_size(size: usize, sys: &System) -> Self {
        let mut processes_checked_to_be_used = HashSet::default();
        processes_checked_to_be_used.insert(process::id() as usize);

        let mut processes_usage_updated = sys.processes().keys().map(|pid| (*pid).into()).collect::<HashSet<usize>>();
        processes_usage_updated.insert(process::id() as usize);

        Self {
            processes_usage_updated,
            processes_checked_to_be_used,
            process_used: vec![None; size],
        }
    }

    pub fn get_differences_in_usage_processes(&self, elements: Iter<usize>) -> Vec<usize> {
        let mut result = vec![];
        for element in elements {
            if !self.processes_usage_updated.contains(element) {
                result.push(*element);
            }
        }
        result
    }

    pub fn replace_checked_usage_processes(&mut self, elements: Iter<usize>) {
        self.processes_usage_updated = elements.copied().collect::<HashSet<usize>>();
        self.processes_usage_updated.insert(process::id() as usize);
    }

    pub fn replace_checked_to_be_used_processes(&mut self, elements: Iter<usize>) {
        self.processes_checked_to_be_used = elements.copied().collect::<HashSet<usize>>();
        self.processes_checked_to_be_used.insert(process::id() as usize);
    }
}
