use std::collections::HashSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Error};
use crossbeam_channel::unbounded;
use log::{debug, info};
use sysinfo::{CpuExt, Pid, ProcessExt, ProcessRefreshKind, System, SystemExt};
use tokio::time::interval;

use crate::enums::{DataType, HeaderValues, SimpleDataCollectionMode};
use crate::model::{CustomProcessData, ProcessCache, Settings};
use crate::ploty_creator::load_results_and_save_plot;
use crate::set_ctrl_c_handler;

pub async fn collect_data(sys: &mut System, settings: &Settings) -> Result<(), Error> {
    backup_old_file(settings)?;

    let data_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&settings.data_path)
        .context(format!("Failed to open data file {}", settings.data_path))?;
    let mut data_file = BufWriter::new(data_file);
    write_header_into_file(sys, &mut data_file, settings)?;

    let mut interv = interval(Duration::from_millis((settings.check_interval * 1000.0) as u64));
    interv.tick().await; // This will instantly finish, so next time will take required amount of seconds

    let (ctx, crx) = unbounded::<()>();
    set_ctrl_c_handler(ctx);

    let mut collected_bytes = 0;
    let mut process_cache_data = ProcessCache::new_with_size(settings.process_cmd_to_search.len(), sys);

    info!("Started collecting data...");
    loop {
        collect_and_save_data(sys, &mut data_file, settings, &mut collected_bytes, &mut process_cache_data)?;

        if crx.try_recv().is_ok() {
            drop(data_file);
            if settings.app_mode == crate::enums::AppMode::COLLECT_AND_CONVERT {
                load_results_and_save_plot(settings)?;
            }
            return Ok(());
        }

        interv.tick().await;
    }
}

// Function to create
fn backup_old_file(settings: &Settings) -> Result<(), Error> {
    if settings.backup_number == 0 {
        return Ok(()); // No backup required
    }
    let mut backup_file_names = vec![];
    for i in 1..=settings.backup_number {
        backup_file_names.push(format_new_name(&settings.data_path, &format!("__{i}")));
    }

    // Remove last backup file
    let last_file_name = backup_file_names[backup_file_names.len() - 1].clone();
    if Path::new(&last_file_name).exists() {
        fs::remove_file(&last_file_name).context(format!(
            "Failed to remove backup file {}",
            &backup_file_names[backup_file_names.len() - 1]
        ))?;
    }

    // Rename all backup files
    for i in (0..backup_file_names.len() - 1).rev() {
        let old_file_name = backup_file_names[i].clone();
        if Path::new(&old_file_name).exists() {
            let new_file_name = backup_file_names[i + 1].clone();
            fs::rename(&old_file_name, &new_file_name).context(format!("Failed to rename backup file {} into {}", &old_file_name, &new_file_name))?;
        }
    }

    // Rename current file into first backup file name
    if Path::new(&settings.data_path).exists() {
        fs::rename(&settings.data_path, &backup_file_names[0]).context(format!(
            "Failed to rename data file {} into {}",
            &settings.data_path, &backup_file_names[0]
        ))?;
    }

    info!("Backup files renamed successfully");

    Ok(())
}

fn format_new_name(file_path: &str, item_to_add: &str) -> String {
    if let Some(index) = file_path.rfind('.') {
        let (base, extension) = file_path.split_at(index);
        format!("{base}{item_to_add}{extension}")
    } else {
        format!("{file_path}{item_to_add}")
    }
}

fn write_header_into_file(sys: &mut System, data_file: &mut BufWriter<std::fs::File>, settings: &Settings) -> Result<(), Error> {
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

    let general_info = format!(
        "{}={},{}={},{}={},{}={},{}={}{}",
        HeaderValues::INTERVAL_SECONDS,
        settings.check_interval,
        HeaderValues::CPU_CORE_COUNT,
        sys.cpus().len(),
        HeaderValues::MEMORY_TOTAL,
        convert_into_string_megabytes(sys.total_memory()),
        HeaderValues::SWAP_TOTAL,
        convert_into_string_megabytes(sys.total_swap()),
        HeaderValues::APP_VERSION,
        env!("CARGO_PKG_VERSION"),
        custom_headers
    );
    writeln!(data_file, "{general_info}").context(format!("Failed to write general into data file {}", settings.data_path))?;

    let custom_columns = (0..settings.process_cmd_to_search.len())
        .map(|idx| format!("CUSTOM_{idx}_CPU,CUSTOM_{idx}_MEMORY"))
        .collect::<Vec<_>>()
        .join(",");
    let custom_columns = if custom_columns.is_empty() {
        String::new()
    } else {
        format!(",{custom_columns}")
    };

    let data_header = format!(
        "{},{}{custom_columns}",
        DataType::UNIX_TIMESTAMP,
        settings
            .collection_mode
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
            .join(",")
    );
    writeln!(data_file, "{data_header}").context(format!("Failed to write header into data file {}", settings.data_path))?;

    if !settings.disable_instant_flushing {
        data_file.flush().context(format!("Failed to flush data file {}", settings.data_path))?;
    }

    Ok(())
}

