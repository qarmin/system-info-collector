use log::error;
use std::process;
use std::time::SystemTime;

use crate::cli::{CollectArgs, ConvertArgs};
use crate::enums::LogLev;

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
    pub log_level: LogLev,
    pub open_plot_file: bool,
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
}

impl From<CollectArgs> for CollectSettings {
    fn from(cli: CollectArgs) -> Self {
        let process_to_search: Vec<_> = cli
            .process_cmd_to_search
            .iter()
            .map(|e| {
                if e.contains('=') || e.contains(',') {
                    error!("{e} - cannot use here = or ,");
                    process::exit(1);
                }
                let split = e.split('|').collect::<Vec<_>>();
                if split.len() != 2 {
                    error!("{e} - should contains two parts split by |");
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
            log_level: cli.common.log_level,
            open_plot_file: cli.common.open_plot_file,
        };

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
            log_level: cli.common.log_level,
            open_plot_file: cli.common.open_plot_file,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_settings_from_args_minimal() {
        let cli = crate::cli::CollectArgs {
            check_interval: 1.0,
            common: crate::cli::CommonCliItems {
                data_path: "data.csv".to_string(),
                plot_path: "plot.html".to_string(),
                plot_width: 800,
                plot_height: 600,
                white_plot_mode: false,
                open_plot_file: false,
                log_level: crate::enums::LogLev::Info,
            },
            collection_mode: vec![crate::enums::SimpleDataCollectionMode::CPU_USAGE_TOTAL],
            disable_instant_flushing: false,
            backup_number: 0,
            maximum_data_file_size_mb: 10.0,
            process_cmd_to_search: vec![],
            convert_after: false,
        };
        let settings: CollectSettings = cli.into();
        assert_eq!(settings.convert.data_path, "data.csv");
        assert_eq!(settings.collection_mode.len(), 1);
    }
}
