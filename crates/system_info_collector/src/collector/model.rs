use crate::collector::enums::{DataType, GeneralInfoGroup};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Default, Clone, Debug, Deserialize)]
pub struct CollectedItemModels {
    pub collected_data: HashMap<DataType, Vec<String>>,
    pub collected_groups: Vec<GeneralInfoGroup>,
    pub memory_total: f64,
    pub swap_total: f64,
    pub cpu_core_count: usize,
    pub check_interval: f32,
    pub start_time: f64,
    /// CPU model string from CSV metadata (empty on old files).
    pub cpu_model: String,
    /// GPU names parsed from CSV metadata (GPU_0=name, GPU_1=name, …).
    pub gpu_names: Vec<String>,
    /// Total VRAM per GPU in MB, parallel to `gpu_names` (0 if unknown).
    pub gpu_vram_mb: Vec<u64>,
    /// Driver per GPU, parallel to `gpu_names` (empty if unknown or on old files).
    pub gpu_driver_versions: Vec<String>,
    /// Disk labels parsed from CSV metadata (DISK_LABEL_N, mount point on older files).
    pub disk_labels: Vec<String>,
    /// Network interface labels parsed from CSV metadata (NET_LABEL_N, name on older files).
    pub net_labels: Vec<String>,
}
