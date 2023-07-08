use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::time::{Duration, SystemTime};

use anyhow::Error;
use crossbeam_channel::unbounded;
use log::{debug, info};
use sysinfo::{CpuExt, System, SystemExt};
use tokio::time::interval;

use crate::model::{Settings, SingleItemModel};
use crate::ploty_creator::load_results_and_save_plot;
use crate::set_ctrl_c_handler;

pub async fn collect_data(sys: &mut System, settings: &Settings) -> Result<(), Error> {
    let csv_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&settings.data_path)
        .unwrap();
    let mut csv_file = BufWriter::new(csv_file);
    let string_header = "unix_timestamp,memory_used,memory_available,memory_free,memory_total,cpu_total,cpu_usage_per_core".to_string();
    writeln!(csv_file, "{string_header}").unwrap();

    let time_interval_miliseconds = 1000;
    let mut interv = interval(Duration::from_millis(time_interval_miliseconds));
    interv.tick().await; // This will instantly finish, so next time will take required amount of seconds

    let (ctx, crx) = unbounded::<()>();
    set_ctrl_c_handler(ctx);

    info!("Started collecting data...");
    loop {
        save_system_info_to_file(sys, &mut csv_file);

        if crx.try_recv().is_ok() {
            drop(csv_file);
            if settings.app_mode == crate::enums::AppMode::COLLECT_AND_CONVERT {
                load_results_and_save_plot(settings)?;
            }
            return Ok(());
        }

        interv.tick().await;
    }
}

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
