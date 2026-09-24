use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use log::{info, warn};

use crate::discovery::RuntimeDiscovery;
use crate::disk_stats::{DiskCounters, rates, read_counters};
use crate::settings::CollectSettings;
use crate::shared_state::SharedState;

/// Turns the cumulative `/proc/diskstats` counters into per-interval rates, the
/// way `iostat` does: busy% from the delta of `io_ticks`, throughput from the
/// delta of transferred sectors.  Writes `state.latest_disk_io[disk_index]`.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, discovery: Arc<RuntimeDiscovery>) {
    let interval_ms = (settings.disk_io_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));

    let mut previous: Option<(HashMap<String, DiskCounters>, Instant)> = None;

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let counters = read_counters();
        let now = Instant::now();

        if counters.is_empty() {
            warn!("No disk I/O counters available on this system, stopping disk_io_worker");
            break;
        }

        if let Some((previous_counters, measured_at)) = &previous {
            let elapsed = now.duration_since(*measured_at).as_secs_f64();
            if elapsed > 0.0 {
                let snapshots = discovery
                    .disks
                    .iter()
                    .map(|disk| rates(disk.io_stat_name.as_deref()?, &counters, previous_counters, elapsed))
                    .collect();

                let mut guard = state.write().expect("SharedState RwLock poisoned");
                guard.latest_disk_io = snapshots;
            }
        }

        previous = Some((counters, now));
    }

    info!("disk_io_worker stopped");
}
