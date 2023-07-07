use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct SingleItemModel {
    pub unix_timestamp: f64,
    pub memory_used: u64,
    pub memory_available: u64,
    pub memory_free: u64,
    pub memory_total: u64,
    pub cpu_usage_per_core: String,
    pub cpu_total: f32,
}

impl SingleItemModel {
    pub fn to_csv_string(&self) -> String {
        format!(
            "{},{},{},{},{},{},{}",
            self.unix_timestamp,
            self.memory_used,
            self.memory_available,
            self.memory_free,
            self.memory_total,
            self.cpu_total,
            self.cpu_usage_per_core
        )
    }
}

#[derive(Default, Clone, Debug, Deserialize)]
pub struct CollectedItemModels {
    pub unix_timestamp: Vec<f64>,
    pub memory_used: Vec<u64>,
    pub memory_available: Vec<u64>,
    pub memory_free: Vec<u64>,
    pub memory_total: Vec<u64>,
    pub cpu_core_usage: Vec<Vec<f32>>,
    pub cpu_total: Vec<f32>,
}
impl CollectedItemModels {
    pub fn new_with_reserved_space(space: usize) -> Self {
        CollectedItemModels {
            unix_timestamp: Vec::with_capacity(space),
            memory_used: Vec::with_capacity(space),
            memory_available: Vec::with_capacity(space),
            memory_free: Vec::with_capacity(space),
            memory_total: Vec::with_capacity(space),
            cpu_core_usage: Vec::with_capacity(space),
            cpu_total: Vec::with_capacity(space),
        }
    }
}
