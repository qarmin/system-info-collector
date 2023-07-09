use clap::Parser;

use crate::enums::{AppMode, DataCollectionMode, LogLev};

#[derive(Parser, Debug)]
#[command(name = "System Info Collector")]
#[command(author = "Rafał Mikrut")]
#[command(version = "0.1")]
#[command(about = "App to collect info about system", long_about = None)]
pub struct Cli {
    #[arg(
        short,
        long,
        default_value = "1.0",
        value_name = "INTERVAL",
        help = "Interval of checking cpu/memory usage in seconds, using smaller value than 1.0 may not work correctly"
    )]
    pub check_interval: f32,

    #[arg(
        short,
        long,
        default_value = "system_data.csv",
        value_name = "DATA_PATH",
        help = "Path to data file collected by this app, if mode is set to Convert, then this file must exists, in other modes it will be created."
    )]
    pub data_path: String,

    #[arg(
        short,
        long,
        default_value = "system_data_plot.html",
        value_name = "HTML_PLOT_PATH",
        help = "Path where html file with plot will be saved. Only useful for Convert/CollectAndConvert mode."
    )]
    pub plot_path: String,

    #[arg(
        short,
        long,
        default_value = "collect",
        value_name = "APP_MODE",
        help = "Collect will collect system data, Convert will convert."
    )]
    pub app_mode: AppMode,

    #[arg(
        short = 'm',
        long,
        num_args = 1..,
        default_values = & ["cpu-usage-total", "cpu-usage-per-core", "memory-used"],
        value_name = "DATA_TYPE",
        help = "List data"
    )]
    pub collection_mode: Vec<DataCollectionMode>,

    #[arg(short = 'w', long, default_value = "1700", value_name = "WIDTH", help = "Width of generated plot.")]
    pub plot_width: u32,

    #[arg(short = 'r', long, default_value = "800", value_name = "HEIGHT", help = "Height of generated plot.")]
    pub plot_height: u32,

    #[arg(short = 'z', long, default_value = "false", value_name = "WHITE_PLOT_MODE", help = "White plot mode.")]
    pub white_plot_mode: bool,

    #[arg(short, long, default_value = "info", value_name = "Info", help = "Logging level")]
    pub log_level: LogLev,

    #[arg(
        short,
        long,
        default_value = "false",
        value_name = "OPEN_PLOT_FILE",
        help = "Open generated plot file in default html viewer"
    )]
    pub open_plot_file: bool,
}

pub(crate) fn parse_cli() -> Cli {
    Cli::parse()
}
