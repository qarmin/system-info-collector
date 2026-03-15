use std::process;
use std::time::SystemTime;

use system_info_collector_core::settings::{CollectSettings, ConvertSettings, FindingStruct};

use crate::cli::{CollectArgs, ConvertArgs};

pub fn build_convert_settings(args: ConvertArgs) -> ConvertSettings {
    ConvertSettings {
        data_path: args.common.data_path,
        plot_path: args.common.plot_path,
        plot_width: args.common.plot_width,
        plot_height: args.common.plot_height,
        white_plot_mode: args.common.white_plot_mode,
        open_plot_file: args.common.open_plot_file,
    }
}

pub fn build_collect_settings(args: CollectArgs) -> CollectSettings {
    let process_to_search: Vec<FindingStruct> = args
        .process_cmd_to_search
        .iter()
        .map(|e| {
            if e.contains('=') || e.contains(',') {
                log::error!("{e} — cannot use '=' or ',' in process search specification");
                process::exit(1);
            }
            let parts = e.split('|').collect::<Vec<_>>();
            if parts.len() != 2 {
                log::error!("{e} — must have format NAME|SEARCH_TEXT");
                process::exit(1);
            }
            FindingStruct {
                graph_name: parts[0].to_string(),
                search_text: parts[1].to_string(),
            }
        })
        .collect();

    let convert_settings = ConvertSettings {
        data_path: args.common.data_path,
        plot_path: args.common.plot_path,
        plot_width: args.common.plot_width,
        plot_height: args.common.plot_height,
        white_plot_mode: args.common.white_plot_mode,
        open_plot_file: args.common.open_plot_file,
    };

    if args.common.open_plot_file && !args.convert_after {
        log::error!("Cannot use --open-plot-file without --convert-after");
        process::exit(1);
    }

    let check_interval = args.check_interval.max(0.1);
    let sysinfo_interval = args.sysinfo_interval.unwrap_or(check_interval).max(0.05);

    CollectSettings {
        check_interval,
        sysinfo_interval_secs: sysinfo_interval,
        network_interval_secs: args.network_interval.max(0.05),
        gpu_interval_secs: args.gpu_interval.max(0.05),
        convert: convert_settings,
        collection_mode: args.collection_mode,
        disable_instant_flushing: args.disable_instant_flushing,
        backup_number: args.backup_number,
        maximum_data_file_size_bytes: (args.maximum_data_file_size_mb * 1024.0 * 1024.0) as usize,
        need_to_refresh_processes: !process_to_search.is_empty(),
        process_cmd_to_search: process_to_search,
        start_time: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("time before UNIX_EPOCH")
            .as_secs_f64(),
        convert_after: args.convert_after,
        serve: args.serve,
        port: args.port,
        max_results: args.max_results.clamp(1, 100_000),
    }
}
