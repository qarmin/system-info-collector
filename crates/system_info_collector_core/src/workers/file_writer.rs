use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// Format a float with at most 2 decimal places, stripping trailing zeros
/// (and the decimal point itself when not needed).
/// e.g. 0.0000 → "0", 17332.7700 → "17332.8", 1.01 → "1"
fn fmt_f64(v: f64) -> String {
    let s = format!("{v:.1}");
    if s.contains('.') {
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    } else {
        s
    }
}

use anyhow::{Context, Error};
use chrono::Utc;
use log::{error, info};
use sysinfo::{Disks, System};

use crate::discovery::RuntimeDiscovery;
use crate::enums::{DataType, HeaderValues, SimpleDataCollectionMode, network_rate_columns};
use crate::settings::CollectSettings;
use crate::shared_state::SharedState;
use crate::workers::sysinfo_worker::bytes_to_mb;

/// Spawned as a tokio task.  Wakes every `check_interval` seconds, clones the
/// latest snapshots out of `SharedState` (brief read-lock), formats a CSV row,
/// writes it to disk and calls `on_row` so the HTTP server can update its
/// in-memory buffer.
///
/// When `--top-n-processes` is active, `on_top_row` (if supplied) is also
/// called each tick with `(seconds_since_start, top_cpu, top_ram)` so the
/// HTTP server can maintain a live top-process history.
#[expect(clippy::type_complexity)]
pub async fn run<F>(
    settings: Arc<CollectSettings>,
    state: Arc<RwLock<SharedState>>,
    shutdown: Arc<AtomicBool>,
    mut data_file: BufWriter<File>,
    on_row: Arc<F>,
    discovery: Arc<RuntimeDiscovery>,
    csv_header: String,
    on_top_row: Option<Arc<dyn Fn(f64, Vec<(String, f32)>, Vec<(String, f64)>) + Send + Sync>>,
) where
    F: Fn(Vec<String>) + Send + Sync + 'static,
{
    let interval_ms = (settings.check_interval * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(100)));
    // consume the first instant tick
    interval.tick().await;

    let disks = &discovery.disks;

    // Initialize disk monitor if any disks are requested and disk space modes are active.
    let has_disk_space_modes = settings.collection_mode.iter().any(|m| m.is_disk_space());
    let mut sysinfo_disks = if disks.is_empty() || !has_disk_space_modes {
        None
    } else {
        Some(Disks::new_with_refreshed_list())
    };

    let mut collected_bytes: usize = 0;

    // Previous data-row values used for compact CSV encoding.
    // Index 0 = timestamp (always written), indices 1..N = data columns.
    let mut prev_row: Vec<String> = Vec::new();

    // Track last disk refresh time to honour disk_interval_secs.
    let mut last_disk_refresh: Option<Instant> = None;
    let disk_interval_ms = (settings.disk_interval_secs * 1000.0) as u128;

    // Open optional top-N process files.
    let mut top_cpu_file: Option<BufWriter<File>> = None;
    let mut top_ram_file: Option<BufWriter<File>> = None;
    if settings.top_n_processes > 0 {
        let n = settings.top_n_processes;
        let cpu_path = top_n_path(&settings.convert.data_path, "cpu");
        let ram_path = top_n_path(&settings.convert.data_path, "ram");
        match open_top_n_file(&cpu_path, "CPU", n, settings.start_time) {
            Ok(f) => top_cpu_file = Some(f),
            Err(e) => error!("Failed to open top-CPU file {cpu_path}: {e}"),
        }
        match open_top_n_file(&ram_path, "RAM", n, settings.start_time) {
            Ok(f) => top_ram_file = Some(f),
            Err(e) => error!("Failed to open top-RAM file {ram_path}: {e}"),
        }
    }

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
        let (sysinfo_snap, network_snaps, gpu_snaps, disk_io_snaps, process_snaps, top_cpu_snap, top_ram_snap) = {
            let guard = state.read().expect("SharedState RwLock poisoned");
            (
                guard.latest_sysinfo.clone(),
                guard.latest_networks.clone(),
                guard.latest_gpus.clone(),
                guard.latest_disk_io.clone(),
                guard.latest_processes.clone(),
                guard.latest_top_cpu.clone(),
                guard.latest_top_ram.clone(),
            )
        };

        // Refresh disk stats at disk_interval_secs rate.
        if let Some(ref mut sd) = sysinfo_disks {
            let should_refresh = last_disk_refresh.is_none_or(|t: Instant| t.elapsed().as_millis() >= disk_interval_ms);
            if should_refresh {
                sd.refresh(false);
                last_disk_refresh = Some(Instant::now());
            }
        }

        // Build the CSV row in the same column order as the header.
        let mut row: Vec<String> = Vec::with_capacity(16);
        row.push(fmt_f64(seconds_since_start));

        let has_rx = settings.collection_mode.contains(&SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC);
        let has_tx = settings.collection_mode.contains(&SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC);
        let mut network_rate_written = false;

        for mode in &settings.collection_mode {
            match mode {
                SimpleDataCollectionMode::CPU_USAGE_TOTAL => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| fmt_f64(s.cpu_usage_total)));
                }
                SimpleDataCollectionMode::CPU_USAGE_PER_CORE => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| {
                        s.cpu_usage_per_core.iter().map(|v| fmt_f64(*v)).collect::<Vec<_>>().join(";")
                    }));
                }
                SimpleDataCollectionMode::MEMORY_USED => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| fmt_f64(s.memory_used_mb)));
                }
                SimpleDataCollectionMode::MEMORY_FREE => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| fmt_f64(s.memory_free_mb)));
                }
                SimpleDataCollectionMode::MEMORY_AVAILABLE => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| fmt_f64(s.memory_available_mb)));
                }
                SimpleDataCollectionMode::SWAP_USED => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| fmt_f64(s.swap_used_mb)));
                }
                SimpleDataCollectionMode::SWAP_FREE => {
                    row.push(sysinfo_snap.as_ref().map_or("-1".to_string(), |s| fmt_f64(s.swap_free_mb)));
                }
                // Network rate modes expand to one column per discovered interface,
                // interleaved rx/tx per interface (see `network_rate_columns`).
                // Values are written in MB/s (bytes ÷ 1 048 576) for readability.
                SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC | SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC => {
                    if !network_rate_written {
                        network_rate_written = true;
                        for snap in &network_snaps {
                            if has_rx {
                                row.push(snap.as_ref().map_or("-1".to_string(), |n| fmt_f64(n.rx_bytes_per_sec / 1_048_576.0)));
                            }
                            if has_tx {
                                row.push(snap.as_ref().map_or("-1".to_string(), |n| fmt_f64(n.tx_bytes_per_sec / 1_048_576.0)));
                            }
                        }
                    }
                }
                // Cumulative RX/TX totals since interface up, in MB - own chart, separate from the rate above.
                SimpleDataCollectionMode::NETWORK_TOTAL => {
                    for snap in &network_snaps {
                        match snap {
                            Some(n) => {
                                row.push(fmt_f64(n.total_rx_bytes as f64 / 1_048_576.0));
                                row.push(fmt_f64(n.total_tx_bytes as f64 / 1_048_576.0));
                            }
                            None => {
                                row.push("-1".to_string());
                                row.push("-1".to_string());
                            }
                        }
                    }
                }
                // GPU modes expand to one column per discovered GPU.
                SimpleDataCollectionMode::GPU_UTILIZATION => {
                    for snap in &gpu_snaps {
                        row.push(snap.as_ref().map_or("-1".to_string(), |g| g.utilization_gpu.to_string()));
                    }
                }
                SimpleDataCollectionMode::GPU_MEMORY_USED => {
                    for snap in &gpu_snaps {
                        row.push(snap.as_ref().map_or("-1".to_string(), |g| g.memory_used_mb.to_string()));
                    }
                }
                SimpleDataCollectionMode::GPU_TEMPERATURE => {
                    for snap in &gpu_snaps {
                        row.push(snap.as_ref().map_or("-1".to_string(), |g| g.temperature.to_string()));
                    }
                }
                // Disk modes expand to one column per tracked disk.
                SimpleDataCollectionMode::DISK_USED => {
                    for disk_entry in disks {
                        let val = sysinfo_disks
                            .as_ref()
                            .and_then(|sd| sd.iter().find(|d| d.mount_point().to_string_lossy() == disk_entry.mount_point.as_str()));
                        match val {
                            Some(d) => {
                                let used = d.total_space().saturating_sub(d.available_space());
                                row.push((used / 1_073_741_824).to_string());
                            }
                            None => row.push("-1".to_string()),
                        }
                    }
                }
                SimpleDataCollectionMode::DISK_AVAILABLE => {
                    for disk_entry in disks {
                        let val = sysinfo_disks
                            .as_ref()
                            .and_then(|sd| sd.iter().find(|d| d.mount_point().to_string_lossy() == disk_entry.mount_point.as_str()));
                        match val {
                            Some(d) => row.push((d.available_space() / 1_073_741_824).to_string()),
                            None => row.push("-1".to_string()),
                        }
                    }
                }
                // Disk I/O modes expand to one column per tracked disk, fed by disk_io_worker.
                SimpleDataCollectionMode::DISK_BUSY => {
                    for snap in &disk_io_snaps {
                        row.push(snap.as_ref().map_or("-1".to_string(), |d| fmt_f64(d.busy_pct)));
                    }
                }
                SimpleDataCollectionMode::DISK_READ => {
                    for snap in &disk_io_snaps {
                        row.push(snap.as_ref().map_or("-1".to_string(), |d| fmt_f64(d.read_mb_per_sec)));
                    }
                }
                SimpleDataCollectionMode::DISK_WRITE => {
                    for snap in &disk_io_snaps {
                        row.push(snap.as_ref().map_or("-1".to_string(), |d| fmt_f64(d.write_mb_per_sec)));
                    }
                }
            }
        }

        // Custom process columns (two per pattern: CPU%, memory MB)
        for proc_opt in &process_snaps {
            if let Some(p) = proc_opt {
                row.push(fmt_f64(p.cpu_usage as f64));
                row.push(fmt_f64(p.memory_mb));
            } else {
                row.push("-1".to_string());
                row.push("-1".to_string());
            }
        }

        // In compact mode, replace data values that haven't changed with empty strings.
        // The timestamp (index 0) is always written in full.
        // `on_row` always receives the full row so the HTTP server has complete data.
        let row_str = if settings.compact_csv && !prev_row.is_empty() {
            let compact: Vec<&str> = row
                .iter()
                .enumerate()
                .map(|(i, v)| if i == 0 || (prev_row.get(i) != Some(v)) { v.as_str() } else { "" })
                .collect();
            compact.join(",")
        } else {
            row.join(",")
        };

        // Update the previous row with the current full values.
        if settings.compact_csv {
            prev_row.clone_from(&row);
        }

        collected_bytes += row_str.len();

        if collected_bytes >= settings.maximum_data_file_size_bytes {
            info!(
                "Data file reached size limit ({}), rotating to a new file",
                humansize::format_size(settings.maximum_data_file_size_bytes, humansize::BINARY)
            );
            match rotate_data_file(&mut data_file, &settings.convert.data_path, &csv_header) {
                Ok(new_file) => {
                    data_file = new_file;
                    collected_bytes = 0;
                    prev_row.clear();
                }
                Err(e) => {
                    error!("Failed to rotate data file: {e}, stopping collection");
                    shutdown.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        if let Err(e) = writeln!(data_file, "{row_str}").context(format!("Failed to write to {}", settings.convert.data_path)) {
            error!("{e}");
            shutdown.store(true, Ordering::Relaxed);
            break;
        }

        if !settings.disable_instant_flushing
            && let Err(e) = data_file.flush().context(format!("Failed to flush {}", settings.convert.data_path))
        {
            error!("{e}");
            shutdown.store(true, Ordering::Relaxed);
            break;
        }

        // Write top-N rows (best-effort: errors are logged but don't stop collection).
        if settings.top_n_processes > 0 {
            let n = settings.top_n_processes;
            if let Some(ref mut f) = top_cpu_file {
                write_top_n_row(f, seconds_since_start, &top_cpu_snap, n, !settings.disable_instant_flushing);
            }
            if let Some(ref mut f) = top_ram_file {
                let ram_as_f32: Vec<(String, f32)> = top_ram_snap.iter().map(|(name, v)| (name.clone(), *v as f32)).collect();
                write_top_n_row(f, seconds_since_start, &ram_as_f32, n, !settings.disable_instant_flushing);
            }
            if let Some(ref cb) = on_top_row {
                cb(seconds_since_start, top_cpu_snap.clone(), top_ram_snap.clone());
            }
        }

        on_row(row);
    }

    info!("file_writer stopped");
}

/// Write the two-line CSV header (metadata line + column-name line).
/// Requires an initial `System` refresh for memory / CPU metadata.
/// Returns the header content so it can be reused when rotating files.
pub fn write_csv_header(
    data_file: &mut BufWriter<File>,
    sys: &System,
    settings: &CollectSettings,
    discovery: &RuntimeDiscovery,
    app_version: &str,
) -> Result<String, Error> {
    let disks = &discovery.disks;
    // Custom process metadata entries: CUSTOM_0=NAME, CUSTOM_1=NAME, …
    let custom_meta: String = settings
        .process_cmd_to_search
        .iter()
        .enumerate()
        .map(|(idx, e)| format!(",CUSTOM_{idx}={}", e.graph_name))
        .collect();

    // GPU metadata entries: GPU_0=NAME, GPU_1=NAME, …
    let gpu_meta: String = discovery
        .gpus
        .iter()
        .map(|g| format!(",GPU_{}={}", g.gpu_index, g.display_name()))
        .collect();

    // GPU VRAM metadata: GPU_VRAM_0=MB, GPU_VRAM_1=MB, …
    let gpu_vram_meta: String = discovery
        .gpus
        .iter()
        .filter(|g| g.vram_total_mb > 0)
        .map(|g| format!(",GPU_VRAM_{}={}", g.gpu_index, g.vram_total_mb))
        .collect();

    // CPU model from first CPU (commas replaced to avoid breaking the CSV metadata line).
    let cpu_model_meta = sys
        .cpus()
        .first()
        .map(|c| format!(",CPU_MODEL={}", c.brand().replace(',', " ")))
        .unwrap_or_default();

    // Network interface metadata entries: NET_0=eth0, NET_1=wlan0, …
    let net_meta: String = discovery
        .interfaces
        .iter()
        .map(|i| format!(",NET_{}={}", i.iface_index, i.name))
        .collect();

    // Disk metadata entries: DISK_0=/,DISK_1=/home,…
    let disk_meta: String = disks.iter().map(|d| format!(",DISK_{}={}", d.disk_index, d.mount_point)).collect();

    let mem_total = bytes_to_mb(sys.total_memory());
    let swap_total = bytes_to_mb(sys.total_swap());
    let general_info = format!(
        "{}={},{}={},{}={mem_total:.2},{}={swap_total:.2},{}={},{}={}{}{}{}{}{}{}",
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
        custom_meta,
        gpu_meta,
        net_meta,
        cpu_model_meta,
        gpu_vram_meta,
        disk_meta,
    );
    // Column header line.
    // GPU/network modes expand to one column per discovered GPU/interface.
    let mut columns: Vec<String> = vec![DataType::SECONDS_SINCE_START.column_name()];

    let has_rx = settings.collection_mode.contains(&SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC);
    let has_tx = settings.collection_mode.contains(&SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC);
    let mut network_rate_emitted = false;

    for mode in &settings.collection_mode {
        match mode {
            SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC | SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC => {
                if !network_rate_emitted {
                    network_rate_emitted = true;
                    let interfaces = discovery.interfaces.iter().map(|iface| (iface.iface_index, iface.name.clone()));
                    for column in network_rate_columns(has_rx, has_tx, interfaces) {
                        columns.push(column.column_name());
                    }
                }
            }
            SimpleDataCollectionMode::NETWORK_TOTAL => {
                for iface in &discovery.interfaces {
                    columns.push(DataType::NET_N_RX_TOTAL_MB((iface.iface_index, iface.name.clone())).column_name());
                    columns.push(DataType::NET_N_TX_TOTAL_MB((iface.iface_index, iface.name.clone())).column_name());
                }
            }
            SimpleDataCollectionMode::GPU_UTILIZATION => {
                for gpu in &discovery.gpus {
                    columns.push(DataType::GPU_N_UTIL((gpu.gpu_index, gpu.display_name().to_string())).column_name());
                }
            }
            SimpleDataCollectionMode::GPU_MEMORY_USED => {
                for gpu in &discovery.gpus {
                    columns.push(DataType::GPU_N_VRAM_MB((gpu.gpu_index, gpu.display_name().to_string())).column_name());
                }
            }
            SimpleDataCollectionMode::GPU_TEMPERATURE => {
                for gpu in &discovery.gpus {
                    columns.push(DataType::GPU_N_TEMP_C((gpu.gpu_index, gpu.display_name().to_string())).column_name());
                }
            }
            SimpleDataCollectionMode::DISK_USED => {
                for disk in disks {
                    columns.push(DataType::DISK_N_USED_GB((disk.disk_index, disk.mount_point.clone())).column_name());
                }
            }
            SimpleDataCollectionMode::DISK_AVAILABLE => {
                for disk in disks {
                    columns.push(DataType::DISK_N_AVAIL_GB((disk.disk_index, disk.mount_point.clone())).column_name());
                }
            }
            SimpleDataCollectionMode::DISK_BUSY => {
                for disk in disks {
                    columns.push(DataType::DISK_N_BUSY_PCT((disk.disk_index, disk.mount_point.clone())).column_name());
                }
            }
            SimpleDataCollectionMode::DISK_READ => {
                for disk in disks {
                    columns.push(DataType::DISK_N_READ_MBPS((disk.disk_index, disk.mount_point.clone())).column_name());
                }
            }
            SimpleDataCollectionMode::DISK_WRITE => {
                for disk in disks {
                    columns.push(DataType::DISK_N_WRITE_MBPS((disk.disk_index, disk.mount_point.clone())).column_name());
                }
            }
            other => columns.push(other.to_string()),
        }
    }

    // Custom process columns (two per pattern: CPU%, memory MB)
    for (idx, _) in settings.process_cmd_to_search.iter().enumerate() {
        columns.push(format!("CUSTOM_{idx}_CPU"));
        columns.push(format!("CUSTOM_{idx}_MEMORY"));
    }

    let header = format!("{general_info}\n{}\n", columns.join(","));
    write!(data_file, "{header}").context(format!("Failed to write header to {}", settings.convert.data_path))?;

    if !settings.disable_instant_flushing {
        data_file.flush().context(format!("Failed to flush {}", settings.convert.data_path))?;
    }

    Ok(header)
}

/// Flush the current data file, rename it with a UTC datetime stamp, clean up
/// old rotated files (keep at most `MAX_ROTATED_FILES`), open a fresh file at
/// the original path, write the CSV header, and return the new `BufWriter`.
fn rotate_data_file(data_file: &mut BufWriter<File>, data_path: &str, csv_header: &str) -> Result<BufWriter<File>, Error> {
    const MAX_ROTATED_FILES: usize = 10;

    data_file.flush().context("Failed to flush data file before rotation")?;

    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let rotated_path = insert_before_extension(data_path, &format!("_{timestamp}"));
    fs::rename(data_path, &rotated_path).context(format!("Failed to rename {data_path} to {rotated_path}"))?;
    info!("Rotated data file to {rotated_path}");

    cleanup_rotated_files(data_path, MAX_ROTATED_FILES);

    let mut writer = open_data_file_at(data_path)?;
    write!(writer, "{csv_header}").context(format!("Failed to write header to new file {data_path}"))?;
    writer.flush().context(format!("Failed to flush new file {data_path}"))?;

    Ok(writer)
}

/// Delete the oldest rotated files so at most `max_count` remain.
/// Rotated files are named `{base}_{YYYY-MM-DD_HH-MM-SS}{ext}`.
fn cleanup_rotated_files(data_path: &str, max_count: usize) {
    let path = Path::new(data_path);
    let dir = path.parent().unwrap_or(Path::new("."));
    let base = match path.file_stem() {
        Some(s) => s.to_string_lossy().into_owned(),
        None => return,
    };
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();

    let mut rotated: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_rotated_filename(&name, &base, &ext) { Some(e.path()) } else { None }
        })
        .collect();

    rotated.sort();

    while rotated.len() > max_count {
        let oldest = rotated.remove(0);
        if let Err(e) = fs::remove_file(&oldest) {
            error!("Failed to remove old rotated file {}: {e}", oldest.display());
        } else {
            info!("Removed old rotated file {}", oldest.display());
        }
    }
}

