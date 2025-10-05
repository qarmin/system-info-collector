use std::sync::Arc;
use std::collections::VecDeque;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemDataPoint {
    pub timestamp: f64,
    pub cpu_usage: Option<f32>,
    pub memory_used: Option<f64>,
    pub memory_available: Option<f64>,
    pub swap_used: Option<f64>,
    pub custom_processes: Vec<ProcessData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessData {
    pub name: String,
    pub cpu_usage: f32,
    pub memory_usage: f64,
}

#[derive(Clone)]
pub struct DataBuffer {
    buffer: Arc<RwLock<VecDeque<SystemDataPoint>>>,
    max_size: usize,
}

impl DataBuffer {
    pub fn new(max_size: usize) -> Self {
        let clamped_size = max_size.clamp(1, 1000);
        Self {
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(clamped_size))),
            max_size: clamped_size,
        }
    }

    pub async fn push(&self, data: SystemDataPoint) {
        let mut buffer = self.buffer.write().await;
        if buffer.len() >= self.max_size {
            buffer.pop_front();
        }
        buffer.push_back(data);
    }

    pub async fn get_latest(&self, count: usize) -> Vec<SystemDataPoint> {
        let buffer = self.buffer.read().await;
        let take_count = count.min(buffer.len());
        buffer.iter().rev().take(take_count).rev().cloned().collect()
    }

    pub async fn get_all(&self) -> Vec<SystemDataPoint> {
        let buffer = self.buffer.read().await;
        buffer.iter().cloned().collect()
    }

    pub async fn len(&self) -> usize {
        let buffer = self.buffer.read().await;
        buffer.len()
    }
}

