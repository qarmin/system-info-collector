#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use clap::ValueEnum;
use strum::{Display, EnumIter, EnumString};

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display)]
pub enum DataCollectionMode {
    #[default]
    CPU_USAGE_TOTAL,
    CPU_USAGE_PER_CORE,
    MEMORY_USED,
    MEMORY_FREE,
    MEMORY_AVAILABLE,
    // TODO CustomProcessName, CustomProcessId
}

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default, Display)]
pub enum AppMode {
    #[default]
    COLLECT,
    COLLECT_AND_CONVERT,
    CONVERT,
}

#[derive(Clone, EnumString, EnumIter, ValueEnum, Debug, Eq, PartialEq, Default)]
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
