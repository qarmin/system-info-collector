use crate::enums::SimpleDataCollectionMode;

#[derive(Default, Clone, Debug)]
pub struct FindingStruct {
    pub graph_name: String,
    pub search_text: String,
}

/// How to split the output when converting a CSV to HTML.
#[derive(Default, Clone, Debug, PartialEq)]
pub enum SplitMode {
    /// Single output file containing all data.
    #[default]
    Full,
    /// One output file per calendar day (local time).
    PerDay,
    /// One output file per ISO week (local time).
    PerWeek,
}

#[derive(Default, Clone, Debug)]
pub struct ConvertSettings {
    /// Main data CSV (first -d argument).
    pub data_path: String,
    /// Additional data files (extra -d arguments): top-CPU and/or top-RAM process files.
    pub extra_data_paths: Vec<String>,
    pub plot_path: String,
    pub plot_width: u32,
    pub plot_height: u32,
    pub white_plot_mode: bool,
    pub open_plot_file: bool,
    /// How to split the output into multiple HTML files.
    pub split_mode: SplitMode,
}

/// Upper bound on live-buffer samples, so a short interval combined with a long
/// buffer duration cannot exhaust memory.
pub const MAX_BUFFER_SAMPLES: usize = 1_000_000;

#[derive(Default, Clone, Debug)]
pub struct CollectSettings {
    /// Main check interval (seconds) — how often file_writer writes a CSV row.
    pub check_interval: f32,
    /// Interval at which sysinfo_worker refreshes CPU / memory / processes.
    pub sysinfo_interval_secs: f32,
    /// Interval at which network_worker refreshes network counters.
    pub network_interval_secs: f32,
    /// Interval at which nvidia_worker polls the GPU.
    pub gpu_interval_secs: f32,
    /// Interval at which disk stats are refreshed inside file_writer.
    pub disk_interval_secs: f32,
    /// Interval at which disk_io_worker samples the cumulative I/O counters.
    pub disk_io_interval_secs: f32,

    pub convert: ConvertSettings,
    pub collection_mode: Vec<SimpleDataCollectionMode>,
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
    /// How much history the live web-view buffer should hold, in seconds.
    /// The number of samples is derived from this and `check_interval`.
    pub buffer_seconds: f32,
    /// Number of top processes to track by CPU% and RAM (0 = disabled).
    pub top_n_processes: usize,
    /// Disk mount points or device names to track (empty + !all_disks = no disk monitoring).
    pub disk_mount_points: Vec<String>,
    /// If true, track all available non-virtual disks.
    pub all_disks: bool,
    /// Mount points or device names to leave out of `all_disks`.
    pub excluded_disks: Vec<String>,
    /// Specific network interface names to track (empty + !all_networks = no network collection).
    pub network_interfaces: Vec<String>,
    /// If true, track all available non-virtual network interfaces.
    pub all_networks: bool,
    /// Interface names to leave out of `all_networks`.
    pub excluded_networks: Vec<String>,
    /// If true, repeated values are omitted from CSV rows (written as empty strings).
    /// The reader fills them back in from the previous row.  Reduces file size significantly
    /// for slow-changing metrics.  Disable with --no-compact.
    pub compact_csv: bool,
}

impl CollectSettings {
    /// Number of samples the live buffer needs to cover `buffer_seconds` at the
    /// configured collection interval, capped at [`MAX_BUFFER_SAMPLES`].
    pub fn buffer_capacity(&self) -> usize {
        let interval = self.check_interval.max(0.1);
        ((self.buffer_seconds / interval).ceil() as usize).clamp(1, MAX_BUFFER_SAMPLES)
    }
}
