use std::collections::HashMap;
use std::fs::{metadata, File};
use std::io::{BufRead, BufReader, Lines};

use anyhow::{Context, Error, Result};
use log::info;

use crate::enums::{DataType, GeneralInfoGroup, HeaderValues};
use crate::model::{CollectedItemModels, Settings};

pub fn load_csv_results(settings: &Settings) -> Result<CollectedItemModels, Error> {
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

    let (swap_total, memory_total, cpu_core_count, check_interval, hashmap_data) = parse_file_values_data(&mut lines_iter)?;
    let (collected_data_names, collected_groups) = parse_header(&mut lines_iter, &hashmap_data)?;
    let collected_data = parse_data(&mut lines_iter, &collected_data_names, cpu_core_count)?;

    Ok(CollectedItemModels {
        collected_data_names,
        collected_data,
        collected_groups,
        memory_total,
        swap_total,
        cpu_core_count,
        check_interval,
    })
}

// TODO here should be added better error handling, if last line is broken, then this should ignore problem and continue
// TODO consider to add debug check results type, may be available as option in settings
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
            // Unwrap is safe, because we checked this line earlier
            i.push(split.next().unwrap().to_string());
        }
    }

    let mut collected_data: HashMap<DataType, Vec<String>> = HashMap::default();
    for (data_name, data) in collected_data_names.iter().zip(collected_vec_data.into_iter()) {
        collected_data.insert(data_name.clone(), data);
    }

    // Special formatting for CPU usage per core which is really Vec<Vec<String>> instead, Vec<String>
    if let Some(cpu_per_core_data) = collected_data.get(&DataType::CPU_USAGE_PER_CORE) {
        let mut cpu_per_core_data_pre_formatted = Vec::new();
        for _ in 0..cpu_core_count {
            cpu_per_core_data_pre_formatted.push(Vec::new());
        }

        for cpu_core_data in cpu_per_core_data {
            let mut split = cpu_core_data.split(';');
            let count = split.clone().count();
            if count != cpu_core_count {
                return Err(Error::msg(format!(
                    "Cpu data - \"{cpu_core_data}\" not contains required amount cpu usage results ({count}/{cpu_core_count})"
                )));
            }
            for i in &mut cpu_per_core_data_pre_formatted {
                // Unwrap is safe, because we checked this line earlier
                i.push(split.next().unwrap().to_string());
            }
        }

        let mut cpu_per_core_data_formatted = Vec::new();
        for data in cpu_per_core_data_pre_formatted {
            cpu_per_core_data_formatted.push(data.join(";"));
        }

        collected_data.insert(DataType::CPU_USAGE_PER_CORE, cpu_per_core_data_formatted);
    }

    Ok(collected_data)
}

