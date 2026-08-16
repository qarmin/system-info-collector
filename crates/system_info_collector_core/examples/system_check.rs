#![allow(clippy::print_stdout)]

use std::thread::sleep;
use std::time::Duration;

use sysinfo::{CpuRefreshKind, Disks, Networks, System};
use system_info_collector_core::discovery::{GpuVendor, discover_gpus};
use system_info_collector_core::workers::sysinfo_worker::bytes_to_mb;

fn main() {
    print_cpu();
    print_memory();
    print_disks();
    print_network();
    print_gpus();
}

fn print_cpu() {
    let mut sys = System::new();
    sys.refresh_cpu_list(CpuRefreshKind::nothing());

    let logical = sys.cpus().len();
    let physical = System::physical_core_count().unwrap_or(0);
    let model = sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default();

    println!("== CPU ==");
    println!("{model} ({physical} physical / {logical} logical cores)\n");
}

fn print_memory() {
    let mut sys = System::new();
    sys.refresh_memory();

    println!("== Memory ==");
    println!(
        "RAM:  {:.0} MB total, {:.0} MB used",
        bytes_to_mb(sys.total_memory()),
        bytes_to_mb(sys.used_memory())
    );
    println!(
        "Swap: {:.0} MB total, {:.0} MB used\n",
        bytes_to_mb(sys.total_swap()),
        bytes_to_mb(sys.used_swap())
    );
}

fn print_disks() {
    println!("== Disks ==");
    let disks = Disks::new_with_refreshed_list();
    if disks.is_empty() {
        println!("none found");
    }
    for disk in &disks {
        let total_gb = disk.total_space() as f64 / 1024.0 / 1024.0 / 1024.0;
        let avail_gb = disk.available_space() as f64 / 1024.0 / 1024.0 / 1024.0;
        println!(
            "{:<20} {:<30} {:<10} {total_gb:>8.1} GB total, {avail_gb:>8.1} GB free",
            disk.name().to_string_lossy(),
            disk.mount_point().display(),
            disk.file_system().to_string_lossy(),
        );
    }
    println!();
}

fn print_network() {
    println!("== Network interfaces ==");
    let mut networks = Networks::new_with_refreshed_list();
    let mut names: Vec<String> = networks.keys().cloned().collect();
    names.sort_unstable();
    if names.is_empty() {
        println!("none found");
        return;
    }

    let before: Vec<(u64, u64)> = names
        .iter()
        .map(|name| networks.get(name.as_str()).map_or((0, 0), |n| (n.total_received(), n.total_transmitted())))
        .collect();

    let sample = Duration::from_millis(300);
    sleep(sample);
    networks.refresh(true);
    let elapsed = sample.as_secs_f64();

    for (name, &(prev_rx, prev_tx)) in names.iter().zip(before.iter()) {
        let (total_rx, total_tx) = networks
            .get(name.as_str())
            .map_or((prev_rx, prev_tx), |n| (n.total_received(), n.total_transmitted()));
        let rx_kbps = total_rx.saturating_sub(prev_rx) as f64 / elapsed / 1024.0;
        let tx_kbps = total_tx.saturating_sub(prev_tx) as f64 / elapsed / 1024.0;
        println!(
            "{name:<16} rx={rx_kbps:>8.1} KB/s  tx={tx_kbps:>8.1} KB/s  (total rx={:.1} MB, tx={:.1} MB)",
            total_rx as f64 / 1024.0 / 1024.0,
            total_tx as f64 / 1024.0 / 1024.0
        );
    }
    println!();
}

fn print_gpus() {
    println!("== GPU ==");
    let gpus = discover_gpus();
    if gpus.is_empty() {
        println!("none detected");
        return;
    }

    for gpu in &gpus {
        println!("[{}] {} - {}", gpu.gpu_index, gpu.display_name(), gpu.vendor_kind());
        match &gpu.vendor {
            GpuVendor::Nvidia { nvml_index, .. } => print_nvidia_sample(*nvml_index),
            GpuVendor::AmdLinux { handle, .. } => print_amd_sample(handle),
            GpuVendor::IntelLinux { card_device_path, .. } => print_intel_sample(card_device_path),
        }
    }
}

fn print_nvidia_sample(nvml_index: u32) {
    use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

    let Ok(nvml) = nvml_wrapper::Nvml::init() else {
        println!("    NVML unavailable for live sample");
        return;
    };
    let Ok(device) = nvml.device_by_index(nvml_index) else {
        println!("    failed to open NVML device");
        return;
    };

    let utilization = device.utilization_rates().map_or(0, |u| u.gpu);
    let (used_mb, total_mb) = device
        .memory_info()
        .map_or((0, 0), |m| (m.used / 1024 / 1024, m.total / 1024 / 1024));
    let temp = device.temperature(TemperatureSensor::Gpu).unwrap_or(0);

    println!("    utilization={utilization}%  memory={used_mb}/{total_mb} MB  temp={temp} C");
}

fn print_amd_sample(handle: &amdgpu_sysfs::gpu_handle::GpuHandle) {
    let utilization = handle.get_busy_percent().unwrap_or(0);
    let used_mb = handle.get_used_vram().unwrap_or(0) / 1024 / 1024;
    let total_mb = handle.get_total_vram().unwrap_or(0) / 1024 / 1024;
    let temp = handle.hw_monitors.first().and_then(|mon| {
        let temps = mon.get_temps();
        temps.get("edge").or_else(|| temps.values().next()).and_then(|t| t.current)
    });

    match temp {
        Some(t) => println!("    utilization={utilization}%  memory={used_mb}/{total_mb} MB  temp={t:.0} C"),
        None => println!("    utilization={utilization}%  memory={used_mb}/{total_mb} MB  temp=n/a"),
    }
}

// Intel exposes no instantaneous busy% in sysfs, only cumulative RC6 (idle) residency,
// so utilization has to be derived from a short before/after sample.
fn print_intel_sample(card_device_path: &std::path::Path) {
    let rc6_path = card_device_path.join("gt/gt0/rc6_residency_ms");
    let read_rc6 = || std::fs::read_to_string(&rc6_path).ok()?.trim().parse::<u64>().ok();

    let Some(start) = read_rc6() else {
        println!("    rc6_residency_ms not available");
        return;
    };
    let sample = Duration::from_millis(200);
    sleep(sample);
    let Some(end) = read_rc6() else {
        println!("    rc6_residency_ms not available");
        return;
    };

    let rc6_delta = end.saturating_sub(start);
    let busy = (1.0 - rc6_delta as f64 / sample.as_millis() as f64).clamp(0.0, 1.0) * 100.0;
    println!("    utilization~={busy:.0}% (approximated over a {}ms sample)", sample.as_millis());
}
