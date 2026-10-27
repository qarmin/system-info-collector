use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

use super::{Gpu, GpuKind, GpuReading};

pub fn discover(gpus: &mut Vec<Gpu>) {
    let nvml = match Nvml::init() {
        Ok(nvml) => nvml,
        Err(e) => {
            log::debug!("NVML unavailable ({e}), skipping NVIDIA GPUs");
            return;
        }
    };
    let driver_version = nvml.sys_driver_version().ok().map(|v| format!("NVIDIA {v}"));
    let count = match nvml.device_count() {
        Ok(count) => count,
        Err(e) => {
            log::warn!("Failed to query NVIDIA device count: {e}");
            return;
        }
    };
    for index in 0..count {
        match nvml.device_by_index(index) {
            Ok(device) => gpus.push(Gpu {
                name: device.name().unwrap_or_else(|_| format!("NVIDIA GPU {index}")),
                kind: GpuKind::Nvidia { nvml_index: index },
                vram_total_bytes: device.memory_info().ok().map(|m| m.total),
                driver_version: driver_version.clone(),
            }),
            Err(e) => log::warn!("Failed to access NVIDIA GPU {index}: {e}"),
        }
    }
}

pub fn read(nvml: &Nvml, index: u32) -> Option<GpuReading> {
    let device = nvml.device_by_index(index).inspect_err(|e| log::debug!("NVIDIA GPU {index}: {e}")).ok()?;
    let memory = device.memory_info().ok();
    Some(GpuReading {
        utilization_percent: device.utilization_rates().ok().map(|u| u.gpu),
        memory_used_bytes: memory.as_ref().map(|m| m.used),
        memory_total_bytes: memory.as_ref().map(|m| m.total),
        temperature_celsius: device.temperature(TemperatureSensor::Gpu).ok(),
    })
}
