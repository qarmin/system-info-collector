use std::collections::HashMap;
use std::process;

use serde::Deserialize;

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

#[derive(Default, Clone, Debug)]
pub struct ProcessCache {
    pub process_used: HashMap<usize, SingleProcessCacheStruct>,
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
