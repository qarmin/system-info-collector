use clap::{Parser, Subcommand};

use crate::enums::SimpleDataCollectionMode;

#[derive(Debug, clap::Args, Clone)]
pub struct CommonCliItems {
    #[arg(
        short,
        long,
        default_value = "system_data.csv",
        value_name = "DATA_PATH",
        help = "Path to data file collected by this app or to convert."
    )]
    pub data_path: String,

    #[arg(
        short,
        long,
        default_value = "system_data_plot.html",
        value_name = "HTML_PLOT_PATH",
        help = "Path where html file with plot will be saved."
    )]
    pub plot_path: String,

    #[arg(short = 'w', long, default_value = "1700", value_name = "WIDTH", help = "Width of generated plot.")]
    pub plot_width: u32,

    #[arg(short = 'r', long, default_value = "800", value_name = "HEIGHT", help = "Height of generated plot.")]
    pub plot_height: u32,

    #[arg(short = 'z', long, default_value = "false", value_name = "WHITE_PLOT_MODE", help = "White plot mode.")]
    pub white_plot_mode: bool,

    #[arg(
        short,
        long,
        default_value = "false",
        value_name = "OPEN_PLOT_FILE",
        help = "Open generated plot file in default html viewer"
    )]
    pub open_plot_file: bool,
}

#[derive(Parser, Debug)]
#[command(name = "System Info Collector")]
#[command(author = "Rafał Mikrut")]
#[command(version = "0.1")]
#[command(about = "App to collect info about system", long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Collect system info. Use --convert-after to also convert/plot after collecting
    Collect(CollectArgs),
    /// Convert existing data file into plot
    Convert(ConvertArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct CollectArgs {
    #[arg(
        short,
        long,
        default_value = "1.0",
        value_name = "INTERVAL",
        help = "Interval of checking cpu/memory usage in seconds, minimum value is 0.25 second(sysinfo library contains hard limit 200ms)."
    )]
    pub check_interval: f32,

    #[command(flatten)]
    pub common: CommonCliItems,

    #[arg(
        short = 'm',
        long,
        num_args = 1..,
        default_values = & ["cpu-usage-total", "memory-used"],
        value_name = "DATA_TYPE",
        help = "List data"
    )]
    pub collection_mode: Vec<SimpleDataCollectionMode>,

    #[arg(
        short = 'i',
        long,
        default_value = "false",
        value_name = "INSTANT_FLUSHING",
        help = "Disables automatic flushing when writing to the file. This may cause data loss in case of a system crash. Use with caution."
    )]
    pub disable_instant_flushing: bool,

    #[arg(
        short,
        long,
        default_value = "5",
        value_name = "BACKUP_NUMBER",
        help = "Number of file backups to keep. Backup files will be named data__1.csv, data__2.csv, etc. if the original file is named data.csv."
    )]
    pub backup_number: u32,

    #[arg(
        short = 'k',
        long,
        default_value = "100.0",
        value_name = "MAXIMUM_FILE_SIZE_MB",
        help = "Maximum file size of created data, to help prevent using too much disk space."
    )]
    pub maximum_data_file_size_mb: f32,

    #[arg(
        short = 'e',
        long,
        value_name = "CMD_SEARCH_TEXT",
        help = "Search for certain text in process run command"
    )]
    pub process_cmd_to_search: Vec<String>,

    #[arg(long, help = "Also convert/plot after collecting is finished")]
    pub convert_after: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct ConvertArgs {
    #[command(flatten)]
    pub common: CommonCliItems,
}

pub(crate) fn parse_cli() -> Args {
    Args::parse()
}
