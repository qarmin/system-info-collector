use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::{info, warn};
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

use crate::discovery::{DiscoveredGpu, GpuVendor};
use crate::settings::CollectSettings;
use crate::shared_state::{GpuSnapshot, SharedState};

/// Polls all discovered NVIDIA GPUs via NVML.
/// Each GPU's snapshot is written to `state.latest_gpus[gpu.gpu_index]`.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, gpus: Arc<Vec<DiscoveredGpu>>) {
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            warn!("NVML init failed ({e}), NVIDIA GPU monitoring disabled");
            return;
        }
    };

    info!("nvidia_worker: monitoring {} NVIDIA GPU(s)", gpus.len());

    let interval_ms = (settings.gpu_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    // consume the instant first tick
    interval.tick().await;

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        for gpu in gpus.iter() {
            let nvml_index = match &gpu.vendor {
                GpuVendor::Nvidia { nvml_index, .. } => *nvml_index,
                _ => continue,
            };

            let device = match nvml.device_by_index(nvml_index) {
                Ok(d) => d,
                Err(e) => {
                    warn!("Failed to access NVIDIA GPU {} ({e})", gpu.gpu_index);
                    continue;
                }
            };

            let utilization_gpu = device.utilization_rates().map_or(0, |u| u.gpu);
            let (memory_used_mb, memory_total_mb) = device
                .memory_info()
                .map_or((0, 0), |m| (m.used / 1024 / 1024, m.total / 1024 / 1024));
            let temperature = device.temperature(TemperatureSensor::Gpu).unwrap_or(0);

            let snapshot = GpuSnapshot {
                utilization_gpu,
                memory_used_mb,
                memory_total_mb,
                temperature,
            };

            {
                let mut guard = state.write().expect("SharedState RwLock poisoned");
                if let Some(slot) = guard.latest_gpus.get_mut(gpu.gpu_index) {
                    *slot = Some(snapshot);
                }
            }
        }
    }

    info!("nvidia_worker stopped");
}