/// Returns `true` if `file_name` looks like a rotated file for the given `base` and `ext`.
/// Expected middle part: `_YYYY-MM-DD_HH-MM-SS` (20 chars including the leading underscore).
pub fn is_rotated_filename(file_name: &str, base: &str, ext: &str) -> bool {
    let prefix = format!("{base}_");
    if !file_name.starts_with(&prefix) || !file_name.ends_with(ext) {
        return false;
    }
    #[expect(clippy::string_slice)]
    let middle = &file_name[prefix.len()..file_name.len() - ext.len()];
    // middle must be exactly "YYYY-MM-DD_HH-MM-SS" (19 chars)
    middle.len() == 19
        && middle.chars().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => c == '-',
            10 => c == '_',
            _ => c.is_ascii_digit(),
        })
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
    open_data_file_at(&settings.convert.data_path)
}

fn open_data_file_at(path: &str) -> Result<BufWriter<File>, Error> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .context(format!("Failed to open data file {path}"))?;
    Ok(BufWriter::new(file))
}

// ── Top-N process file helpers ────────────────────────────────────────────────

/// Derive the path for a top-N file from the main data path.
/// e.g. `system_data.csv` → `system_data_top_cpu.csv`
pub fn top_n_path(data_path: &str, kind: &str) -> String {
    insert_before_extension(data_path, &format!("_top_{kind}"))
}

