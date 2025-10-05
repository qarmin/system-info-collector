use anyhow::{Context, Error};
use crossbeam_channel::unbounded;
use log::{debug, info};
use sysinfo::{ProcessesToUpdate, System};
use tokio::time::interval;

use std::time::{Duration, Instant, SystemTime};

use crate::collecting::collector::{check_for_new_and_old_process_data, get_system_pids};
use crate::enums::SimpleDataCollectionMode;
use crate::model::ProcessCache;
use crate::serving::data_buffer::{DataBuffer, DataPoint};
use crate::set_ctrl_c_handler;
use crate::settings::{ProcessSettings};

pub async fn collect_and_serve(sys: &mut System, settings: &ServeSettings, data_buffer: DataBuffer) -> Result<(), Error> {
    let mut interv = interval(Duration::from_millis((settings.check_interval * 1000.0) as u64));
    interv.tick().await; // This will instantly finish, so next time will take required amount of seconds

    let (ctx, crx) = unbounded::<()>();
    set_ctrl_c_handler(ctx);

    let mut process_cache_data = ProcessCache::new_with_size(settings.process_cmd_to_search.len(), sys);

    info!("Started collecting and serving data...");
    loop {
        collect_and_buffer_data(sys, settings, &data_buffer, &mut process_cache_data).await?;

        if crx.try_recv().is_ok() {
            return Ok(());
        }

        interv.tick().await;
    }
}

async fn collect_and_buffer_data(
    sys: &mut System,
    settings: &ServeSettings,
    data_buffer: &DataBuffer,
    process_cache_data: &mut ProcessCache,
) -> Result<(), Error> {
    let current_time = SystemTime::now();

    let start = Instant::now();
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    if settings.need_to_refresh_processes {
        check_for_new_and_old_process_data(sys, process_cache_data, settings)?;
    }

    debug!("Refreshed app/os usage data in {:?}", start.elapsed());

    let mut data_to_save = vec![];

    // SECONDS_SINCE_START - always required
    let timestamp = current_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("Cannot fail, because this cannot set time before UNIX_EPOCH")
        .as_secs_f64()
        - settings.start_time;

    data_to_save.push(format!("{:.2}", timestamp));

    for i in &settings.collection_mode {
        let collected_string = match i {
            SimpleDataCollectionMode::MEMORY_USED => convert_into_string_megabytes(sys.used_memory()),
            SimpleDataCollectionMode::MEMORY_AVAILABLE => convert_into_string_megabytes(sys.available_memory()),
            SimpleDataCollectionMode::MEMORY_FREE => convert_into_string_megabytes(sys.free_memory()),
            SimpleDataCollectionMode::CPU_USAGE_TOTAL => {
                format!(
                    "{:.2}",
                    sys.cpus().iter().map(sysinfo::Cpu::cpu_usage).sum::<f32>() / sys.cpus().len() as f32
                )
            }
            SimpleDataCollectionMode::CPU_USAGE_PER_CORE => sys.cpus().iter().map(|e| format!("{:.2}", e.cpu_usage())).collect::<Vec<_>>().join(";"),
            SimpleDataCollectionMode::SWAP_FREE => convert_into_string_megabytes(sys.free_swap()),
            SimpleDataCollectionMode::SWAP_USED => convert_into_string_megabytes(sys.used_swap()),
        };
        data_to_save.push(collected_string);
    }

    if settings.need_to_refresh_processes {
        for process_opt in &process_cache_data.process_used {
            if let Some(process) = process_opt {
                data_to_save.push(format!("{:.2}", process.cpu_usage / sys.cpus().len() as f32));
                data_to_save.push(convert_into_string_megabytes(process.memory_usage));
            } else {
                data_to_save.push("-1".to_string());
                data_to_save.push("-1".to_string());
            }
        }
    }

    let data_point = DataPoint {
        timestamp,
        data: data_to_save,
    };

    data_buffer.add_data_point(data_point).await;

    Ok(())
}

fn convert_into_string_megabytes(bytes: u64) -> String {
    format!("{:.2}", bytes as f64 / 1024.0 / 1024.0)
}

