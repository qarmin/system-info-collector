use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct DataPoint {
    pub timestamp: f64,
    pub data: Vec<String>,
}

impl DataPoint {
    pub fn from_collected_data(data: &Vec<String>) -> Self {
        let timestamp = if !data.is_empty() { data[0].parse::<f64>().unwrap_or(0.0) } else { 0.0 };

        Self {
            timestamp,
            data: data.clone(),
        }
    }

    pub fn from(data_str: &str) -> Self {
        let data: Vec<String> = data_str.split(',').map(std::string::ToString::to_string).collect();
        Self::from_collected_data(&data)
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

    pub async fn set_metadata(&self, metadata: SystemMetadata) {
        let mut meta = self.metadata.write().await;
        *meta = Some(metadata);
    }

    pub async fn get_metadata(&self) -> SystemMetadata {
        let meta = self.metadata.read().await;
        meta.clone().unwrap_or(SystemMetadata {
            system_info: SystemInfo {
                total_memory_mb: 0.0,
                total_swap_mb: 0.0,
                cpu_cores: 0,
                start_time: 0.0,
                app_version: "".to_string(),
            },
            column_headers: vec!["Timestamp".to_string()],
            max_buffer_size: self.max_size,
        })
    }

    pub async fn get_max_size(&self) -> usize {
        self.max_size
    }

    pub async fn add_data_point(&self, data_point: DataPoint) {
        let mut buffer = self.buffer.write().await;

        if buffer.len() >= self.max_size {
            buffer.pop_front();
        }

        buffer.push_back(data_point);
    }

    pub async fn get_last_n(&self, n: usize) -> Vec<DataPoint> {
        let buffer = self.buffer.read().await;
        let count = n.min(buffer.len());

        buffer.iter().rev().take(count).rev().cloned().collect()
    }

    pub async fn get_first_and_last(&self) -> (Option<DataPoint>, Option<DataPoint>) {
        let buffer = self.buffer.read().await;
        let first = buffer.front().cloned();
        let last = buffer.back().cloned();
        (first, last)
    }

    pub async fn len(&self) -> usize {
        let buffer = self.buffer.read().await;
        buffer.len()
    }
}
