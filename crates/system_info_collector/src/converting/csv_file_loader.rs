use std::collections::HashMap;
use std::fs::{metadata, File};
use std::io::{BufRead, BufReader, Lines};

use anyhow::{Context, Error, Result};
use log::info;
use system_info_collector_core::enums::{DataType, GeneralInfoGroup, HeaderValues};
use system_info_collector_core::model::{CollectedItemModels, TopProcessData};
use system_info_collector_core::settings::ConvertSettings;

pub fn load_csv_results(settings: &ConvertSettings) -> Result<CollectedItemModels, Error> {
    info!(
        "Data csv file is {} in size",
        humansize::format_size(
            metadata(&settings.data_path).context("Failed to get metadata of data file")?.len(),
            humansize::BINARY,
        )
    );

    let data_file = File::open(&settings.data_path).context(format!("Failed to open data file {}", &settings.data_path))?;
    let data_file = BufReader::new(data_file);

    let mut lines_iter = data_file.lines();

    let (swap_total, memory_total, cpu_core_count, check_interval, hashmap_data, start_time) = parse_file_values_data(&mut lines_iter)?;

    // Extract GPU names from metadata map (GPU_0=name, GPU_1=name, …).
    let mut gpu_name_entries: Vec<(usize, String)> = hashmap_data
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("GPU_").and_then(|n| n.parse::<usize>().ok()).map(|idx| (idx, v.clone())))
        .collect();
    gpu_name_entries.sort_by_key(|(idx, _)| *idx);
    let gpu_names: Vec<String> = gpu_name_entries.into_iter().map(|(_, name)| name).collect();

    // Extract GPU VRAM totals (GPU_VRAM_0=MB, …).
    let mut gpu_vram_entries: Vec<(usize, u64)> = hashmap_data
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("GPU_VRAM_")
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|idx| v.parse::<u64>().ok().map(|mb| (idx, mb)))
        })
        .collect();
    gpu_vram_entries.sort_by_key(|(idx, _)| *idx);
    let gpu_vram_mb: Vec<u64> = gpu_vram_entries.into_iter().map(|(_, mb)| mb).collect();

    // CPU model string (absent in old CSV files).
    let cpu_model = hashmap_data.get("CPU_MODEL").cloned().unwrap_or_default();

    // Extract disk mount points (DISK_0=path, DISK_1=path, …).
    let mut disk_name_entries: Vec<(usize, String)> = hashmap_data
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("DISK_").and_then(|n| n.parse::<usize>().ok()).map(|idx| (idx, v.clone())))
        .collect();
    disk_name_entries.sort_by_key(|(idx, _)| *idx);
    let disk_names: Vec<String> = disk_name_entries.into_iter().map(|(_, name)| name).collect();

    let (collected_data_names, collected_groups) = parse_header(&mut lines_iter, &hashmap_data)?;
    let collected_data = parse_data(&mut lines_iter, &collected_data_names, cpu_core_count)?;

    // Load optional extra data files (top-N process files).
    let mut top_cpu_processes: Option<TopProcessData> = None;
    let mut top_ram_processes: Option<TopProcessData> = None;

    for extra_path in &settings.extra_data_paths {
        match load_top_process_file(extra_path) {
            Ok((kind, data)) => {
                info!("Loaded top-{kind} process file: {extra_path}");
                if kind == "CPU" {
                    top_cpu_processes = Some(data);
                } else {
                    top_ram_processes = Some(data);
                }
            }
            Err(e) => {
                log::warn!("Failed to load extra data file {extra_path}: {e}");
            }
        }
    }

    Ok(CollectedItemModels {
        collected_data,
        collected_groups,
        memory_total,
        swap_total,
        cpu_core_count,
        check_interval,
        start_time,
        cpu_model,
        gpu_names,
        gpu_vram_mb,
        disk_names,
        top_cpu_processes,
        top_ram_processes,
    })
}

