use std::process;
use std::time::SystemTime;

use crate::cli::{CollectArgs, ConvertArgs};

#[derive(Default, Clone, Debug)]
pub struct FindingStruct {
    pub graph_name: String,
    pub search_text: String,
}

#[derive(Default, Clone, Debug)]
pub struct ConvertSettings {
    pub data_path: String,
    pub plot_path: String,
    pub plot_width: u32,
    pub plot_height: u32,
    pub white_plot_mode: bool,
    pub open_plot_file: bool,
}

// Trait for process settings access
pub trait ProcessSettings {
    fn process_cmd_to_search(&self) -> &Vec<FindingStruct>;
}

#[derive(Default, Clone, Debug)]
pub struct CollectSettings {
    pub check_interval: f32,
    pub convert: ConvertSettings,
    pub collection_mode: Vec<crate::enums::SimpleDataCollectionMode>,
    pub disable_instant_flushing: bool,
    pub backup_number: u32,
    pub maximum_data_file_size_bytes: usize,
    pub process_cmd_to_search: Vec<FindingStruct>,
    pub need_to_refresh_processes: bool,
    pub start_time: f64,
    pub convert_after: bool,
    // Server options
    pub serve: bool,
    pub port: u16,
    pub max_results: usize,
}

impl ProcessSettings for CollectSettings {
    fn process_cmd_to_search(&self) -> &Vec<FindingStruct> {
        &self.process_cmd_to_search
    }
}

impl From<CollectArgs> for CollectSettings {
    fn from(cli: CollectArgs) -> Self {
        let process_to_search: Vec<_> = cli
            .process_cmd_to_search
            .iter()
            .map(|e| {
                if e.contains('=') || e.contains(',') {
                    log::error!("{e} - cannot use here = or ,");
                    process::exit(1);
                }
                let split = e.split('|').collect::<Vec<_>>();
                if split.len() != 2 {
                    log::error!("{e} - should contains two parts split by |");
                    process::exit(1);
                }
                FindingStruct {
                    graph_name: split[0].to_string(),
                    search_text: split[1].to_string(),
                }
            })
            .collect();

        let convert_settings = ConvertSettings {
            data_path: cli.common.data_path,
            plot_path: cli.common.plot_path,
            plot_width: cli.common.plot_width,
            plot_height: cli.common.plot_height,
            white_plot_mode: cli.common.white_plot_mode,
            open_plot_file: cli.common.open_plot_file,
        };

        if cli.common.open_plot_file && !cli.convert_after {
            log::error!("Cannot use --open-plot-file without --convert-after");
            process::exit(1);
        }

        CollectSettings {
            check_interval: cli.check_interval,
            convert: convert_settings,
            collection_mode: cli.collection_mode,
            disable_instant_flushing: cli.disable_instant_flushing,
            backup_number: cli.backup_number,
            maximum_data_file_size_bytes: (cli.maximum_data_file_size_mb * 1024.0 * 1024.0) as usize,
            need_to_refresh_processes: !process_to_search.is_empty(),
            process_cmd_to_search: process_to_search,
            start_time: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("Cannot fail duration since UNIX_EPOCH")
                .as_secs_f64(),
            convert_after: cli.convert_after,
            // Server options
            serve: cli.serve,
            port: cli.port,
            max_results: cli.max_results.clamp(1, 1000),
        }
    }
}

impl From<ConvertArgs> for ConvertSettings {
    fn from(cli: ConvertArgs) -> Self {
        ConvertSettings {
            data_path: cli.common.data_path,
            plot_path: cli.common.plot_path,
            plot_width: cli.common.plot_width,
            plot_height: cli.common.plot_height,
            white_plot_mode: cli.common.white_plot_mode,
            open_plot_file: cli.common.open_plot_file,
        }
    }
}
