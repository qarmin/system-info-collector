use std::fs::metadata;

use anyhow::{Context, Error, Result};
use log::info;

use crate::model::{CollectedItemModels, Settings, SingleItemModel};

pub fn load_csv_results(settings: &Settings) -> Result<CollectedItemModels, Error> {
    info!(
        "Data csv file is {} in size",
        humansize::format_size(
            metadata(&settings.data_path).context("Failed to get metadata of CSV file")?.len(),
            humansize::BINARY,
        )
    );

    let loaded_items = csv::Reader::from_path(&settings.data_path)
        .unwrap()
        .deserialize()
        .collect::<Result<Vec<SingleItemModel>, _>>()?;

    convert_results(loaded_items)
}

pub fn convert_results(loaded_items: Vec<SingleItemModel>) -> Result<CollectedItemModels, Error> {
    if loaded_items.is_empty() {
        return Ok(CollectedItemModels::default());
    }

    let mut collected_items = CollectedItemModels::new_with_reserved_space(loaded_items.len());

    let number_of_cpus = loaded_items[0].cpu_usage_per_core.split(';').count();
    collected_items.cpu_core_usage = Vec::with_capacity(number_of_cpus);
    for _ in 0..number_of_cpus {
        collected_items.cpu_core_usage.push(Vec::with_capacity(loaded_items.len()));
    }

    for item in loaded_items {
        collected_items.memory_total.push(item.memory_total);
        collected_items.memory_used.push(item.memory_used);
        collected_items.memory_available.push(item.memory_available);
        collected_items.memory_free.push(item.memory_free);
        collected_items.cpu_total.push(item.cpu_total);
        collected_items.unix_timestamp.push(item.unix_timestamp);

        let splits = item.cpu_usage_per_core.split(';');
        if splits.clone().count() != number_of_cpus {
            return Err(Error::msg(format!(
                "Cpu number changed when decoding, requested {} found later {} cpus",
                number_of_cpus,
                splits.count()
            )));
        }
        for (idx, split) in splits.enumerate() {
            collected_items.cpu_core_usage[idx].push(split.parse::<f32>().context("Failed to parse cpu usage")?);
        }
    }
    Ok(collected_items)
}