/// Load a top-N process file.  Returns `(type_tag, data)` where type_tag is "CPU" or "RAM".
pub fn load_top_process_file(path: &str) -> Result<(String, TopProcessData), Error> {
    let file = File::open(path).context(format!("Failed to open top-N file {path}"))?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Metadata line: START_TIME=xxx,TOP_N=5,TYPE=CPU
    let meta_line = lines.next().context("Missing metadata line")?.context("Failed to read metadata line")?;
    let mut meta: HashMap<String, String> = HashMap::new();
    for kv in meta_line.split(',') {
        let mut parts = kv.splitn(2, '=');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            meta.insert(k.to_string(), v.to_string());
        }
    }

    let start_time: f64 = meta.get("START_TIME").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let n: usize = meta.get("TOP_N").and_then(|s| s.parse().ok()).unwrap_or(5);
    let kind = meta.get("TYPE").cloned().unwrap_or_else(|| "CPU".to_string());

    // Column header line: TIMESTAMP,1,2,...,N  (skip it)
    let _header = lines.next().context("Missing header line")?.context("Failed to read header line")?;

    let mut timestamps: Vec<f64> = Vec::new();
    let mut ranks: Vec<Vec<Option<(String, f64)>>> = (0..n).map(|_| Vec::new()).collect();

    for line in lines {
        let line = line.context("Failed to read data line")?;
        if line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split(',');
        let ts: f64 = cols.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        timestamps.push(ts);

        for rank_vec in &mut ranks {
            let entry = cols.next().and_then(|s| {
                if s.is_empty() {
                    return None;
                }
                let mut parts = s.splitn(2, '|');
                let name = parts.next()?.to_string();
                let val: f64 = parts.next()?.parse().ok()?;
                Some((name, val))
            });
            rank_vec.push(entry);
        }
    }

    Ok((
        kind,
        TopProcessData {
            n,
            start_time,
            timestamps,
            ranks,
        },
    ))
}

fn parse_data(
    lines_iter: &mut Lines<BufReader<File>>,
    collected_data_names: &[DataType],
    cpu_core_count: usize,
) -> Result<HashMap<DataType, Vec<String>>, Error> {
    let mut collected_vec_data: Vec<Vec<String>> = Vec::new();
    for _ in 0..collected_data_names.len() {
        collected_vec_data.push(Vec::new());
    }

    for line in lines_iter {
        let line = line.context("Failed to read line of data file")?;
        let mut split = line.split(',');
        if split.clone().count() != collected_data_names.len() {
            info!("Line \"{line}\" is broken - not enough items, skipping it");
            continue;
        }
        for i in &mut collected_vec_data {
            i.push(split.next().expect("Validated before").to_string());
        }
    }

    let mut collected_data: HashMap<DataType, Vec<String>> = HashMap::default();
    for (data_name, data) in collected_data_names.iter().zip(collected_vec_data) {
        collected_data.insert(data_name.clone(), data);
    }

    // Special formatting for CPU_USAGE_PER_CORE: stored as semicolon-joined per-row;
    // reorganised into one Vec<String> per core where each entry is all timestamps joined.
    if let Some(cpu_per_core_data) = collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        let mut per_core: Vec<Vec<String>> = (0..cpu_core_count).map(|_| Vec::new()).collect();

        for row in cpu_per_core_data {
            let mut split = row.split(';');
            let count = split.clone().count();
            if count != cpu_core_count {
                return Err(Error::msg(format!(
                    "CPU data \"{row}\" does not contain the expected number of cores ({count}/{cpu_core_count})"
                )));
            }
            for core_vec in &mut per_core {
                core_vec.push(split.next().expect("Validated above").to_string());
            }
        }

        let reformatted: Vec<String> = per_core.into_iter().map(|v| v.join(";")).collect();
        collected_data.insert(DataType::CPU_USAGE_PER_CORE, reformatted);
    }

    Ok(collected_data)
}

