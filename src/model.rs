use serde::Deserialize;

use crate::cli::Cli;
use crate::enums::{AppMode, DataCollectionMode, LogLev};

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

impl SingleItemModel {
    pub fn to_csv_string(&self) -> String {
        format!(
            "{},{},{},{},{},{},{}",
            self.unix_timestamp,
            self.memory_used,
            self.memory_available,
            self.memory_free,
            self.memory_total,
            self.cpu_total,
            self.cpu_usage_per_core
        )
    }
}

#[derive(Default, Clone, Debug, Deserialize)]
pub struct CollectedItemModels {
    pub unix_timestamp: Vec<f64>,
    pub memory_used: Vec<u64>,
    pub memory_available: Vec<u64>,
    pub memory_free: Vec<u64>,
    pub memory_total: Vec<u64>,
    pub cpu_core_usage: Vec<Vec<f32>>,
    pub cpu_total: Vec<f32>,
}

impl CollectedItemModels {
    pub fn new_with_reserved_space(space: usize) -> Self {
        CollectedItemModels {
            unix_timestamp: Vec::with_capacity(space),
            memory_used: Vec::with_capacity(space),
            memory_available: Vec::with_capacity(space),
            memory_free: Vec::with_capacity(space),
            memory_total: Vec::with_capacity(space),
            cpu_core_usage: Vec::with_capacity(space),
            cpu_total: Vec::with_capacity(space),
        }
    }
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
