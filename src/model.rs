use std::collections::HashMap;

use serde::Deserialize;

use crate::cli::Cli;
use crate::enums::{AppMode, DataCollectionMode, DataType, GeneralInfoGroup, LogLev};

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
    pub disable_instant_flushing: bool,
    pub use_web_gl: bool,
    pub backup_number: u32,
    pub maximum_data_file_size_bytes: usize,
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
            disable_instant_flushing: cli.disable_instant_flushing,
            use_web_gl: true, // TODO: add this to CLI - need to check if this works
            backup_number: cli.backup_number,
            maximum_data_file_size_bytes: (cli.maximum_data_file_size_mb * 1024.0 * 1024.0) as usize,
        }
    }
}