fn parse_header(
    lines_iter: &mut Lines<BufReader<File>>,
    hashmap_data: &HashMap<String, String>,
) -> Result<(Vec<DataType>, Vec<GeneralInfoGroup>), Error> {
    let header_line = lines_iter
        .next()
        .context("Failed to read second line of data file")?
        .context("Failed to read second line of data file")?;

    // Build name lookup maps from the metadata line.
    // GPU_N=<name>, NET_N=<iface>, CUSTOM_N=<name>
    let mut gpu_names: HashMap<usize, String> = HashMap::new();
    let mut iface_names: HashMap<usize, String> = HashMap::new();
    let mut custom_names: HashMap<usize, String> = HashMap::new();
    let mut disk_names: HashMap<usize, String> = HashMap::new();

    for (key, val) in hashmap_data {
        if let Some(rest) = key.strip_prefix("GPU_") {
            if let Ok(idx) = rest.parse::<usize>() {
                gpu_names.insert(idx, val.clone());
            }
        } else if let Some(rest) = key.strip_prefix("NET_") {
            if let Ok(idx) = rest.parse::<usize>() {
                iface_names.insert(idx, val.clone());
            }
        } else if let Some(rest) = key.strip_prefix("CUSTOM_") {
            if let Ok(idx) = rest.parse::<usize>() {
                custom_names.insert(idx, val.clone());
            }
        } else if let Some(rest) = key.strip_prefix("DISK_") {
            if let Ok(idx) = rest.parse::<usize>() {
                disk_names.insert(idx, val.clone());
            }
        }
    }

    let collected_data_names: Vec<DataType> = header_line
        .split(',')
        .map(|item| {
            DataType::from_column_name(item, &gpu_names, &iface_names, &custom_names, &disk_names).ok_or_else(|| {
                Error::msg(format!(
                    "Unknown column \"{item}\" in data file (allowed: {})",
                    DataType::get_allowed_values()
                ))
            })
        })
        .collect::<Result<_, Error>>()?;

    if collected_data_names.len() <= 1 {
        return Err(Error::msg("No data columns found in CSV"));
    }
    if collected_data_names[0] != DataType::SECONDS_SINCE_START {
        return Err(Error::msg("First column must be SECONDS_SINCE_START"));
    }

    let mut collected_groups = Vec::new();
    let cpu_cols = [DataType::CPU_USAGE_TOTAL, DataType::CPU_USAGE_PER_CORE];
    let mem_cols = [DataType::MEMORY_AVAILABLE, DataType::MEMORY_FREE, DataType::MEMORY_USED];
    let swap_cols = [DataType::SWAP_USED, DataType::SWAP_FREE];

    if collected_data_names.iter().any(|e| cpu_cols.contains(e) || e.is_cpu()) {
        collected_groups.push(GeneralInfoGroup::CPU);
    }
    if collected_data_names.iter().any(|e| mem_cols.contains(e)) {
        collected_groups.push(GeneralInfoGroup::MEMORY);
    }
    if collected_data_names.iter().any(|e| swap_cols.contains(e)) {
        collected_groups.push(GeneralInfoGroup::SWAP);
    }
    if collected_data_names.iter().any(|e| e.is_network()) {
        collected_groups.push(GeneralInfoGroup::NETWORK);
    }
    if collected_data_names.iter().any(|e| e.is_gpu()) {
        collected_groups.push(GeneralInfoGroup::GPU);
    }
    if collected_data_names.iter().any(|e| e.is_disk()) {
        collected_groups.push(GeneralInfoGroup::DISK);
    }

    Ok((collected_data_names, collected_groups))
}

type ParsedOkResult = (f64, f64, usize, f32, HashMap<String, String>, f64);

fn parse_file_values_data(lines_iter: &mut Lines<BufReader<File>>) -> Result<ParsedOkResult, Error> {
    let line = lines_iter
        .next()
        .context("Failed to read first line of data file")?
        .context("Failed to read first line of data file")?;

    let mut map: HashMap<String, String> = HashMap::new();
    for item in line.split(',') {
        let mut kv = item.split('=');
        let key = kv.next().context("Missing key in metadata line")?.to_string();
        let val = kv.next().context("Missing value in metadata line")?.to_string();
        map.insert(key, val);
    }

    let swap_total = map
        .remove(&HeaderValues::SWAP_TOTAL.to_string())
        .context("Missing SWAP_TOTAL")?
        .parse::<f64>()
        .context("Failed to parse SWAP_TOTAL")?;
    let memory_total = map
        .remove(&HeaderValues::MEMORY_TOTAL.to_string())
        .context("Missing MEMORY_TOTAL")?
        .parse::<f64>()
        .context("Failed to parse MEMORY_TOTAL")?;
    let cpu_core_count = map
        .remove(&HeaderValues::CPU_CORE_COUNT.to_string())
        .context("Missing CPU_CORE_COUNT")?
        .parse::<usize>()
        .context("Failed to parse CPU_CORE_COUNT")?;
    let check_interval = map
        .remove(&HeaderValues::INTERVAL_SECONDS.to_string())
        .context("Missing INTERVAL_SECONDS")?
        .parse::<f32>()
        .context("Failed to parse INTERVAL_SECONDS")?;
    let start_time = map
        .remove(&HeaderValues::UNIX_TIMESTAMP_START_TIME.to_string())
        .context("Missing UNIX_TIMESTAMP_START_TIME")?
        .parse::<f64>()
        .context("Failed to parse UNIX_TIMESTAMP_START_TIME")?;

    Ok((swap_total, memory_total, cpu_core_count, check_interval, map, start_time))
}
