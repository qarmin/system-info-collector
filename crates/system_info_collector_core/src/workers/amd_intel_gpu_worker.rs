use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::info;

use crate::discovery::{DiscoveredGpu, GpuVendor};
use crate::settings::CollectSettings;
use crate::shared_state::{GpuSnapshot, SharedState};

/// Polls AMD and Intel GPUs on Linux via sysfs.
/// Each GPU's snapshot is written to `state.latest_gpus[gpu.gpu_index]`.
///
/// AMD: reads `/sys/class/drm/cardN/device/gpu_busy_percent`,
///      `mem_info_vram_used`, `mem_info_vram_total`, and hwmon temperature.
/// Intel: approximates utilization via RC6 residency delta
///        (`/sys/class/drm/cardN/gt/gt0/rc6_residency_ms`).
#[cfg(target_os = "linux")]
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, gpus: Arc<Vec<DiscoveredGpu>>) {
    info!("amd_intel_gpu_worker: monitoring {} AMD/Intel GPU(s)", gpus.len());

    // Intel: track previous RC6 residency and wall-clock time for delta calculation.
    let mut intel_prev: Vec<Option<(u64, std::time::Instant)>> = gpus.iter().map(|_| None).collect();

    let interval_ms = (settings.gpu_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    // consume the instant first tick
    interval.tick().await;

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        for (i, gpu) in gpus.iter().enumerate() {
            let snapshot = match &gpu.vendor {
                GpuVendor::AmdLinux { handle, .. } => read_amd_snapshot(handle),
                GpuVendor::IntelLinux { card_device_path, .. } => {
                    let snap = read_intel_snapshot(card_device_path, intel_prev[i].as_ref());
                    // Update the prev state for the next iteration.
                    if let Some(rc6) = read_u64_from_sysfs(&card_device_path.join("gt/gt0/rc6_residency_ms")) {
                        intel_prev[i] = Some((rc6, std::time::Instant::now()));
                    }
                    snap
                }
                GpuVendor::Nvidia { .. } => continue, // handled by nvidia_worker
            };

            {
                let mut guard = state.write().expect("SharedState RwLock poisoned");
                if let Some(slot) = guard.latest_gpus.get_mut(gpu.gpu_index) {
                    *slot = Some(snapshot);
                }
            }
        }
    }

    info!("amd_intel_gpu_worker stopped");
}

/// Stub for non-Linux targets so the module always compiles.
#[cfg(not(target_os = "linux"))]
pub async fn run(_settings: Arc<CollectSettings>, _state: Arc<RwLock<SharedState>>, _shutdown: Arc<AtomicBool>, _gpus: Arc<Vec<DiscoveredGpu>>) {
    // AMD/Intel sysfs reading is only available on Linux.
}

/// Reads a live snapshot via `amdgpu-sysfs`, which wraps the same sysfs files
/// (`gpu_busy_percent`, `mem_info_vram_*`, hwmon `tempN_input`) with proper
/// multi-hwmon handling instead of hand-rolled parsing.
#[cfg(target_os = "linux")]
pub fn read_amd_snapshot(handle: &amdgpu_sysfs::gpu_handle::GpuHandle) -> GpuSnapshot {
    let utilization_gpu = u32::from(handle.get_busy_percent().unwrap_or(0));
    let memory_used_mb = handle.get_used_vram().unwrap_or(0) / 1024 / 1024;
    let memory_total_mb = handle.get_total_vram().unwrap_or(0) / 1024 / 1024;
    let temperature = read_amd_temperature(handle).unwrap_or(0);

    GpuSnapshot {
        utilization_gpu,
        memory_used_mb,
        memory_total_mb,
        temperature,
    }
}

#[cfg(target_os = "linux")]
fn read_intel_snapshot(card_path: &std::path::Path, prev: Option<&(u64, std::time::Instant)>) -> GpuSnapshot {
    // RC6 residency represents time the GPU is in a power-save state.
    // utilization ≈ (1 - rc6_delta_ms / elapsed_ms) * 100, clamped to 0..100.
    let rc6_ms = read_u64_from_sysfs(&card_path.join("gt/gt0/rc6_residency_ms")).unwrap_or(0);

    let utilization_gpu = if let Some((prev_rc6, prev_time)) = prev {
        let elapsed_ms = prev_time.elapsed().as_millis() as u64;
        if elapsed_ms > 0 {
            let rc6_delta = rc6_ms.saturating_sub(*prev_rc6);
            let busy_frac = 1.0 - (rc6_delta as f64 / elapsed_ms as f64);
            (busy_frac * 100.0).clamp(0.0, 100.0) as u32
        } else {
            0
        }
    } else {
        0
    };

    GpuSnapshot {
        utilization_gpu,
        memory_used_mb: 0, // Intel integrated — VRAM is shared with system RAM
        memory_total_mb: 0,
        temperature: 0, // Intel GPU temp not universally available via sysfs
    }
}

/// Prefers the "edge" sensor (die edge temperature, present on all AMD GPUs);
/// falls back to whatever sensor the hwmon reports first.
#[cfg(target_os = "linux")]
fn read_amd_temperature(handle: &amdgpu_sysfs::gpu_handle::GpuHandle) -> Option<u32> {
    handle.hw_monitors.first().and_then(|mon| {
        let temps = mon.get_temps();
        temps
            .get("edge")
            .or_else(|| temps.values().next())
            .and_then(|t| t.current)
            .map(|c| c as u32)
    })
}

#[cfg(target_os = "linux")]
fn read_u64_from_sysfs(path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
}
