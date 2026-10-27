use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::{info, warn};
use system_info_probe::disk::{DiskMetrics, DiskSampler};

use crate::collector::discovery::RuntimeDiscovery;
use crate::collector::settings::CollectSettings;
use crate::collector::shared_state::{DiskIoSnapshot, SharedState};

const MIB: f64 = 1_048_576.0;

/// Busy% and throughput of each tracked disk the way `iostat` derives them, into
/// `state.latest_disk_io[disk_index]`.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, discovery: Arc<RuntimeDiscovery>) {
    let mut sampler = DiskSampler::new(discovery.disks.iter().map(|d| d.tracked.clone()).collect(), DiskMetrics::Io);
    let interval_ms = (settings.disk_io_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    let mut previous = sampler.sample();
    if previous.counters.iter().all(Option::is_none) {
        warn!("No disk I/O counters available for the tracked disks, stopping disk_io_worker");
        return;
    }
    interval.tick().await;

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let current = sampler.sample();
        let snapshots = current
            .io_since(&previous)
            .into_iter()
            .map(|rates| {
                rates.map(|r| DiskIoSnapshot {
                    busy_pct: r.busy_percent,
                    read_mb_per_sec: r.read_bytes_per_sec / MIB,
                    write_mb_per_sec: r.write_bytes_per_sec / MIB,
                })
            })
            .collect();
        state.write().expect("SharedState RwLock poisoned").latest_disk_io = snapshots;
        previous = current;
    }
    info!("disk_io_worker stopped");
}
