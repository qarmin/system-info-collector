use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Error};
use log::{error, info};
use sysinfo::System;

use crate::enums::{HeaderValues, SimpleDataCollectionMode};
use crate::settings::CollectSettings;
use crate::shared_state::SharedState;
use crate::workers::sysinfo_worker::bytes_to_mb;

/// Spawned as a tokio task.  Wakes every `check_interval` seconds, clones the
/// latest snapshots out of `SharedState` (brief read-lock), formats a CSV row,
/// writes it to disk and calls `on_row` so the HTTP server can update its
/// in-memory buffer.
pub async fn run<F>(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, mut data_file: BufWriter<File>, on_row: Arc<F>)
where
    F: Fn(Vec<String>) + Send + Sync + 'static,
{
    let interval_ms = (settings.check_interval * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(100)));
    // consume the first instant tick
    interval.tick().await;

    let mut collected_bytes: usize = 0;

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let seconds_since_start = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("time before UNIX_EPOCH")
            .as_secs_f64()
            - settings.start_time;

        // Brief read-lock to clone the latest snapshots.
        let (sysinfo_snap, network_snap, gpu_snap, process_snaps) = {
            let guard = state.read().expect("SharedState RwLock poisoned");
            (guard.latest_sysinfo.clone(), guard.latest_network.clone(), guard.latest_gpu.clone(), guard.latest_processes.clone())
        };

        // Build the CSV row in the same column order as the header.
        let mut row: Vec<String> = Vec::with_capacity(16);
        row.push(format!("{seconds_since_start:.2}"));

        for mode in &settings.collection_mode {
            let value = match mode {
                SimpleDataCollectionMode::CPU_USAGE_TOTAL => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| format!("{:.2}", s.cpu_usage_total)),
                SimpleDataCollectionMode::CPU_USAGE_PER_CORE => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| s.cpu_usage_per_core.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(";")),
                SimpleDataCollectionMode::MEMORY_USED => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| format!("{:.2}", s.memory_used_mb)),
                SimpleDataCollectionMode::MEMORY_FREE => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| format!("{:.2}", s.memory_free_mb)),
                SimpleDataCollectionMode::MEMORY_AVAILABLE => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| format!("{:.2}", s.memory_available_mb)),
                SimpleDataCollectionMode::SWAP_USED => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| format!("{:.2}", s.swap_used_mb)),
                SimpleDataCollectionMode::SWAP_FREE => sysinfo_snap.as_ref().map_or("-1".to_string(), |s| format!("{:.2}", s.swap_free_mb)),
                SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC => network_snap.as_ref().map_or("-1".to_string(), |n| format!("{:.2}", n.rx_bytes_per_sec)),
                SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC => network_snap.as_ref().map_or("-1".to_string(), |n| format!("{:.2}", n.tx_bytes_per_sec)),
                SimpleDataCollectionMode::GPU_UTILIZATION => gpu_snap.as_ref().map_or("-1".to_string(), |g| g.utilization_gpu.to_string()),
                SimpleDataCollectionMode::GPU_MEMORY_USED => gpu_snap.as_ref().map_or("-1".to_string(), |g| g.memory_used_mb.to_string()),
                SimpleDataCollectionMode::GPU_TEMPERATURE => gpu_snap.as_ref().map_or("-1".to_string(), |g| g.temperature.to_string()),
            };
            row.push(value);
        }

        // Custom process columns (two per pattern: CPU%, memory MB)
        for proc_opt in &process_snaps {
            if let Some(p) = proc_opt {
                row.push(format!("{:.2}", p.cpu_usage));
                row.push(format!("{:.2}", p.memory_mb));
            } else {
                row.push("-1".to_string());
                row.push("-1".to_string());
            }
        }

        let row_str = row.join(",");
        collected_bytes += row_str.len();

        if collected_bytes >= settings.maximum_data_file_size_bytes {
            let _ = data_file.flush();
            error!(
                "Exceeded allowed data size ({}), stopping collection",
                humansize::format_size(settings.maximum_data_file_size_bytes, humansize::BINARY)
            );
            shutdown.store(true, Ordering::Relaxed);
            break;
        }

        if let Err(e) = writeln!(data_file, "{row_str}").context(format!("Failed to write to {}", settings.convert.data_path)) {
            error!("{e}");
            shutdown.store(true, Ordering::Relaxed);
            break;
        }

        if !settings.disable_instant_flushing {
            if let Err(e) = data_file.flush().context(format!("Failed to flush {}", settings.convert.data_path)) {
                error!("{e}");
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
        }

        on_row(row);
    }

    info!("file_writer stopped");
}

