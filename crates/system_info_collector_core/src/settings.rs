use crate::enums::SimpleDataCollectionMode;

#[derive(Default, Clone, Debug)]
pub struct FindingStruct {
    pub graph_name: String,
    pub search_text: String,
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
}

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
    pub max_results: usize,
    /// Number of top processes to track by CPU% and RAM (0 = disabled).
    pub top_n_processes: usize,
    /// Disk mount points to track (empty = no disk monitoring).
    pub disk_mount_points: Vec<String>,
}
