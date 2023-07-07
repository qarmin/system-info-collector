use crate::model::SingleItemModel;
use log::debug;
use std::io::BufWriter;

use std::io::Write;
use std::time::SystemTime;
use sysinfo::{CpuExt, System, SystemExt};

pub fn save_system_info_to_file(sys: &mut System, buf_file: &mut BufWriter<std::fs::File>) {
    let current_time = SystemTime::now();

    let start = SystemTime::now();
    sys.refresh_cpu();
    sys.refresh_memory();
    // sys.refresh_processes();
    debug!("Refreshed CPU and memory in {:?}", start.elapsed().unwrap());

    let data_item = SingleItemModel {
        unix_timestamp: current_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64(),
        memory_used: convert_bytes_into_mega_bytes(sys.used_memory()),
        memory_available: convert_bytes_into_mega_bytes(sys.available_memory()),
        memory_free: convert_bytes_into_mega_bytes(sys.free_memory()),
        memory_total: convert_bytes_into_mega_bytes(sys.total_memory()),
        cpu_total: sys.cpus().iter().map(sysinfo::CpuExt::cpu_usage).sum::<f32>() / sys.cpus().len() as f32,
        cpu_usage_per_core: sys.cpus().iter().map(|e| format!("{:.2}", e.cpu_usage())).collect::<Vec<_>>().join(";"),
    };

    writeln!(buf_file, "{}", data_item.to_csv_string()).unwrap();
}

pub fn convert_bytes_into_mega_bytes(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}