/// Write the two-line CSV header (metadata line + column-name line).
/// Requires an initial `System` refresh for memory / CPU metadata.
pub fn write_csv_header(data_file: &mut BufWriter<File>, sys: &System, settings: &CollectSettings, app_version: &str) -> Result<(), Error> {
    let custom_headers = settings
        .process_cmd_to_search
        .iter()
        .enumerate()
        .map(|(idx, e)| format!("CUSTOM_{idx}={}", e.graph_name))
        .collect::<Vec<_>>()
        .join(",");

    let custom_headers = if custom_headers.is_empty() {
        String::new()
    } else {
        format!(",{custom_headers}")
    };

    let mem_total = bytes_to_mb(sys.total_memory());
    let swap_total = bytes_to_mb(sys.total_swap());
    let general_info = format!(
        "{}={},{}={},{}={mem_total:.2},{}={swap_total:.2},{}={},{}={}{}",
        HeaderValues::INTERVAL_SECONDS,
        settings.check_interval,
        HeaderValues::CPU_CORE_COUNT,
        sys.cpus().len(),
        HeaderValues::MEMORY_TOTAL,
        HeaderValues::SWAP_TOTAL,
        HeaderValues::UNIX_TIMESTAMP_START_TIME,
        settings.start_time,
        HeaderValues::APP_VERSION,
        app_version,
        custom_headers
    );
    writeln!(data_file, "{general_info}").context(format!("Failed to write header to {}", settings.convert.data_path))?;

    let custom_cols = (0..settings.process_cmd_to_search.len())
        .map(|idx| format!("CUSTOM_{idx}_CPU,CUSTOM_{idx}_MEMORY"))
        .collect::<Vec<_>>()
        .join(",");
    let custom_cols = if custom_cols.is_empty() {
        String::new()
    } else {
        format!(",{custom_cols}")
    };

    use crate::enums::DataType;
    let data_header = format!(
        "{},{}{}",
        DataType::SECONDS_SINCE_START,
        settings.collection_mode.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(","),
        custom_cols
    );
    writeln!(data_file, "{data_header}").context(format!("Failed to write column header to {}", settings.convert.data_path))?;

    if !settings.disable_instant_flushing {
        data_file.flush().context(format!("Failed to flush {}", settings.convert.data_path))?;
    }

    Ok(())
}

/// Rotate existing backup files and rename the current data file.
pub fn backup_old_file(settings: &CollectSettings) -> Result<(), Error> {
    if settings.backup_number == 0 {
        return Ok(());
    }
    let mut names = vec![];
    for i in 1..=settings.backup_number {
        names.push(insert_before_extension(&settings.convert.data_path, &format!("__{i}")));
    }

    let last = &names[names.len() - 1];
    if Path::new(last).exists() {
        fs::remove_file(last).context(format!("Failed to remove backup file {last}"))?;
    }

    for i in (0..names.len() - 1).rev() {
        if Path::new(&names[i]).exists() {
            fs::rename(&names[i], &names[i + 1]).context(format!("Failed to rename {} → {}", names[i], names[i + 1]))?;
        }
    }

    if Path::new(&settings.convert.data_path).exists() {
        fs::rename(&settings.convert.data_path, &names[0]).context(format!("Failed to rename data file to {}", names[0]))?;
    }

    info!("Backup files rotated successfully");
    Ok(())
}

fn insert_before_extension(path: &str, suffix: &str) -> String {
    if let Some(idx) = path.rfind('.') {
        let (base, ext) = path.split_at(idx);
        format!("{base}{suffix}{ext}")
    } else {
        format!("{path}{suffix}")
    }
}

/// Open / create the data CSV file.
pub fn open_data_file(settings: &CollectSettings) -> Result<BufWriter<File>, Error> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&settings.convert.data_path)
        .context(format!("Failed to open data file {}", settings.convert.data_path))?;
    Ok(BufWriter::new(file))
}
