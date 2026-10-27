/// Snapshot of CPU and memory metrics collected by system_worker.
#[derive(Debug, Clone, Default)]
pub struct SysinfoSnapshot {
    pub cpu_usage_total: f64,
    pub cpu_usage_per_core: Vec<f64>,
    pub memory_used_mb: f64,
    pub memory_free_mb: f64,
    pub memory_available_mb: f64,
    pub swap_used_mb: f64,
    pub swap_free_mb: f64,
}

/// Snapshot of a single GPU's metrics.  Produced by nvidia_worker or
/// amd_intel_gpu_worker; indexed by `DiscoveredGpu::gpu_index`.
#[derive(Debug, Clone, Default)]
/// Fields are `None` where the vendor does not expose them, e.g. memory of an Intel iGPU.
pub struct GpuSnapshot {
    /// GPU compute utilization in percent (0-100).
    pub utilization_gpu: Option<u32>,
    pub memory_used_mb: Option<u64>,
    pub memory_total_mb: Option<u64>,
    pub temperature: Option<u32>,
}

/// Snapshot of a single network interface's I/O metrics.
/// Indexed by `DiscoveredInterface::iface_index`.
#[derive(Debug, Clone, Default)]
pub struct NetworkInterfaceSnapshot {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub total_rx_bytes: u64,
    pub total_tx_bytes: u64,
}

/// Snapshot of a single disk's I/O activity, derived from the same counters
/// `iostat` uses.  Indexed by `DiscoveredDisk::disk_index`.
#[derive(Debug, Clone, Default)]
pub struct DiskIoSnapshot {
    /// Share of wall time the device had at least one request in flight (iostat %util).
    pub busy_pct: f64,
    pub read_mb_per_sec: f64,
    pub write_mb_per_sec: f64,
}

/// Per-process metrics for a single tracked process.
#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub pid: usize,
    pub name: String,
    pub cpu_usage: f32,
    pub memory_mb: f64,
}

/// Central shared state written by workers and read by the file_writer.
///
/// Protected by `std::sync::RwLock` — each worker holds the write lock only for
/// the brief moment it takes to copy a snapshot struct in.  The file_writer
/// holds the read lock only long enough to clone the latest snapshots out.
#[derive(Debug, Default)]
pub struct SharedState {
    /// Latest CPU / memory snapshot (written by system_worker).
    pub latest_sysinfo: Option<SysinfoSnapshot>,
    /// Latest per-interface network snapshots, indexed by iface_index.
    /// Sized to `RuntimeDiscovery::interface_count()` at engine startup.
    pub latest_networks: Vec<Option<NetworkInterfaceSnapshot>>,
    /// Latest per-GPU snapshots, indexed by gpu_index.
    /// Sized to `RuntimeDiscovery::gpu_count()` at engine startup.
    pub latest_gpus: Vec<Option<GpuSnapshot>>,
    /// Latest per-disk I/O snapshots, indexed by disk_index.
    /// Sized to `RuntimeDiscovery::disk_count()` at engine startup.
    pub latest_disk_io: Vec<Option<DiskIoSnapshot>>,
    /// Ping results accumulated since the last row, drained by file_writer.
    pub ping_windows: Vec<system_info_probe::ping::PingWindow>,
    /// Latest per-process snapshots, indexed by search-pattern slot.
    /// Written by system_worker alongside sysinfo data.
    pub latest_processes: Vec<Option<ProcessSnapshot>>,
}
