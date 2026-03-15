use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

/// A single data point: the raw CSV columns for one measurement interval.
#[derive(Clone, Debug)]
pub struct DataPoint {
    pub timestamp: f64,
    pub data: Vec<String>,
}

impl DataPoint {
    pub fn from_row(row: &[String]) -> Self {
        let timestamp = row.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        Self {
            timestamp,
            data: row.to_vec(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemMetadata {
    pub system_info: SystemInfo,
    pub column_headers: Vec<String>,
    pub max_buffer_size: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    pub total_memory_mb: f64,
    pub total_swap_mb: f64,
    pub cpu_cores: usize,
    pub start_time: f64,
    pub app_version: String,
}

/// Thread-safe circular buffer used by the HTTP server for live data.
///
/// Uses `std::sync::RwLock` (not tokio's) so that the sync `on_row` callback
/// in the core engine can push data without an async context.
#[derive(Clone)]
pub struct DataBuffer {
    buffer: Arc<RwLock<VecDeque<DataPoint>>>,
    max_size: usize,
    metadata: Arc<RwLock<Option<SystemMetadata>>>,
}

impl DataBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(max_size))),
            max_size,
            metadata: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_metadata(&self, metadata: SystemMetadata) {
        let mut meta = self.metadata.write().expect("metadata lock poisoned");
        *meta = Some(metadata);
    }

    pub fn get_metadata(&self) -> SystemMetadata {
        let meta = self.metadata.read().expect("metadata lock poisoned");
        meta.clone().unwrap_or(SystemMetadata {
            system_info: SystemInfo {
                total_memory_mb: 0.0,
                total_swap_mb: 0.0,
                cpu_cores: 0,
                start_time: 0.0,
                app_version: String::new(),
            },
            column_headers: vec!["Timestamp".to_string()],
            max_buffer_size: self.max_size,
        })
    }

    pub fn get_max_size(&self) -> usize {
        self.max_size
    }

    pub fn add_data_point(&self, data_point: DataPoint) {
        let mut buffer = self.buffer.write().expect("buffer lock poisoned");
        if buffer.len() >= self.max_size {
            buffer.pop_front();
        }
        buffer.push_back(data_point);
    }

    pub fn get_last_n(&self, n: usize) -> Vec<DataPoint> {
        let buffer = self.buffer.read().expect("buffer lock poisoned");
        let count = n.min(buffer.len());
        buffer.iter().rev().take(count).rev().cloned().collect()
    }

    pub fn get_first_and_last(&self) -> (Option<DataPoint>, Option<DataPoint>) {
        let buffer = self.buffer.read().expect("buffer lock poisoned");
        (buffer.front().cloned(), buffer.back().cloned())
    }

    pub fn len(&self) -> usize {
        self.buffer.read().expect("buffer lock poisoned").len()
    }
}
