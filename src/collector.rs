use std::fs;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Error};
use crossbeam_channel::unbounded;
use log::{debug, info};
use sysinfo::{CpuExt, System, SystemExt};
use tokio::time::interval;

use crate::enums::{DataCollectionMode, DataType, HeaderValues};
use crate::model::Settings;
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

    info!("Started collecting data...");
    loop {
        save_system_info_to_file(sys, &mut data_file, settings, &mut collected_bytes)?;

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
    let general_info = format!(
        "{}={},{}={},{}={}",
        HeaderValues::INTERVAL_SECONDS,
        settings.check_interval,
        HeaderValues::CPU_CORE_COUNT,
        sys.cpus().len(),
        HeaderValues::MEMORY_TOTAL,
        convert_bytes_into_mega_bytes(sys.total_memory())
    );
    writeln!(data_file, "{general_info}").context(format!("Failed to write general into data file {}", settings.data_path))?;

    let data_header = format!(
        "{},{}",
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

fn save_system_info_to_file(
    sys: &mut System,
    data_file: &mut BufWriter<fs::File>,
    settings: &Settings,
    collected_bytes: &mut usize,
) -> Result<(), Error> {
    let current_time = SystemTime::now();

    let start = SystemTime::now();
    sys.refresh_cpu();
    sys.refresh_memory();
    // sys.refresh_processes();
    debug!("Refreshed CPU and memory in {:?}", start.elapsed().unwrap());

    let mut data_to_save = vec![];

    // UNIX_TIMESTAMP - always required
    data_to_save.push(current_time.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64().to_string());

    for i in &settings.collection_mode {
        let collected_string = match i {
            DataCollectionMode::MEMORY_USED => convert_bytes_into_mega_bytes(sys.used_memory()).to_string(),
            DataCollectionMode::MEMORY_AVAILABLE => convert_bytes_into_mega_bytes(sys.available_memory()).to_string(),
            DataCollectionMode::MEMORY_FREE => convert_bytes_into_mega_bytes(sys.free_memory()).to_string(),
            DataCollectionMode::CPU_USAGE_TOTAL => {
                format!(
                    "{:.2}",
                    sys.cpus().iter().map(sysinfo::CpuExt::cpu_usage).sum::<f32>() / sys.cpus().len() as f32
                )
            }
            DataCollectionMode::CPU_USAGE_PER_CORE => sys.cpus().iter().map(|e| format!("{:.2}", e.cpu_usage())).collect::<Vec<_>>().join(";"),
        };
        data_to_save.push(collected_string);
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

pub fn convert_bytes_into_mega_bytes(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}
