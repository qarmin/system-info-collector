#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use clap::ValueEnum;
use serde::Deserialize;
use strum::{Display, EnumIter, EnumString};

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
pub enum DataCollectionMode {
    #[default]
    CPU_USAGE_TOTAL,
    CPU_USAGE_PER_CORE,
    MEMORY_USED,
    MEMORY_FREE,
    MEMORY_AVAILABLE,
    // TODO CustomProcessName, CustomProcessId
}

// Must contains same enums as above with additional UNIX_TIMESTAMP and maybe some other

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
pub enum DataType {
    #[default]
    UNIX_TIMESTAMP,
    CPU_USAGE_TOTAL,
    CPU_USAGE_PER_CORE,
    MEMORY_USED,
    MEMORY_FREE,
    MEMORY_AVAILABLE,
}
impl DataType {
    pub fn is_memory(self) -> bool {
        matches!(self, DataType::MEMORY_USED | DataType::MEMORY_FREE | DataType::MEMORY_AVAILABLE)
    }
    // pub fn is_cpu(self) -> bool {
    //     matches!(self, DataType::CPU_USAGE_TOTAL | DataType::CPU_USAGE_PER_CORE)
    // }
    pub fn pretty_print(self) -> String {
        match self {
            DataType::UNIX_TIMESTAMP => "Unix timestamp".to_string(),
            DataType::CPU_USAGE_TOTAL => "CPU usage total".to_string(),
            DataType::CPU_USAGE_PER_CORE => "CPU usage per core".to_string(),
            DataType::MEMORY_USED => "Memory used".to_string(),
            DataType::MEMORY_FREE => "Memory free".to_string(),
            DataType::MEMORY_AVAILABLE => "Memory available".to_string(),
        }
    }
}
#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
pub enum HeaderValues {
    #[default]
    MEMORY_TOTAL,
    CPU_CORE_COUNT,
    INTERVAL_SECONDS,
}

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
pub enum GeneralInfoGroup {
    #[default]
    CPU,
    MEMORY,
}

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
pub enum AppMode {
    #[default]
    COLLECT,
    COLLECT_AND_CONVERT,
    CONVERT,
}

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
pub enum LogLev {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl From<LogLev> for log::LevelFilter {
    fn from(log_lev: LogLev) -> Self {
        match log_lev {
            LogLev::Off => Self::Off,
            LogLev::Error => Self::Error,
            LogLev::Warn => Self::Warn,
            LogLev::Info => Self::Info,
            LogLev::Debug => Self::Debug,
            LogLev::Trace => Self::Trace,
        }
    }
}
