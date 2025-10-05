use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

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

#[derive(Clone)]
pub struct DataBuffer {
    buffer: Arc<RwLock<VecDeque<DataPoint>>>,
    max_size: usize,
}

impl DataBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(max_size))),
            max_size,
        }
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
