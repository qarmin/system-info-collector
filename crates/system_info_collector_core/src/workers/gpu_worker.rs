use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::info;
use system_info_probe::gpu::GpuSampler;

use crate::discovery::RuntimeDiscovery;
use crate::settings::CollectSettings;
use crate::shared_state::{GpuSnapshot, SharedState};

/// Every discovered GPU, whatever the vendor, into `state.latest_gpus[gpu_index]`.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, discovery: Arc<RuntimeDiscovery>) {
    let mut sampler = GpuSampler::new(discovery.gpus.iter().map(|g| g.gpu.clone()).collect());
    let interval_ms = (settings.gpu_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    // Utilization from RC6 residency and the Vivante idle register needs a window, which the
    // first sample opens.
    interval.tick().await;
    sampler.sample();

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let snapshots: Vec<Option<GpuSnapshot>> = sampler
            .sample()
            .readings
            .into_iter()
            .map(|reading| {
                reading.map(|r| GpuSnapshot {
                    utilization_gpu: r.utilization_percent,
                    memory_used_mb: r.memory_used_bytes.map(|b| b / 1024 / 1024),
                    memory_total_mb: r.memory_total_bytes.map(|b| b / 1024 / 1024),
                    temperature: r.temperature_celsius,
                })
            })
            .collect();
        state.write().expect("SharedState RwLock poisoned").latest_gpus = snapshots;
    }
    info!("gpu_worker stopped");
}
