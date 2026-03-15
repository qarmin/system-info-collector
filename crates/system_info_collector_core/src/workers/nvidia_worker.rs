use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::{info, warn};
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

use crate::settings::CollectSettings;
use crate::shared_state::{GpuSnapshot, SharedState};

pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>) {
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            warn!("NVML init failed ({e}), GPU monitoring disabled");
            return;
        }
    };

    let device = match nvml.device_by_index(0) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to access GPU device 0 ({e}), GPU monitoring disabled");
            return;
        }
    };

    info!("nvidia_worker: GPU monitoring active");

    let interval_ms = (settings.gpu_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let utilization_gpu = device.utilization_rates().map(|u| u.gpu).unwrap_or(0);
        let (memory_used_mb, memory_total_mb) = device
            .memory_info()
            .map(|m| (m.used / 1024 / 1024, m.total / 1024 / 1024))
            .unwrap_or((0, 0));
        let temperature = device.temperature(TemperatureSensor::Gpu).unwrap_or(0);

        let snapshot = GpuSnapshot {
            utilization_gpu,
            memory_used_mb,
            memory_total_mb,
            temperature,
        };

        {
            let mut guard = state.write().expect("SharedState RwLock poisoned");
            guard.latest_gpu = Some(snapshot);
        }
    }

    info!("nvidia_worker stopped");
}
