use std::collections::HashMap;
use std::fs::{metadata, File};
use std::io::{BufRead, BufReader, Lines};

use anyhow::{Context, Error, Result};
use log::info;
use strum::IntoEnumIterator;

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

    let (memory_total, cpu_core_count, check_interval) = parse_file_values_data(&mut lines_iter)?;
    let (collected_data_names, collected_groups) = parse_header(&mut lines_iter)?;
    let collected_data = parse_data(&mut lines_iter, &collected_data_names)?;

    Ok(CollectedItemModels {
        collected_data_names,
        collected_data,
        collected_groups,
        memory_total,
        cpu_core_count,
        check_interval,
    })
}

// TODO here should be added better error handling, if last line is broken, then this should ignore problem and continue
// TODO consider to add debug check results type, may be available as option in settings
fn parse_data(lines_iter: &mut Lines<BufReader<File>>, collected_data_names: &[DataType]) -> Result<HashMap<DataType, Vec<String>>, Error> {
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
        collected_data.insert(*data_name, data);
    }

    Ok(collected_data)
}

fn parse_header(lines_iter: &mut Lines<BufReader<File>>) -> Result<(Vec<DataType>, Vec<GeneralInfoGroup>), Error> {
    // Header data like UNIX_TIMESTAMP, MEMORY_USED, CPU_TOTAL, etc.
    let collected_data_names_str: String = lines_iter
        .next()
        .context("Failed to read second line of data file")?
        .context("Failed to read second line of data file")?;
    let collected_data_names: Vec<DataType> = collected_data_names_str
        .split(',')
        .map(|item| match item.parse::<DataType>() {
            Ok(item) => Ok(item),
            Err(_) => Err(Error::msg(format!(
                "Failed to parse item {item} from data file, allowed values are {:?}",
                DataType::iter().map(|e| e.to_string()).collect::<String>()
            ))),
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
    if collected_data_names.iter().any(|e| cpu_collection.contains(e)) {
        collected_groups.push(GeneralInfoGroup::CPU);
    }
    if collected_data_names.iter().any(|e| memory_collection.contains(e)) {
        collected_groups.push(GeneralInfoGroup::MEMORY);
    }
    Ok((collected_data_names, collected_groups))
}

fn parse_file_values_data(lines_iter: &mut Lines<BufReader<File>>) -> Result<(u64, u64, f32), Error> {
    let general_data_info = lines_iter
        .next()
        .context("Failed to read first line of data file")?
        .context("Failed to read first line of data file")?;

    // MEMORY_TOTAL, CPU_CORE_COUNT, INTERVAL_SECONDS, etc.
    let mut general_data_hashmap: HashMap<HeaderValues, String> = HashMap::new();
    for item in general_data_info.split(',') {
        let mut split = item.split('=');
        let key = split.next().context("Failed to get key from general data")?.parse().context(format!(
            "Failed to parse header value, available values {}",
            HeaderValues::iter().map(|e| e.to_string()).collect::<String>()
        ))?;
        let value = split.next().context("Failed to get value from general data")?.to_string();
        general_data_hashmap.insert(key, value);
    }

    let memory_total = general_data_hashmap
        .get(&HeaderValues::MEMORY_TOTAL)
        .context("Failed to get MEMORY_TOTAL from general data")?
        .parse::<u64>()
        .context("Failed to parse MEMORY_TOTAL from general data")?;
    let cpu_core_count = general_data_hashmap
        .get(&HeaderValues::CPU_CORE_COUNT)
        .context("Failed to get CPU_CORE_COUNT from general data")?
        .parse::<u64>()
        .context("Failed to parse CPU_CORE_COUNT from general data")?;
    let check_interval = general_data_hashmap
        .get(&HeaderValues::INTERVAL_SECONDS)
        .context("Failed to get INTERVAL_SECONDS from general data")?
        .parse::<f32>()
        .context("Failed to parse INTERVAL_SECONDS from general data")?;

    Ok((memory_total, cpu_core_count, check_interval))
}
