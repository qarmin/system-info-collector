use clap::{Parser, Subcommand};
use system_info_collector_core::enums::SimpleDataCollectionMode;

/// Plot-related settings shared between Collect and Convert commands.
#[derive(Debug, clap::Args, Clone)]
pub struct PlotArgs {
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

    #[arg(
        short = 'r',
        long,
        default_value = "800",
        value_name = "HEIGHT",
        help = "Minimum height of generated plot (auto-scales with number of charts)."
    )]
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
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "App to collect info about system", long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Collect(CollectArgs),
    Convert(ConvertArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct CollectArgs {
    #[arg(
        short,
        long,
        default_value = "1.0",
        value_name = "INTERVAL",
        help = "Main collection interval in seconds — how often a CSV row is written (minimum 0.1 s)."
    )]
    pub check_interval: f32,

    #[arg(
        short,
        long,
        default_value = "system_data.csv",
        value_name = "DATA_PATH",
        help = "Path to the output data file."
    )]
    pub data_path: String,

    #[command(flatten)]
    pub plot: PlotArgs,

    #[arg(
        short = 'm',
        long,
        num_args = 1..,
        default_values = &["cpu-usage-total", "memory-used"],
        value_name = "DATA_TYPE",
        help = "Metrics to collect."
    )]
    pub collection_mode: Vec<SimpleDataCollectionMode>,

    #[arg(
        short = 'i',
        long,
        default_value = "false",
        value_name = "INSTANT_FLUSHING",
        help = "Disable automatic file flushing after each write - may improve performance a little, but increases risk of data loss on crash."
    )]
    pub disable_instant_flushing: bool,

    #[arg(
        long,
        default_value = "false",
        help = "Disable compact CSV mode. By default, repeated values are omitted (written as empty) to reduce file size; the reader fills them back in automatically."
    )]
    pub no_compact: bool,

    #[arg(
        short,
        long,
        default_value = "5",
        value_name = "BACKUP_NUMBER",
        help = "Number of rotating backup files to keep."
    )]
    pub backup_number: u32,

    #[arg(
        short = 'k',
        long,
        default_value = "200.0",
        value_name = "MAXIMUM_FILE_SIZE_MB",
        help = "Maximum data-file size in MB before collection stops."
    )]
    pub maximum_data_file_size_mb: f32,

    #[arg(
        short = 'e',
        long,
        value_name = "NAME|SEARCH_TEXT",
        help = "Track a process whose command line contains SEARCH_TEXT, labelled NAME in the plot."
    )]
    pub process_cmd_to_search: Vec<String>,

    // ── Per-worker intervals ──────────────────────────────────────────────────
    #[arg(
        long,
        value_name = "SYSINFO_INTERVAL",
        help = "Interval (seconds) at which the sysinfo worker refreshes CPU / memory / processes. Defaults to --check-interval."
    )]
    pub sysinfo_interval: Option<f32>,

    #[arg(
        long,
        default_value = "1.0",
        value_name = "NETWORK_INTERVAL",
        help = "Interval (seconds) at which the network worker polls interface counters."
    )]
    pub network_interval: f32,

    #[arg(
        long,
        default_value = "1.0",
        value_name = "GPU_INTERVAL",
        help = "Interval (seconds) at which the NVIDIA GPU worker polls via NVML."
    )]
    pub gpu_interval: f32,

    #[arg(
        long,
        default_value = "5.0",
        value_name = "DISK_INTERVAL",
        help = "Interval (seconds) at which disk space stats are refreshed."
    )]
    pub disk_interval: f32,

    #[arg(
        long,
        default_value = "1.0",
        value_name = "DISK_IO_INTERVAL",
        help = "Interval (seconds) over which disk busy% and read/write throughput are averaged."
    )]
    pub disk_io_interval: f32,

    // ── HTTP server ───────────────────────────────────────────────────────────
    #[arg(
        short = 's',
        long,
        default_value = "false",
        value_name = "SERVE",
        help = "Start HTTP server to serve real-time data."
    )]
    pub serve: bool,

    #[arg(
        short = 'P',
        long,
        default_value = "5998",
        value_name = "PORT",
        help = "Port for the HTTP server (requires --serve)."
    )]
    pub port: u16,

    #[arg(
        short = 'l',
        long,
        default_value = "86400",
        value_name = "SECONDS",
        help = "How much history the live web view keeps in memory, in seconds (default 86400 = 24 h). The sample count is derived from --check-interval. Exports are unaffected - they always read the whole CSV file."
    )]
    pub buffer_seconds: f32,

    #[arg(short = 'C', long, help = "Convert to HTML plot after collection finishes.")]
    pub convert_after: bool,

    // ── Top-N processes ───────────────────────────────────────────────────────
    #[arg(
        long,
        default_value = "0",
        value_name = "N",
        help = "Track the top N most CPU-hungry and RAM-hungry processes, writing them to separate files (0 = disabled) VERY RESOURCE-INTENSIVE, because it needs to refresh all processes"
    )]
    pub top_n_processes: usize,

    // ── Disk monitoring ───────────────────────────────────────────────────────
    #[arg(
        long,
        value_name = "MOUNT_OR_DEVICE",
        help = "Track a disk by mount point (e.g. /) or device name (e.g. /dev/nvme0n1p2). Repeatable."
    )]
    pub disk: Vec<String>,

    #[arg(long, default_value = "false", help = "Track all available non-virtual disks.")]
    pub all_disks: bool,

    #[arg(long, default_value = "false", help = "List available disks and exit.")]
    pub list_disks: bool,

    // ── Network interface selection ───────────────────────────────────────────
    #[arg(long, value_name = "INTERFACE", help = "Track a specific network interface (e.g. enp8s0). Repeatable.")]
    pub network: Vec<String>,

    #[arg(long, default_value = "false", help = "Track all available non-virtual network interfaces.")]
    pub all_networks: bool,

    #[arg(long, default_value = "false", help = "List available network interfaces and exit.")]
    pub list_networks: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct ConvertArgs {
    /// One or more data files: first is the main CSV, the rest are top-N process files
    /// (auto-detected from their header).
    /// Usage: -d system_data.csv -d system_data_top_cpu.csv -d system_data_top_ram.csv
    #[arg(short, long, num_args = 1.., default_values = &["system_data.csv"], value_name = "DATA_PATH")]
    pub data_paths: Vec<String>,

    #[command(flatten)]
    pub plot: PlotArgs,

    #[arg(
        long,
        default_value = "full",
        value_name = "SPLIT_MODE",
        help = "How to split the output: 'full' (single file), 'per-day' (one file per calendar day), 'per-week' (one file per ISO week)."
    )]
    pub split_mode: String,
}

pub(crate) fn parse_cli() -> Args {
    Args::parse()
}
