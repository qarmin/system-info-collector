#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use serde::Deserialize;
use strum::{Display, EnumIter, EnumString};

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

/// Column identifiers used in the CSV file.
///
/// Static variants map 1:1 to CSV column names.  Dynamic variants carry the
/// index and a human-readable label that were resolved at collection time:
///
/// - `GPU_N_UTIL((0, "RTX 4090"))` → column `GPU_0_UTIL`, label "RTX 4090 util %"
/// - `NET_N_RX_BPS((1, "eth0"))` → column `NET_1_RX_BPS`, label "eth0 RX bytes/s"
///
/// The `Display` impl and `column_name()` helper both produce the canonical CSV
/// column string.
#[derive(Clone, Debug, Eq, PartialEq, Default, Deserialize, Hash)]
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
    // Static (legacy single-interface/GPU) — kept for backwards compatibility
    // with old CSV files that predate multi-GPU/multi-interface support.
    NETWORK_RX_BYTES_PER_SEC,
    NETWORK_TX_BYTES_PER_SEC,
    GPU_UTILIZATION,
    GPU_MEMORY_USED,
    GPU_TEMPERATURE,
    // Dynamic per-GPU columns: (gpu_index, gpu_name)
    GPU_N_UTIL((usize, String)),
    GPU_N_VRAM_MB((usize, String)),
    GPU_N_TEMP_C((usize, String)),
    // Dynamic per-interface columns: (iface_index, iface_name)
    NET_N_RX_BPS((usize, String)),
    NET_N_TX_BPS((usize, String)),
    // Custom process columns: (index, process_graph_name)
    CUSTOM_CPU((usize, String)),
    CUSTOM_MEMORY((usize, String)),
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.column_name())
    }
}

impl DataType {
    /// The canonical CSV column name for this variant.
    pub fn column_name(&self) -> String {
        match self {
            Self::SECONDS_SINCE_START => "SECONDS_SINCE_START".to_string(),
            Self::CPU_USAGE_TOTAL => "CPU_USAGE_TOTAL".to_string(),
            Self::CPU_USAGE_PER_CORE => "CPU_USAGE_PER_CORE".to_string(),
            Self::SWAP_FREE => "SWAP_FREE".to_string(),
            Self::SWAP_USED => "SWAP_USED".to_string(),
            Self::MEMORY_USED => "MEMORY_USED".to_string(),
            Self::MEMORY_FREE => "MEMORY_FREE".to_string(),
            Self::MEMORY_AVAILABLE => "MEMORY_AVAILABLE".to_string(),
            Self::NETWORK_RX_BYTES_PER_SEC => "NETWORK_RX_BYTES_PER_SEC".to_string(),
            Self::NETWORK_TX_BYTES_PER_SEC => "NETWORK_TX_BYTES_PER_SEC".to_string(),
            Self::GPU_UTILIZATION => "GPU_UTILIZATION".to_string(),
            Self::GPU_MEMORY_USED => "GPU_MEMORY_USED".to_string(),
            Self::GPU_TEMPERATURE => "GPU_TEMPERATURE".to_string(),
            Self::GPU_N_UTIL((idx, _)) => format!("GPU_{idx}_UTIL"),
            Self::GPU_N_VRAM_MB((idx, _)) => format!("GPU_{idx}_VRAM_MB"),
            Self::GPU_N_TEMP_C((idx, _)) => format!("GPU_{idx}_TEMP_C"),
            Self::NET_N_RX_BPS((idx, _)) => format!("NET_{idx}_RX_BPS"),
            Self::NET_N_TX_BPS((idx, _)) => format!("NET_{idx}_TX_BPS"),
            Self::CUSTOM_CPU((idx, _)) => format!("CUSTOM_{idx}_CPU"),
            Self::CUSTOM_MEMORY((idx, _)) => format!("CUSTOM_{idx}_MEMORY"),
        }
    }

