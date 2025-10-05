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
        CustomProcessData {
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
        // Do not allow to check current process, because cmd values will always be valid for it
        let mut processes_checked_to_be_used = HashSet::default();
        processes_checked_to_be_used.insert(process::id() as usize);

        let mut processes_usage_updated = sys.processes().keys().map(|pid| (*pid).into()).collect::<HashSet<usize>>();
        processes_usage_updated.insert(process::id() as usize);

        ProcessCache {
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
