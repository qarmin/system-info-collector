use std::collections::{HashMap, HashSet};
use std::process;

use serde::Deserialize;
use sysinfo::{Process, ProcessExt};

use crate::cli::Cli;
use crate::enums::{AppMode, DataType, GeneralInfoGroup, LogLev, SimpleDataCollectionMode};

#[derive(Default, Clone, Debug, Deserialize)]
pub struct CollectedItemModels {
    pub collected_data_names: Vec<DataType>,
    pub collected_data: HashMap<DataType, Vec<String>>,
    pub collected_groups: Vec<GeneralInfoGroup>,
    pub memory_total: f64,
    pub cpu_core_count: usize,
    pub check_interval: f32,
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
            name: process.name().to_string(),
            cmd_string: process.cmd().join(" "),
            memory_usage: process.memory(),
            cpu_usage: process.cpu_usage(),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ProcessCache {
    pub processes_checked: HashSet<usize>,
    pub process_used: Vec<Option<CustomProcessData>>,
}
impl ProcessCache {
    pub fn new_with_size(size: usize) -> Self {
        let mut process_used = vec![];
        for _ in 0..size {
            process_used.push(None);
        }

        // Do not allow to check current process, because cmd values will always be valid for it
        let mut processes_checked = HashSet::default();
        processes_checked.insert(process::id() as usize);

        ProcessCache {
            processes_checked,
            process_used,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct SingleProcessCacheStruct {
    pub collected_name: String,
    pub need_to_check: bool,
}

#[derive(Default, Clone, Debug)]
pub struct FindingStruct {
    pub graph_name: String,
    pub search_text: String,
}

#[derive(Default, Clone, Debug)]
pub struct Settings {
    pub check_interval: f32,
    pub data_path: String,
    pub plot_path: String,
    pub app_mode: AppMode,
    pub collection_mode: Vec<SimpleDataCollectionMode>,
    pub plot_width: u32,
    pub plot_height: u32,
    pub white_plot_mode: bool,
    pub log_level: LogLev,
    pub open_plot_file: bool,
    pub disable_instant_flushing: bool,
    pub use_web_gl: bool,
    pub backup_number: u32,
    pub maximum_data_file_size_bytes: usize,
    pub process_cmd_to_search: Vec<FindingStruct>,
    pub need_to_refresh_processes: bool,
}

impl From<Cli> for Settings {
    fn from(cli: Cli) -> Self {
        let process_to_search: Vec<_> = cli
            .process_cmd_to_search
            .iter()
            .map(|e| {
                if e.contains('=') || e.contains(',') {
                    eprintln!("{e} - cannot use here = or ,");
                    process::exit(1);
                }
                let split = e.split('|').collect::<Vec<_>>();
                if split.len() != 2 {
                    eprintln!("{e} - should contains two parts split by |");
                    process::exit(1);
                }
                FindingStruct {
                    graph_name: split[0].to_string(),
                    search_text: split[1].to_string(),
                }
            })
            .collect();

        Settings {
            check_interval: cli.check_interval,
            data_path: cli.data_path,
            plot_path: cli.plot_path,
            app_mode: cli.app_mode,
            collection_mode: cli.collection_mode,
            plot_width: cli.plot_width,
            plot_height: cli.plot_height,
            white_plot_mode: cli.white_plot_mode,
            log_level: cli.log_level,
            open_plot_file: cli.open_plot_file,
            disable_instant_flushing: cli.disable_instant_flushing,
            use_web_gl: true, // TODO: add this to CLI - need to check if this works
            backup_number: cli.backup_number,
            maximum_data_file_size_bytes: (cli.maximum_data_file_size_mb * 1024.0 * 1024.0) as usize,
            need_to_refresh_processes: !process_to_search.is_empty(),
            process_cmd_to_search: process_to_search,
        }
    }
}