    /// Parse a CSV column name back into a `DataType`.
    /// Dynamic variants require the caller to supply a name resolver
    /// (`gpu_names` / `iface_names` from the metadata line).
    pub fn from_column_name(
        s: &str,
        gpu_names: &std::collections::HashMap<usize, String>,
        iface_names: &std::collections::HashMap<usize, String>,
        custom_names: &std::collections::HashMap<usize, String>,
    ) -> Option<Self> {
        match s {
            "SECONDS_SINCE_START" => Some(Self::SECONDS_SINCE_START),
            "CPU_USAGE_TOTAL" => Some(Self::CPU_USAGE_TOTAL),
            "CPU_USAGE_PER_CORE" => Some(Self::CPU_USAGE_PER_CORE),
            "SWAP_FREE" => Some(Self::SWAP_FREE),
            "SWAP_USED" => Some(Self::SWAP_USED),
            "MEMORY_USED" => Some(Self::MEMORY_USED),
            "MEMORY_FREE" => Some(Self::MEMORY_FREE),
            "MEMORY_AVAILABLE" => Some(Self::MEMORY_AVAILABLE),
            "NETWORK_RX_BYTES_PER_SEC" => Some(Self::NETWORK_RX_BYTES_PER_SEC),
            "NETWORK_TX_BYTES_PER_SEC" => Some(Self::NETWORK_TX_BYTES_PER_SEC),
            "GPU_UTILIZATION" => Some(Self::GPU_UTILIZATION),
            "GPU_MEMORY_USED" => Some(Self::GPU_MEMORY_USED),
            "GPU_TEMPERATURE" => Some(Self::GPU_TEMPERATURE),
            _ => {
                if let Some(rest) = s.strip_prefix("GPU_") {
                    let parts: Vec<&str> = rest.splitn(2, '_').collect();
                    if parts.len() == 2 {
                        if let Ok(idx) = parts[0].parse::<usize>() {
                            let name = gpu_names.get(&idx).cloned().unwrap_or_else(|| format!("GPU {idx}"));
                            return match parts[1] {
                                "UTIL" => Some(Self::GPU_N_UTIL((idx, name))),
                                "VRAM_MB" => Some(Self::GPU_N_VRAM_MB((idx, name))),
                                "TEMP_C" => Some(Self::GPU_N_TEMP_C((idx, name))),
                                _ => None,
                            };
                        }
                    }
                }
                if let Some(rest) = s.strip_prefix("NET_") {
                    let parts: Vec<&str> = rest.splitn(2, '_').collect();
                    if parts.len() == 2 {
                        if let Ok(idx) = parts[0].parse::<usize>() {
                            let name = iface_names.get(&idx).cloned().unwrap_or_else(|| format!("iface{idx}"));
                            return match parts[1] {
                                "RX_BPS" => Some(Self::NET_N_RX_BPS((idx, name))),
                                "TX_BPS" => Some(Self::NET_N_TX_BPS((idx, name))),
                                _ => None,
                            };
                        }
                    }
                }
                if let Some(rest) = s.strip_prefix("CUSTOM_") {
                    let parts: Vec<&str> = rest.splitn(2, '_').collect();
                    if parts.len() == 2 {
                        if let Ok(idx) = parts[0].parse::<usize>() {
                            let name = custom_names.get(&idx).cloned().unwrap_or_else(|| format!("custom{idx}"));
                            return match parts[1] {
                                "CPU" => Some(Self::CUSTOM_CPU((idx, name))),
                                "MEMORY" => Some(Self::CUSTOM_MEMORY((idx, name))),
                                _ => None,
                            };
                        }
                    }
                }
                None
            }
        }
    }

    pub fn get_allowed_values() -> String {
        // Only list the static variants for error messages.
        [
            "SECONDS_SINCE_START",
            "CPU_USAGE_TOTAL",
            "CPU_USAGE_PER_CORE",
            "SWAP_FREE",
            "SWAP_USED",
            "MEMORY_USED",
            "MEMORY_FREE",
            "MEMORY_AVAILABLE",
            "NETWORK_RX_BYTES_PER_SEC",
            "NETWORK_TX_BYTES_PER_SEC",
            "GPU_UTILIZATION",
            "GPU_MEMORY_USED",
            "GPU_TEMPERATURE",
            "GPU_N_UTIL",
            "GPU_N_VRAM_MB",
            "GPU_N_TEMP_C",
            "NET_N_RX_BPS",
            "NET_N_TX_BPS",
            "CUSTOM_N_CPU",
            "CUSTOM_N_MEMORY",
        ]
        .join(", ")
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
        matches!(
            self,
            Self::NETWORK_RX_BYTES_PER_SEC | Self::NETWORK_TX_BYTES_PER_SEC | Self::NET_N_RX_BPS(_) | Self::NET_N_TX_BPS(_)
        )
    }

    pub fn is_gpu(&self) -> bool {
        matches!(
            self,
            Self::GPU_UTILIZATION
                | Self::GPU_MEMORY_USED
                | Self::GPU_TEMPERATURE
                | Self::GPU_N_UTIL(_)
                | Self::GPU_N_VRAM_MB(_)
                | Self::GPU_N_TEMP_C(_)
        )
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
            Self::NETWORK_RX_BYTES_PER_SEC => "Network RX MB/s".to_string(),
            Self::NETWORK_TX_BYTES_PER_SEC => "Network TX MB/s".to_string(),
            Self::GPU_UTILIZATION => "GPU utilization %".to_string(),
            Self::GPU_MEMORY_USED => "GPU memory used (MB)".to_string(),
            Self::GPU_TEMPERATURE => "GPU temperature (°C)".to_string(),
            Self::GPU_N_UTIL((_, name)) => format!("{name} util %"),
            Self::GPU_N_VRAM_MB((_, name)) => format!("{name} VRAM MB"),
            Self::GPU_N_TEMP_C((_, name)) => format!("{name} temp °C"),
            Self::NET_N_RX_BPS((_, name)) => format!("{name} RX MB/s"),
            Self::NET_N_TX_BPS((_, name)) => format!("{name} TX MB/s"),
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
