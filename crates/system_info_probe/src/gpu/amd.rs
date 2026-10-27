use std::path::PathBuf;

use amdgpu_sysfs::gpu_handle::GpuHandle;

use super::{Gpu, GpuKind, GpuReading};

pub fn discover(gpus: &mut Vec<Gpu>, device_path: PathBuf, card_name: &str) {
    let handle = match GpuHandle::new_from_path(device_path.clone()) {
        Ok(handle) => handle,
        Err(e) => {
            log::debug!("AMD card {card_name}: no sysfs handle ({e}), skipping");
            return;
        }
    };
    if handle.get_busy_percent().is_err() {
        log::debug!("AMD card {card_name}: gpu_busy_percent not available, skipping");
        return;
    }
    gpus.push(Gpu {
        name: crate::pci::device_name(&device_path).unwrap_or_else(|| format!("AMD GPU ({card_name})")),
        vram_total_bytes: handle.get_total_vram().ok(),
        driver_version: super::kernel_driver_version(&device_path),
        kind: GpuKind::Amd { device_path },
    });
}

/// `gpu_busy_percent`, `mem_info_vram_*` and hwmon temperature, through `amdgpu-sysfs` for its
/// multi-hwmon handling.
pub fn read(handle: &GpuHandle) -> GpuReading {
    GpuReading {
        utilization_percent: handle.get_busy_percent().ok().map(u32::from),
        memory_used_bytes: handle.get_used_vram().ok(),
        memory_total_bytes: handle.get_total_vram().ok(),
        temperature_celsius: temperature(handle),
    }
}

/// The "edge" sensor (die edge, present on every AMD GPU), else whatever hwmon lists first.
fn temperature(handle: &GpuHandle) -> Option<u32> {
    let temps = handle.hw_monitors.first()?.get_temps();
    temps.get("edge").or_else(|| temps.values().next())?.current.map(|c| c as u32)
}
