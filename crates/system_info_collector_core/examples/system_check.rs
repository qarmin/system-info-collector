#![expect(clippy::print_stdout, reason = "prints a report for a person")]

use std::thread::sleep;
use std::time::Duration;

use system_info_probe::disk::{DiskMetrics, DiskQuery, DiskSampler};
use system_info_probe::gpu::{GpuDiscovery, GpuSampler};
use system_info_probe::network::{InterfaceQuery, NetworkSampler};
use system_info_probe::system::{MachineInfo, SystemSampler};

const MIB: f64 = 1_048_576.0;
const GIB: f64 = 1_073_741_824.0;
const WINDOW: Duration = Duration::from_secs(1);

fn main() {
    let machine = MachineInfo::read();
    println!("== CPU ==");
    println!(
        "{} ({} physical / {} logical cores)\n",
        machine.cpu_model.unwrap_or_default(),
        machine.physical_cores.unwrap_or(0),
        machine.logical_cores
    );

    println!("== Memory ==");
    if let Ok(sample) = SystemSampler::new().and_then(|mut s| s.sample()) {
        let m = sample.memory;
        println!(
            "RAM:  {:.0} MB total, {:.0} MB used",
            m.total_bytes as f64 / MIB,
            m.used_bytes as f64 / MIB
        );
        println!(
            "Swap: {:.0} MB total, {:.0} MB used\n",
            m.swap_total_bytes as f64 / MIB,
            m.swap_used_bytes() as f64 / MIB
        );
    }

    println!("== Disks ==");
    let filesystems = system_info_probe::disk::filesystems();
    for fs in &filesystems {
        let space = system_info_probe::disk::space(&fs.mount_point);
        println!(
            "{:<20} {:<30} {:<10} {:>8.1} GB total, {:>8.1} GB free",
            fs.device,
            fs.mount_point,
            fs.file_system,
            fs.total_bytes as f64 / GIB,
            space.map_or(0.0, |s| s.available_bytes as f64 / GIB)
        );
    }
    let (tracked, _) = system_info_probe::disk::select(&filesystems, &DiskQuery::All { excluded: Vec::new() });
    let mut disks = DiskSampler::new(tracked, DiskMetrics::Io);
    let before = disks.sample();
    sleep(WINDOW);
    let after = disks.sample();
    println!("\n== Disk I/O over {WINDOW:?} ==");
    for (disk, rates) in disks.disks().iter().zip(after.io_since(&before)) {
        match rates {
            Some(r) => println!(
                "{:<30} busy {:>5.1}%  read {:>7.2} MB/s  write {:>7.2} MB/s",
                disk.filesystem.mount_point,
                r.busy_percent,
                r.read_bytes_per_sec / MIB,
                r.write_bytes_per_sec / MIB
            ),
            None => println!("{:<30} no I/O counters", disk.filesystem.mount_point),
        }
    }

    println!("\n== Network over {WINDOW:?} ==");
    let (interfaces, _) = system_info_probe::network::select(&InterfaceQuery::All { excluded: Vec::new() });
    let mut network = NetworkSampler::new(interfaces);
    let before = network.sample();
    sleep(WINDOW);
    let after = network.sample();
    for (interface, rates) in network.interfaces().iter().zip(after.rates_since(&before)) {
        let (rx, tx) = rates.map_or((0.0, 0.0), |r| (r.rx_bytes_per_sec / MIB, r.tx_bytes_per_sec / MIB));
        println!(
            "{:<16} {:<10} rx {rx:>7.2} MB/s  tx {tx:>7.2} MB/s",
            interface.name,
            interface.kind.label()
        );
    }

    println!("\n== GPUs ==");
    let gpus = system_info_probe::gpu::discover(GpuDiscovery::default());
    if gpus.is_empty() {
        println!("none found");
    }
    let mut sampler = GpuSampler::new(gpus);
    sampler.sample();
    sleep(WINDOW);
    let readings = sampler.sample().readings;
    for (gpu, reading) in sampler.gpus().iter().zip(readings) {
        let reading = reading.unwrap_or_default();
        let show = |value: Option<String>| value.unwrap_or_else(|| "-".to_string());
        println!(
            "{} ({}, driver {}): util {}%, VRAM {} / {} MB, {} °C",
            gpu.name,
            gpu.kind.vendor(),
            gpu.driver_version.as_deref().unwrap_or("unknown"),
            show(reading.utilization_percent.map(|v| v.to_string())),
            show(reading.memory_used_bytes.map(|b| (b / 1024 / 1024).to_string())),
            show(reading.memory_total_bytes.map(|b| (b / 1024 / 1024).to_string())),
            show(reading.temperature_celsius.map(|v| v.to_string())),
        );
    }
}
