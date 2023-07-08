#![allow(non_camel_case_types)]

use clap::ValueEnum;
use strum::{EnumIter, EnumString};

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug)]
pub enum DataCollectionMode {
    CPU_USAGE_TOTAL,
    CPU_USAGE_PER_CORE,
    MEMORY_USAGE,
    MEMORY_FREE,
    MEMORY_AVAILABLE,
    // TODO CustomProcessName, CustomProcessId
}

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug)]
pub enum AppMode {
    COLLECT,
    COLLECT_AND_CONVERT,
    CONVERT,
}
