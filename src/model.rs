use serde::Deserialize;
use std::collections::HashMap;

use crate::cli::Cli;
use crate::enums::{AllDataCollectionMode, AppMode, DataCollectionMode, GeneralInfoGroup, LogLev};

#[derive(Clone, Debug, Deserialize)]
pub struct SingleItemModel {
    pub unix_timestamp: f64,
    pub memory_used: u64,
    pub memory_available: u64,
    pub memory_free: u64,
    pub memory_total: u64,
    pub cpu_usage_per_core: String,
    pub cpu_total: f32,
}

#[derive(Default, Clone, Debug, Deserialize)]
pub struct CollectedItemModels {
    pub collected_data_names: Vec<AllDataCollectionMode>,
    pub collected_data: Vec<Vec<String>>,
    pub collected_groups: Vec<GeneralInfoGroup>,
    pub memory_total: u64,
    pub cpu_core_count: u64,
    pub check_interval: f32,
}

#[derive(Default, Clone, Debug)]
pub struct Settings {
    pub check_interval: f32,
    pub data_path: String,
    pub plot_path: String,
    pub app_mode: AppMode,
    pub collection_mode: Vec<DataCollectionMode>,
    pub plot_width: u32,
    pub plot_height: u32,
    pub white_plot_mode: bool,
    pub log_level: LogLev,
    pub open_plot_file: bool,
    //
    pub memory_total: usize,
    pub cpu_core_count: usize,
}

impl From<Cli> for Settings {
    fn from(cli: Cli) -> Self {
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
            memory_total: 0,
            cpu_core_count: 0,
        }
    }
}
