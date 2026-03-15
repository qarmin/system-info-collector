use std::collections::HashMap;
use std::fs::{metadata, File};
use std::io::{BufRead, BufReader, Lines};

use anyhow::{Context, Error, Result};
use log::info;
use system_info_collector_core::enums::{DataType, GeneralInfoGroup, HeaderValues};
use system_info_collector_core::model::CollectedItemModels;
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
    let (collected_data_names, collected_groups) = parse_header(&mut lines_iter, &hashmap_data)?;
    let collected_data = parse_data(&mut lines_iter, &collected_data_names, cpu_core_count)?;

    Ok(CollectedItemModels {
        collected_data,
        collected_groups,
        memory_total,
        swap_total,
        cpu_core_count,
        check_interval,
        start_time,
    })
}

fn parse_data(lines_iter: &mut Lines<BufReader<File>>, collected_data_names: &[DataType], cpu_core_count: usize) -> Result<HashMap<DataType, Vec<String>>, Error> {
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

fn parse_header(lines_iter: &mut Lines<BufReader<File>>, hashmap_data: &HashMap<String, String>) -> Result<(Vec<DataType>, Vec<GeneralInfoGroup>), Error> {
    let header_line = lines_iter.next().context("Failed to read second line of data file")?.context("Failed to read second line of data file")?;

    let collected_data_names: Vec<DataType> = header_line
        .split(',')
        .map(|item| match item.parse::<DataType>() {
            Ok(dt) => Ok(dt),
            Err(_) => {
                if let Some(s) = item.strip_prefix("CUSTOM_") {
                    let parts = s.split('_').collect::<Vec<_>>();
                    if parts.len() != 2 || parts[0].parse::<usize>().is_err() || !(matches!(parts[1], "CPU" | "MEMORY")) {
                        return Err(Error::msg(format!(
                            "Failed to parse custom column \"{item}\": expected CUSTOM_{{IDX}}_CPU or CUSTOM_{{IDX}}_MEMORY"
                        )));
                    }
                    let idx: usize = parts[0].parse().expect("Validated above");
                    let name = hashmap_data
                        .get(&format!("CUSTOM_{idx}"))
                        .context(format!("CUSTOM_{idx} referenced in header but missing in metadata line"))?.clone();
                    if parts[1] == "CPU" {
                        Ok(DataType::CUSTOM_CPU((idx, name)))
                    } else {
                        Ok(DataType::CUSTOM_MEMORY((idx, name)))
                    }
                } else {
                    Err(Error::msg(format!(
                        "Unknown column \"{item}\" in data file (allowed: {:?})",
                        DataType::get_allowed_values()
                    )))
                }
            }
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
    let net_cols = [DataType::NETWORK_RX_BYTES_PER_SEC, DataType::NETWORK_TX_BYTES_PER_SEC];
    let gpu_cols = [DataType::GPU_UTILIZATION, DataType::GPU_MEMORY_USED, DataType::GPU_TEMPERATURE];

    if collected_data_names.iter().any(|e| cpu_cols.contains(e) || e.is_cpu()) {
        collected_groups.push(GeneralInfoGroup::CPU);
    }
    if collected_data_names.iter().any(|e| mem_cols.contains(e)) {
        collected_groups.push(GeneralInfoGroup::MEMORY);
    }
    if collected_data_names.iter().any(|e| swap_cols.contains(e)) {
        collected_groups.push(GeneralInfoGroup::SWAP);
    }
    if collected_data_names.iter().any(|e| net_cols.contains(e)) {
        collected_groups.push(GeneralInfoGroup::NETWORK);
    }
    if collected_data_names.iter().any(|e| gpu_cols.contains(e)) {
        collected_groups.push(GeneralInfoGroup::GPU);
    }

    Ok((collected_data_names, collected_groups))
}

type ParsedOkResult = (f64, f64, usize, f32, HashMap<String, String>, f64);

fn parse_file_values_data(lines_iter: &mut Lines<BufReader<File>>) -> Result<ParsedOkResult, Error> {
    let line = lines_iter.next().context("Failed to read first line of data file")?.context("Failed to read first line of data file")?;

    let mut map: HashMap<String, String> = HashMap::new();
    for item in line.split(',') {
        let mut kv = item.split('=');
        let key = kv.next().context("Missing key in metadata line")?.to_string();
        let val = kv.next().context("Missing value in metadata line")?.to_string();
        map.insert(key, val);
    }

    let swap_total = map.remove(&HeaderValues::SWAP_TOTAL.to_string()).context("Missing SWAP_TOTAL")?.parse::<f64>().context("Failed to parse SWAP_TOTAL")?;
    let memory_total = map.remove(&HeaderValues::MEMORY_TOTAL.to_string()).context("Missing MEMORY_TOTAL")?.parse::<f64>().context("Failed to parse MEMORY_TOTAL")?;
    let cpu_core_count = map.remove(&HeaderValues::CPU_CORE_COUNT.to_string()).context("Missing CPU_CORE_COUNT")?.parse::<usize>().context("Failed to parse CPU_CORE_COUNT")?;
    let check_interval = map.remove(&HeaderValues::INTERVAL_SECONDS.to_string()).context("Missing INTERVAL_SECONDS")?.parse::<f32>().context("Failed to parse INTERVAL_SECONDS")?;
    let start_time = map.remove(&HeaderValues::UNIX_TIMESTAMP_START_TIME.to_string()).context("Missing UNIX_TIMESTAMP_START_TIME")?.parse::<f64>().context("Failed to parse UNIX_TIMESTAMP_START_TIME")?;

    Ok((swap_total, memory_total, cpu_core_count, check_interval, map, start_time))
}