fn collect_and_save_data(
    sys: &mut System,
    data_file: &mut BufWriter<fs::File>,
    settings: &Settings,
    collected_bytes: &mut usize,
    process_cache_data: &mut ProcessCache,
) -> Result<(), Error> {
    let current_time = SystemTime::now();

    let start = SystemTime::now();
    sys.refresh_cpu();
    sys.refresh_memory();

    if settings.need_to_refresh_processes {
        check_for_new_and_old_process_data(sys, process_cache_data, settings)?;
    }

    debug!("Refreshed app/os usage data in {:?}", start.elapsed().unwrap());

    let mut data_to_save = vec![];

    // UNIX_TIMESTAMP - always required
    data_to_save.push(format!(
        "{:.2}",
        current_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64()
    ));

    for i in &settings.collection_mode {
        let collected_string = match i {
            SimpleDataCollectionMode::MEMORY_USED => convert_into_string_megabytes(sys.used_memory()),
            SimpleDataCollectionMode::MEMORY_AVAILABLE => convert_into_string_megabytes(sys.available_memory()),
            SimpleDataCollectionMode::MEMORY_FREE => convert_into_string_megabytes(sys.free_memory()),
            SimpleDataCollectionMode::CPU_USAGE_TOTAL => {
                format!(
                    "{:.2}",
                    sys.cpus().iter().map(sysinfo::CpuExt::cpu_usage).sum::<f32>() / sys.cpus().len() as f32
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

    let data_to_save_str = data_to_save.join(",");
    *collected_bytes += data_to_save_str.len();

    if *collected_bytes >= settings.maximum_data_file_size_bytes {
        let _ = data_file.flush();
        return Err(Error::msg(format!(
            "Exceeded allowed data size - {}, consider to increase size limit, decrease interval or amount of logged data",
            humansize::format_size(settings.maximum_data_file_size_bytes, humansize::BINARY)
        )));
    }

    writeln!(data_file, "{data_to_save_str}").context(format!("Failed to write data into data file {}", settings.data_path))?;

    if !settings.disable_instant_flushing {
        data_file.flush().context(format!("Failed to flush data file {}", settings.data_path))?;
    }

    Ok(())
}

// Sys-info not have enough fast to check for available processes
// In this step I don't need any info except running process pids
pub fn get_system_pids() -> Result<HashSet<usize>, Error> {
    let Ok(entries) = fs::read_dir("/proc") else  {
        return Err(Error::msg("Failed to read /proc directory"));
    };

    let mut pids = HashSet::new();
    for entry in entries.flatten() {
        if let Ok(file_type) = entry.file_type() {
            if !file_type.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(pid) = name.parse::<usize>() {
                    pids.insert(pid);
                }
            }
        }
    }

    Ok(pids)
}
// Algorithm:
// 1. Get all system pids
// 2. Check for new processes and update them if are > 30, then use sys.refresh_processes_specific, to update them all in batch(probably cheaper than updating one by one)

pub fn check_for_new_and_old_process_data(sys: &mut System, process_cache_data: &mut ProcessCache, settings: &Settings) -> Result<(), Error> {
    let system_pids = get_system_pids()?;

    // If all searched processes are tracked, then app don't need to check for new processes
    // Only update used
    // This save a lot of processing time
    if process_cache_data
        .process_used
        .iter()
        .all(|e| e.is_some() && system_pids.contains(&e.as_ref().unwrap().pid))
    {
        update_usage_of_tracked_process(process_cache_data, sys);
        return Ok(());
    }

    update_new_processes_stats(process_cache_data, sys, &system_pids);
    remove_tracking_of_removed_processes(process_cache_data, &system_pids);
    check_which_process_to_track(process_cache_data, sys, settings, &system_pids);

    update_usage_of_tracked_process(process_cache_data, sys);

    process_cache_data.replace_checked_to_be_used_processes(system_pids.iter());

    Ok(())
}

fn check_which_process_to_track(process_cache_data: &mut ProcessCache, sys: &mut System, settings: &Settings, system_pids: &HashSet<usize>) {
    for (idx, i) in settings.process_cmd_to_search.iter().enumerate() {
        if process_cache_data.process_used[idx].is_some() {
            // Already monitoring process from such name
            continue;
        }

        let mut shortest_matching_process_data = None;
        let mut shortest_text = usize::MAX;

        for (pid, process) in sys.processes() {
            let pid_number: usize = (*pid).into();
            if process_cache_data.processes_checked_to_be_used.contains(&pid_number) || !system_pids.contains(&pid_number) {
                continue;
            }
            let collected_name = process.cmd().join(" ");
            if collected_name.contains(&i.search_text) && collected_name.len() < shortest_text {
                shortest_text = collected_name.len();
                shortest_matching_process_data = Some((pid_number, process, collected_name));
            }
        }

        if let Some((pid_number, process, collected_name)) = shortest_matching_process_data {
            info!(
                "Found process \"{}\" with pid \"{}\" that will be monitored - (\"{}\")",
                process.name(),
                pid_number,
                collected_name,
            );
            process_cache_data.processes_checked_to_be_used.insert(pid_number);
            process_cache_data.process_used[idx] = Some(CustomProcessData::from_process(process));
        }
    }
}

fn remove_tracking_of_removed_processes(process_cache_data: &mut ProcessCache, system_pids: &HashSet<usize>) {
    process_cache_data.process_used = process_cache_data
        .process_used
        .clone()
        .into_iter()
        .map(|e| {
            if let Some(e) = e {
                if system_pids.contains(&e.pid) {
                    Some(e)
                } else {
                    info!(
                        "Process \"{}\" with pid \"{}\" is no longer available, removing from monitoring - (\"{}\")",
                        e.name, e.pid, e.cmd_string
                    );
                    None
                }
            } else {
                None
            }
        })
        .collect();
}

// Needed to get processes name and cmd, rest is updated in update_usage_of_tracked_process
fn update_new_processes_stats(process_cache_data: &mut ProcessCache, sys: &mut System, system_pids: &HashSet<usize>) {
    let new_processes = process_cache_data.get_differences_in_usage_processes(system_pids.iter());

    if !new_processes.is_empty() {
        if new_processes.len() > 40 {
            info!("Found {} new processes, refreshing them in batch", new_processes.len());
            sys.refresh_processes_specifics(ProcessRefreshKind::new().with_cpu());
        } else {
            info!("Found {} new processes, refreshing them one by one", new_processes.len());
            for i in new_processes {
                sys.refresh_process_specifics(Pid::from(i), ProcessRefreshKind::new().with_cpu());
            }
        }
    }
    process_cache_data.replace_checked_usage_processes(system_pids.iter());
}

fn update_usage_of_tracked_process(process_cache_data: &mut ProcessCache, sys: &mut System) {
    let process_count = process_cache_data.process_used.iter().flatten().count();
    if process_count == 0 {
        return;
    }
    debug!("Updating data of {} processes", process_cache_data.process_used.iter().flatten().count());
    for custom_process in process_cache_data.process_used.iter_mut().flatten() {
        sys.refresh_process_specifics(Pid::from(custom_process.pid), ProcessRefreshKind::new().with_cpu());
        let Some(process) = sys.processes().get(&Pid::from(custom_process.pid))  else {
            continue; // Process was removed since we last checked
        };
        custom_process.memory_usage = process.memory();
        custom_process.cpu_usage = process.cpu_usage();
    }
}

// Only track changes > 10 KB
pub fn convert_into_string_megabytes(bytes: u64) -> String {
    format!("{:.2}", convert_bytes_into_mega_bytes(bytes))
}

pub fn convert_bytes_into_mega_bytes(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}
