/// Snapshot of CPU and memory metrics collected by sysinfo_worker.
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
pub struct GpuSnapshot {
    /// GPU compute utilization in percent (0-100).
    pub utilization_gpu: u32,
    /// GPU memory used in megabytes.
    pub memory_used_mb: u64,
    /// GPU memory total in megabytes.
    pub memory_total_mb: u64,
    /// GPU core temperature in Celsius.
    pub temperature: u32,
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
    /// Latest CPU / memory snapshot (written by sysinfo_worker).
    pub latest_sysinfo: Option<SysinfoSnapshot>,
    /// Latest per-interface network snapshots, indexed by iface_index.
    /// Sized to `RuntimeDiscovery::interface_count()` at engine startup.
    pub latest_networks: Vec<Option<NetworkInterfaceSnapshot>>,
    /// Latest per-GPU snapshots, indexed by gpu_index.
    /// Sized to `RuntimeDiscovery::gpu_count()` at engine startup.
    pub latest_gpus: Vec<Option<GpuSnapshot>>,
    /// Latest per-process snapshots, indexed by search-pattern slot.
    /// Written by sysinfo_worker alongside sysinfo data.
    pub latest_processes: Vec<Option<ProcessSnapshot>>,
    /// Top N processes by CPU% (name, cpu_pct_normalized).
    pub latest_top_cpu: Vec<(String, f32)>,
    /// Top N processes by RAM (name, ram_mb).
    pub latest_top_ram: Vec<(String, f64)>,
}
