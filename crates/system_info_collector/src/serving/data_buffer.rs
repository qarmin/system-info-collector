use axum::extract::ws::Utf8Bytes;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use system_info_collector_core::discovery::MountedFilesystem;
use tokio::sync::broadcast;

/// How many frames a slow websocket client may fall behind before it is told to
/// skip ahead to the newest data.  Kept small on purpose - the frames are
/// retained in memory, and a client that cannot keep up is better served by
/// jumping to the present than by replaying a long backlog.
const LIVE_CHANNEL_CAPACITY: usize = 64;

/// A single data point: the raw CSV columns for one measurement interval.
#[derive(Clone, Debug)]
pub struct DataPoint {
    pub timestamp: f64,
    pub data: Vec<String>,
}

impl DataPoint {
    pub fn from_row(row: Vec<String>) -> Self {
        let timestamp = row.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        Self { timestamp, data: row }
    }
}

/// One websocket frame: everything collected in a single tick.  Serialized once
/// in the collector thread and shared by reference with every client, so the
/// per-tick cost does not grow with the number of open browsers.
#[derive(Serialize)]
struct LiveTick<'a> {
    timestamp: f64,
    data: &'a [String],
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemMetadata {
    pub system_info: SystemInfo,
    /// Canonical CSV column names - the web UI groups columns into charts by these.
    pub column_headers: Vec<String>,
    /// Readable name per column, parallel to `column_headers`, e.g.
    /// `/home (nvme1n1 916 GB) busy %` instead of `DISK_1_BUSY_PCT`.
    pub column_labels: Vec<String>,
    pub max_buffer_size: usize,
    pub check_interval: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    /// Distribution and release, e.g. `Ubuntu 24.04.3 LTS`.
    pub os_name: String,
    pub kernel_version: String,
    pub hostname: String,
    pub total_memory_mb: f64,
    pub total_swap_mb: f64,
    pub cpu_cores: usize,
    pub cpu_physical_cores: usize,
    pub cpu_model: String,
    pub gpu_names: Vec<String>,
    /// Total VRAM per GPU in MB, parallel to `gpu_names` (0 = unknown).
    pub gpu_vram_mb: Vec<u64>,
    /// Every attached filesystem, carrying the DISK_N index of the tracked ones.
    pub mounted_disks: Vec<MountedFilesystem>,
    /// What each NET_N column refers to, e.g. `wlan0 (WiFi - Wi-Fi 6 AX201)`.
    pub net_labels: Vec<String>,
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
    updates: broadcast::Sender<Utf8Bytes>,
}

impl DataBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            // Grown on demand rather than preallocated - a 24 h buffer would
            // otherwise reserve its full size before a single sample arrives.
            buffer: Arc::new(RwLock::new(VecDeque::new())),
            max_size,
            metadata: Arc::new(RwLock::new(None)),
            updates: broadcast::Sender::new(LIVE_CHANNEL_CAPACITY),
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
                os_name: String::new(),
                kernel_version: String::new(),
                hostname: String::new(),
                total_memory_mb: 0.0,
                total_swap_mb: 0.0,
                cpu_cores: 0,
                cpu_physical_cores: 0,
                cpu_model: String::new(),
                gpu_names: vec![],
                gpu_vram_mb: vec![],
                mounted_disks: vec![],
                net_labels: vec![],
                start_time: 0.0,
                app_version: String::new(),
            },
            column_headers: vec!["Timestamp".to_string()],
            column_labels: vec!["Timestamp".to_string()],
            max_buffer_size: self.max_size,
            check_interval: 1.0,
        })
    }

    pub fn get_max_size(&self) -> usize {
        self.max_size
    }

    /// Subscribe to the live tick stream.  Each websocket client gets its own
    /// receiver; every frame is serialized once and cloned by reference.
    pub fn subscribe(&self) -> broadcast::Receiver<Utf8Bytes> {
        self.updates.subscribe()
    }

    /// With no browser connected there is nothing to serialize, so the whole
    /// live-update path is skipped.
    fn has_listeners(&self) -> bool {
        self.updates.receiver_count() > 0
    }

    pub fn add_data_point(&self, data_point: DataPoint) {
        if self.has_listeners() {
            let tick = LiveTick {
                timestamp: data_point.timestamp,
                data: &data_point.data,
            };
            match serde_json::to_string(&tick) {
                Ok(json) => {
                    let _ = self.updates.send(json.into());
                }
                Err(e) => log::warn!("Failed to serialize live update: {e}"),
            }
        }

        let mut buffer = self.buffer.write().expect("buffer lock poisoned");
        if buffer.len() >= self.max_size {
            buffer.pop_front();
        }
        buffer.push_back(data_point);
    }

    /// Newest points covering the last `seconds` (relative to the newest point),
    /// capped at `limit` entries.  `None` means unbounded.
    pub fn get_range(&self, seconds: Option<f64>, limit: Option<usize>) -> Vec<DataPoint> {
        let buffer = self.buffer.read().expect("buffer lock poisoned");
        collect_range(buffer.iter().rev(), |p| p.timestamp, buffer.back().map(|p| p.timestamp), seconds, limit)
    }

    pub fn get_first_and_last(&self) -> (Option<DataPoint>, Option<DataPoint>) {
        let buffer = self.buffer.read().expect("buffer lock poisoned");
        (buffer.front().cloned(), buffer.back().cloned())
    }

    pub fn len(&self) -> usize {
        self.buffer.read().expect("buffer lock poisoned").len()
    }
}

/// Walk a newest-first iterator, keep entries not older than `newest - seconds`
/// (up to `limit` of them) and return them in chronological order.
fn collect_range<'a, T, I, F>(rev_iter: I, timestamp_of: F, newest: Option<f64>, seconds: Option<f64>, limit: Option<usize>) -> Vec<T>
where
    T: Clone + 'a,
    I: Iterator<Item = &'a T>,
    F: Fn(&T) -> f64,
{
    let cutoff = match (seconds, newest) {
        (Some(s), Some(newest)) => Some(newest - s),
        _ => None,
    };
    let mut out: Vec<T> = rev_iter
        .take_while(|p| cutoff.is_none_or(|c| timestamp_of(p) >= c))
        .take(limit.unwrap_or(usize::MAX))
        .cloned()
        .collect();
    out.reverse();
    out
}
