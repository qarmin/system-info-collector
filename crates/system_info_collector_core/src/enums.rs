#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use serde::Deserialize;
use strum::{Display, EnumIter, EnumString, IntoEnumIterator};

#[derive(Clone, EnumString, EnumIter, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SimpleDataCollectionMode {
    #[default]
    CPU_USAGE_TOTAL,
    CPU_USAGE_PER_CORE,
    SWAP_FREE,
    SWAP_USED,
    MEMORY_USED,
    MEMORY_FREE,
    MEMORY_AVAILABLE,
    NETWORK_RX_BYTES_PER_SEC,
    NETWORK_TX_BYTES_PER_SEC,
    GPU_UTILIZATION,
    GPU_MEMORY_USED,
    GPU_TEMPERATURE,
}

impl SimpleDataCollectionMode {
    pub fn is_network(self) -> bool {
        matches!(self, Self::NETWORK_RX_BYTES_PER_SEC | Self::NETWORK_TX_BYTES_PER_SEC)
    }

    pub fn is_gpu(self) -> bool {
        matches!(self, Self::GPU_UTILIZATION | Self::GPU_MEMORY_USED | Self::GPU_TEMPERATURE)
    }
}

// Must contain same enums as above with additional SECONDS_SINCE_START and maybe some other
#[derive(Clone, EnumString, EnumIter, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash)]
pub enum DataType {
    #[default]
    SECONDS_SINCE_START,
    CPU_USAGE_TOTAL,
    CPU_USAGE_PER_CORE,
    SWAP_FREE,
    SWAP_USED,
    MEMORY_USED,
    MEMORY_FREE,
    MEMORY_AVAILABLE,
    NETWORK_RX_BYTES_PER_SEC,
    NETWORK_TX_BYTES_PER_SEC,
    GPU_UTILIZATION,
    GPU_MEMORY_USED,
    GPU_TEMPERATURE,
    CUSTOM_CPU((usize, String)),
    CUSTOM_MEMORY((usize, String)),
}

impl DataType {
    pub fn get_allowed_values() -> String {
        Self::iter().map(|e| e.to_string()).collect::<Vec<String>>().join(", ")
    }

    pub fn is_memory(&self) -> bool {
        matches!(
            self,
            Self::MEMORY_USED | Self::MEMORY_FREE | Self::MEMORY_AVAILABLE | Self::CUSTOM_MEMORY(_)
        )
    }

    pub fn is_swap(&self) -> bool {
        matches!(self, Self::SWAP_USED | Self::SWAP_FREE)
    }

    pub fn is_cpu(&self) -> bool {
        matches!(self, Self::CPU_USAGE_TOTAL | Self::CPU_USAGE_PER_CORE | Self::CUSTOM_CPU(_))
    }

    pub fn is_network(&self) -> bool {
        matches!(self, Self::NETWORK_RX_BYTES_PER_SEC | Self::NETWORK_TX_BYTES_PER_SEC)
    }

    pub fn is_gpu(&self) -> bool {
        matches!(self, Self::GPU_UTILIZATION | Self::GPU_MEMORY_USED | Self::GPU_TEMPERATURE)
    }

    pub fn pretty_print(&self) -> String {
        match self {
            Self::SECONDS_SINCE_START => "Unix timestamp".to_string(),
            Self::CPU_USAGE_TOTAL => "CPU usage total".to_string(),
            Self::CPU_USAGE_PER_CORE => "CPU usage per core".to_string(),
            Self::MEMORY_USED => "Memory used".to_string(),
            Self::MEMORY_FREE => "Memory free".to_string(),
            Self::SWAP_FREE => "Swap free".to_string(),
            Self::SWAP_USED => "Swap used".to_string(),
            Self::MEMORY_AVAILABLE => "Memory available".to_string(),
            Self::NETWORK_RX_BYTES_PER_SEC => "Network RX bytes/sec".to_string(),
            Self::NETWORK_TX_BYTES_PER_SEC => "Network TX bytes/sec".to_string(),
            Self::GPU_UTILIZATION => "GPU utilization %".to_string(),
            Self::GPU_MEMORY_USED => "GPU memory used (MB)".to_string(),
            Self::GPU_TEMPERATURE => "GPU temperature (°C)".to_string(),
            Self::CUSTOM_CPU((_, name)) => format!("CPU usage for {name}"),
            Self::CUSTOM_MEMORY((_, name)) => format!("Memory usage for {name}"),
        }
    }
}

#[derive(Clone, EnumString, EnumIter, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum HeaderValues {
    #[default]
    MEMORY_TOTAL,
    SWAP_TOTAL,
    CPU_CORE_COUNT,
    INTERVAL_SECONDS,
    APP_VERSION,
    UNIX_TIMESTAMP_START_TIME,
}

#[derive(Clone, EnumString, EnumIter, Debug, Eq, PartialEq, Default, Display, Deserialize, Hash, Copy)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum GeneralInfoGroup {
    #[default]
    CPU,
    MEMORY,
    SWAP,
    NETWORK,
    GPU,
}