/// Open and write the two-line header for a top-N process file.
/// Format:
///   Line 1: `START_TIME=xxx,TOP_N=5,TYPE=CPU`
///   Line 2: `TIMESTAMP,1,2,3,4,5`
fn open_top_n_file(path: &str, type_tag: &str, n: usize, start_time: f64) -> Result<BufWriter<File>, Error> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .context(format!("Failed to open top-N file {path}"))?;
    let mut writer = BufWriter::new(file);

    // Metadata line
    writeln!(writer, "START_TIME={start_time},TOP_N={n},TYPE={type_tag}").context(format!("Failed to write header to {path}"))?;

    // Column header: TIMESTAMP,1,2,...,N
    let cols: Vec<String> = std::iter::once("TIMESTAMP".to_string()).chain((1..=n).map(|i| i.to_string())).collect();
    writeln!(writer, "{}", cols.join(",")).context(format!("Failed to write column header to {path}"))?;

    writer.flush().context(format!("Failed to flush {path}"))?;
    Ok(writer)
}

/// Write one data row to a top-N file.
/// Pads with empty entries if fewer than `n` processes are present.
fn write_top_n_row(file: &mut BufWriter<File>, timestamp: f64, entries: &[(String, f32)], n: usize, flush: bool) {
    let mut cols: Vec<String> = Vec::with_capacity(n + 1);
    cols.push(fmt_f64(timestamp));
    for i in 0..n {
        if let Some((name, val)) = entries.get(i) {
            cols.push(format!("{name}|{}", fmt_f64(*val as f64)));
        } else {
            cols.push(String::new());
        }
    }
    if let Err(e) = writeln!(file, "{}", cols.join(",")) {
        error!("Failed to write top-N row: {e}");
        return;
    }
    if flush && let Err(e) = file.flush() {
        error!("Failed to flush top-N file: {e}");
    }
}
