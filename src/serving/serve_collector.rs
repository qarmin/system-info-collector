use std::time::{Duration, SystemTime};
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Sender};
use log::{error, info};
use sysinfo::{ProcessesToUpdate, System};

use crate::enums::SimpleDataCollectionMode;
use crate::serving::data_buffer::{DataBuffer, ProcessData, SystemDataPoint};
use crate::settings::ServeSettings;

pub async fn collect_and_serve(
    sys: &mut System,
    settings: &ServeSettings,
    data_buffer: DataBuffer,
) -> Result<()> {
    let (tx, rx) = bounded::<SystemDataPoint>(100);

    // Spawn consumer task
    let buffer_clone = data_buffer.clone();
    tokio::spawn(async move {
        while let Ok(data_point) = rx.recv() {
            buffer_clone.push(data_point).await;
        }
    });

    info!("Starting data collection with interval: {}s", settings.check_interval);
    
    let (ctx, crx) = bounded(1);
    crate::set_ctrl_c_handler(ctx);

    let check_interval = settings.check_interval.max(0.25);
    let sleep_duration = Duration::from_secs_f32(check_interval);

    loop {
        if crx.try_recv().is_ok() {
            info!("Received shutdown signal");
            break;
        }

        let current_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs_f64();

        sys.refresh_memory();
        sys.refresh_cpu_all();
        
        if settings.need_to_refresh_processes {
            sys.refresh_processes(ProcessesToUpdate::All, true);
        }

        let data_point = collect_system_data(sys, settings, current_time);
        
        if let Err(e) = tx.send(data_point) {
            error!("Failed to send data point to buffer: {}", e);
        }

        tokio::time::sleep(sleep_duration).await;
    }

    Ok(())
}

fn collect_system_data(sys: &System, settings: &ServeSettings, timestamp: f64) -> SystemDataPoint {
    let cpu_usage = if settings.collection_mode.contains(&SimpleDataCollectionMode::CPU_USAGE_TOTAL) {
        Some(sys.global_cpu_usage())
    } else {
        None
    };

    let memory_used = if settings.collection_mode.contains(&SimpleDataCollectionMode::MEMORY_USED) {
        Some(sys.used_memory() as f64)
    } else {
        None
    };

    let memory_available = if settings.collection_mode.contains(&SimpleDataCollectionMode::MEMORY_AVAILABLE) {
        Some(sys.available_memory() as f64)
    } else {
        None
    };

    let swap_used = if settings.collection_mode.contains(&SimpleDataCollectionMode::SWAP_USED) {
        Some(sys.used_swap() as f64)
    } else {
        None
    };

    let custom_processes = settings
        .process_cmd_to_search
        .iter()
        .filter_map(|process_info| {
            sys.processes()
                .values()
                .find(|p| {
                    p.exe()
                        .and_then(|e| e.to_str())
                        .map(|e| e.contains(&process_info.search_text))
                        .unwrap_or(false)
                })
                .map(|p| ProcessData {
                    name: process_info.graph_name.clone(),
                    cpu_usage: p.cpu_usage(),
                    memory_usage: p.memory() as f64,
                })
        })
        .collect();

    SystemDataPoint {
        timestamp,
        cpu_usage,
        memory_used,
        memory_available,
        swap_used,
        custom_processes,
    }
}

