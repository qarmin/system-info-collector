use std::collections::HashMap;
use std::fs::metadata;
use std::io::{BufRead, BufReader};

use anyhow::{Context, Error, Result};
use log::info;

use crate::model::{CollectedItemModels, Settings};

pub fn load_csv_results(settings: &Settings) -> Result<CollectedItemModels, Error> {
    info!(
        "Data csv file is {} in size",
        humansize::format_size(
            metadata(&settings.data_path).context("Failed to get metadata of data file")?.len(),
            humansize::BINARY,
        )
    );

    let data_file = std::fs::File::open(&settings.data_path).context(format!("Failed to open data file {}", &settings.data_path))?;
    let mut data_file = BufReader::new(data_file);

    let mut lines_iter = data_file.lines();
    let general_data_info = lines_iter
        .next()
        .context("Failed to read first line of data file")?
        .context("Failed to read first line of data file")?;

    // MEMORY_TOTAL, CPU_CORE_COUNT, INTERVAL_SECONDS, etc.
    let mut general_data_hashmap = HashMap::new();
    for item in general_data_info.split(',') {
        let mut split = item.split('=');
        let key = split.next().context("Failed to get key from general data")?.to_string();
        let value = split.next().context("Failed to get value from general data")?.to_string();
        general_data_hashmap.insert(key, value);
    }

    // Header data like UNIX_TIMESTAMP, MEMORY_USED, CPU_TOTAL, etc.
    let mut collected_data_names: String = lines_iter
        .next()
        .context("Failed to read second line of data file")?
        .context("Failed to read second line of data file")?;
    let collected_data_names_vec: Vec<String> = collected_data_names.split(',').map(|item| item.to_string()).collect();
    if collected_data_names_vec.len() <= 1 {
        return Err(Error::msg("No data to load"));
    }
    if collected_data_names_vec[0] != "UNIX_TIMESTAMP" {
        return Err(Error::msg("First item in data file should be UNIX_TIMESTAMP"));
    }

    // TODO here should be added better error handling, if last line is broken, then this should ignore problem and continue
    // TODO consider to add debug check results type, may be available as option in settings
    let mut loaded_items: Vec<Vec<String>> = Vec::new();
    for i in 0..collected_data_names_vec.len() {
        loaded_items.push(Vec::new());
    }

    for line in lines_iter {
        let line = line.context("Failed to read line of data file")?;
        let mut split = line.split(',');
        if split.clone().count() != collected_data_names_vec.len() {
            info!("Line \"{line}\" is broken - not enough items, skipping it");
            continue;
        }
        for i in &mut loaded_items {
            // Unwrap is safe, because we checked this line earlier
            i.push(split.next().unwrap().to_string());
        }
    }

    Ok(CollectedItemModels {
        hashmap_general_info: general_data_hashmap,
        collected_data_names: collected_data_names_vec,
        collected_data: loaded_items,
    })
}