fn parse_header(
    lines_iter: &mut Lines<BufReader<File>>,
    hashmap_data: &HashMap<String, String>,
) -> Result<(Vec<DataType>, Vec<GeneralInfoGroup>), Error> {
    // Header data like UNIX_TIMESTAMP, MEMORY_USED, CPU_TOTAL, etc.
    let collected_data_names_str: String = lines_iter
        .next()
        .context("Failed to read second line of data file")?
        .context("Failed to read second line of data file")?;
    let collected_data_names: Vec<DataType> = collected_data_names_str
        .split(',')
        .map(|item| match item.parse::<DataType>() {
            Ok(item) => Ok(item),
            Err(_) => {
                if let Some(s) = item.strip_prefix("CUSTOM_") {
                    let split = s.split('_').collect::<Vec<_>>();
                    if split.len() != 2 || split[0].parse::<usize>().is_err() || !(matches!(split[1], "CPU" | "MEMORY")) {
                        return Err(Error::msg(format!(
                            "Failed to parse custom item {item}, should have format CUSTOM_{{IDX}}_CPU or CUSTOM_{{IDX}}_MEMORY"
                        )));
                    }
                    let idx = split[0].parse::<usize>().unwrap();
                    let name = hashmap_data
                        .get(&format!("CUSTOM_{idx}"))
                        .context(format!("Failed to find CUSTOM_{idx} in data file, but it is used in header"))?
                        .to_string();
                    if split[1] == "CPU" {
                        Ok(DataType::CUSTOM_CPU((idx, name)))
                    } else {
                        Ok(DataType::CUSTOM_MEMORY((idx, name)))
                    }
                } else {
                    Err(Error::msg(format!(
                        "Failed to parse item {item} from data file, allowed values are {:?} or CUSTOM_ items",
                        DataType::get_allowed_values()
                    )))
                }
            }
        })
        .collect::<Result<_, Error>>()?;

    if collected_data_names.len() <= 1 {
        return Err(Error::msg("No data to load"));
    }
    if collected_data_names[0] != DataType::UNIX_TIMESTAMP {
        return Err(Error::msg("First item in data file should be UNIX_TIMESTAMP"));
    }

    let mut collected_groups = Vec::new();
    let cpu_collection = [DataType::CPU_USAGE_TOTAL, DataType::CPU_USAGE_PER_CORE];
    let memory_collection = [DataType::MEMORY_AVAILABLE, DataType::MEMORY_FREE, DataType::MEMORY_USED];
    let swap_collection = [DataType::SWAP_USED, DataType::SWAP_FREE];
    if collected_data_names.iter().any(|e| cpu_collection.contains(e)) {
        collected_groups.push(GeneralInfoGroup::CPU);
    }
    if collected_data_names.iter().any(|e| memory_collection.contains(e)) {
        collected_groups.push(GeneralInfoGroup::MEMORY);
    }
    if collected_data_names.iter().any(|e| swap_collection.contains(e)) {
        collected_groups.push(GeneralInfoGroup::SWAP);
    }
    Ok((collected_data_names, collected_groups))
}

type ParsedOkResult = (f64, f64, usize, f32, HashMap<String, String>);

fn parse_file_values_data(lines_iter: &mut Lines<BufReader<File>>) -> std::result::Result<ParsedOkResult, Error> {
    let general_data_info = lines_iter
        .next()
        .context("Failed to read first line of data file")?
        .context("Failed to read first line of data file")?;

    // MEMORY_TOTAL, CPU_CORE_COUNT, INTERVAL_SECONDS, etc.
    let mut general_data_hashmap: HashMap<String, String> = HashMap::new();
    for item in general_data_info.split(',') {
        let mut split = item.split('=');
        let key = split.next().context("Failed to get key from general data")?.to_string();
        let value = split.next().context("Failed to get value from general data")?.to_string();
        general_data_hashmap.insert(key, value);
    }

    let swap_total = general_data_hashmap
        .remove(&HeaderValues::SWAP_TOTAL.to_string())
        .context("Failed to get SWAP_TOTAL from general data")?
        .parse::<f64>()
        .context("Failed to parse SWAP_TOTAL from general data")?;
    let memory_total = general_data_hashmap
        .remove(&HeaderValues::MEMORY_TOTAL.to_string())
        .context("Failed to get MEMORY_TOTAL from general data")?
        .parse::<f64>()
        .context("Failed to parse MEMORY_TOTAL from general data")?;
    let cpu_core_count = general_data_hashmap
        .remove(&HeaderValues::CPU_CORE_COUNT.to_string())
        .context("Failed to get CPU_CORE_COUNT from general data")?
        .parse::<usize>()
        .context("Failed to parse CPU_CORE_COUNT from general data")?;
    let check_interval = general_data_hashmap
        .remove(&HeaderValues::INTERVAL_SECONDS.to_string())
        .context("Failed to get INTERVAL_SECONDS from general data")?
        .parse::<f32>()
        .context("Failed to parse INTERVAL_SECONDS from general data")?;

    Ok((swap_total, memory_total, cpu_core_count, check_interval, general_data_hashmap))
}
