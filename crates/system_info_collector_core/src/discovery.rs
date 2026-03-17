use std::path::PathBuf;

use log::{info, warn};
use sysinfo::Networks;

/// Which GPU technology backs this device.
#[derive(Debug, Clone)]
pub enum GpuVendor {
    /// NVIDIA GPU managed via NVML.
    Nvidia {
        /// Index to pass to `nvml.device_by_index()`.
        nvml_index: u32,
        name: String,
    },
    /// AMD GPU on Linux, monitored via `/sys/class/drm/cardN/device/`.
    AmdLinux { card_device_path: PathBuf, name: String },
    /// Intel GPU on Linux, monitored via `/sys/class/drm/cardN/`.
    IntelLinux { card_device_path: PathBuf, name: String },
}

/// A single discovered GPU entry.
#[derive(Debug, Clone)]
pub struct DiscoveredGpu {
    /// Slot index in `SharedState.latest_gpus`.
    pub gpu_index: usize,
    pub vendor: GpuVendor,
}

impl DiscoveredGpu {
    pub fn display_name(&self) -> &str {
        match &self.vendor {
            GpuVendor::Nvidia { name, .. } | GpuVendor::AmdLinux { name, .. } | GpuVendor::IntelLinux { name, .. } => name,
        }
    }
}

/// A discovered network interface that we want to track.
#[derive(Debug, Clone)]
pub struct DiscoveredInterface {
    /// Slot index in `SharedState.latest_networks`.
    pub iface_index: usize,
    pub name: String,
}

/// Everything discovered at startup.  Workers receive an `Arc` of this so
/// they know which GPUs / interfaces to poll.
#[derive(Debug, Clone, Default)]
pub struct RuntimeDiscovery {
    pub gpus: Vec<DiscoveredGpu>,
    pub interfaces: Vec<DiscoveredInterface>,
}

impl RuntimeDiscovery {
    pub fn gpu_count(&self) -> usize {
        self.gpus.len()
    }
    pub fn interface_count(&self) -> usize {
        self.interfaces.len()
    }
}

// ─── GPU discovery ────────────────────────────────────────────────────────────

/// Discover all supported GPUs: first NVIDIA (via NVML), then AMD/Intel (Linux sysfs).
pub fn discover_gpus() -> Vec<DiscoveredGpu> {
    let mut gpus: Vec<DiscoveredGpu> = Vec::new();

    discover_nvidia_gpus(&mut gpus);

    #[cfg(target_os = "linux")]
    discover_amd_intel_gpus_linux(&mut gpus);

    if gpus.is_empty() {
        info!("No supported GPUs discovered");
    } else {
        for gpu in &gpus {
            info!("Discovered GPU {}: {} ({:?})", gpu.gpu_index, gpu.display_name(), gpu.vendor_kind());
        }
    }

    gpus
}

fn discover_nvidia_gpus(gpus: &mut Vec<DiscoveredGpu>) {
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            info!("NVML unavailable ({e}), skipping NVIDIA GPU discovery");
            return;
        }
    };

    let count = match nvml.device_count() {
        Ok(n) => n,
        Err(e) => {
            warn!("Failed to query NVIDIA device count: {e}");
            return;
        }
    };

    for i in 0..count {
        match nvml.device_by_index(i) {
            Ok(device) => {
                let name = device.name().unwrap_or_else(|_| format!("NVIDIA GPU {i}"));
                let gpu_index = gpus.len();
                gpus.push(DiscoveredGpu {
                    gpu_index,
                    vendor: GpuVendor::Nvidia { nvml_index: i, name },
                });
            }
            Err(e) => warn!("Failed to access NVIDIA GPU {i}: {e}"),
        }
    }
}

#[cfg(target_os = "linux")]
fn discover_amd_intel_gpus_linux(gpus: &mut Vec<DiscoveredGpu>) {
    use std::fs;

    let drm_path = std::path::Path::new("/sys/class/drm");
    let Ok(entries) = fs::read_dir(drm_path) else {
        return;
    };

    // Collect and sort card paths for deterministic ordering.
    let mut card_paths: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            // Only "cardN" — not "cardN-HDMI-A-1" connectors or "renderDN".
            n.starts_with("card") && n.chars().skip(4).all(|c| c.is_ascii_digit())
        })
        .map(|e| e.path())
        .collect();
    card_paths.sort();

    for card_path in card_paths {
        let device_path = card_path.join("device");
        if !device_path.exists() {
            continue;
        }

        let vendor_str = match fs::read_to_string(device_path.join("vendor")) {
            Ok(s) => s.trim().to_lowercase(),
            Err(_) => continue,
        };

        // Skip NVIDIA — handled by NVML.
        if vendor_str == "0x10de" {
            continue;
        }

        let card_name = card_path.file_name().unwrap_or_default().to_string_lossy().to_string();

        match vendor_str.as_str() {
            "0x1002" => {
                // AMD
                if !device_path.join("gpu_busy_percent").exists() {
                    info!("AMD card {card_name}: gpu_busy_percent not available, skipping");
                    continue;
                }
                let name = resolve_gpu_name(&device_path, &format!("AMD GPU ({card_name})"));
                let gpu_index = gpus.len();
                gpus.push(DiscoveredGpu {
                    gpu_index,
                    vendor: GpuVendor::AmdLinux {
                        card_device_path: device_path,
                        name,
                    },
                });
            }
            "0x8086" => {
                // Intel
                let name = resolve_gpu_name(&device_path, &format!("Intel GPU ({card_name})"));
                let gpu_index = gpus.len();
                gpus.push(DiscoveredGpu {
                    gpu_index,
                    vendor: GpuVendor::IntelLinux {
                        card_device_path: device_path,
                        name,
                    },
                });
            }
            _ => {} // Unknown vendor
        }
    }
}

#[cfg(target_os = "linux")]
fn read_pci_label(device_path: &std::path::Path) -> Option<String> {
    let label_path = device_path.join("label");
    if let Ok(s) = std::fs::read_to_string(&label_path) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// Try to look up the GPU name from the system PCI IDs database.
/// Reads vendor/device/subsystem IDs from sysfs and searches `/usr/share/hwdata/pci.ids`.
#[cfg(target_os = "linux")]
fn lookup_pci_name(device_path: &std::path::Path) -> Option<String> {
    let vendor_raw = std::fs::read_to_string(device_path.join("vendor")).ok()?;
    let device_raw = std::fs::read_to_string(device_path.join("device")).ok()?;

    let vendor_id = vendor_raw.trim().trim_start_matches("0x").to_lowercase();
    let device_id = device_raw.trim().trim_start_matches("0x").to_lowercase();

    let sub_vendor = std::fs::read_to_string(device_path.join("subsystem_vendor"))
        .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
        .unwrap_or_default();
    let sub_device = std::fs::read_to_string(device_path.join("subsystem_device"))
        .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
        .unwrap_or_default();

    for pci_ids_path in &["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"] {
        if let Ok(content) = std::fs::read_to_string(pci_ids_path) {
            if let Some(name) = search_pci_ids(&content, &vendor_id, &device_id, &sub_vendor, &sub_device) {
                return Some(name);
            }
        }
    }
    None
}

/// Parse the PCI IDs database file to find the most specific GPU name.
/// Format: vendor lines (no indent), device lines (\t), subsystem lines (\t\t).
#[cfg(target_os = "linux")]
fn search_pci_ids(content: &str, vendor: &str, device: &str, sub_vendor: &str, sub_device: &str) -> Option<String> {
    let mut in_vendor = false;
    let mut device_name: Option<String> = None;
    let mut past_device = false;

    for line in content.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        if !line.starts_with('\t') {
            // Vendor line
            if in_vendor && past_device {
                // We've moved past our vendor, return the device name
                return device_name;
            }
            if in_vendor && !past_device {
                // Different vendor — stop
                return None;
            }
            let id = line.split_whitespace().next().unwrap_or("");
            in_vendor = id.eq_ignore_ascii_case(vendor);
        } else if in_vendor && !line.starts_with("\t\t") {
            // Device line under our vendor
            if past_device {
                // Moved to a different device — return device name (no subsystem match)
                return device_name;
            }
            let trimmed = line.trim_start_matches('\t');
            let id = trimmed.split_whitespace().next().unwrap_or("");
            if id.eq_ignore_ascii_case(device) {
                let rest = trimmed[id.len()..].trim();
                device_name = Some(rest.to_string());
                past_device = true;
            }
        } else if in_vendor && past_device && line.starts_with("\t\t") {
            // Subsystem line: "\t\tsubvend subdev  name"
            if sub_vendor.is_empty() || sub_device.is_empty() {
                continue;
            }
            let trimmed = line.trim_start_matches('\t');
            let mut parts = trimmed.split_whitespace();
            let sv = parts.next().unwrap_or("");
            let sd = parts.next().unwrap_or("");
            if sv.eq_ignore_ascii_case(sub_vendor) && sd.eq_ignore_ascii_case(sub_device) {
                let rest: Vec<_> = parts.collect();
                return Some(rest.join(" "));
            }
        }
    }

    device_name
}

/// Best-effort GPU name: label file → pci.ids → fallback string.
#[cfg(target_os = "linux")]
fn resolve_gpu_name(device_path: &std::path::Path, fallback: &str) -> String {
    read_pci_label(device_path)
        .or_else(|| lookup_pci_name(device_path))
        .unwrap_or_else(|| fallback.to_string())
}

impl DiscoveredGpu {
    pub fn vendor_kind(&self) -> &'static str {
        match &self.vendor {
            GpuVendor::Nvidia { .. } => "NVIDIA",
            GpuVendor::AmdLinux { .. } => "AMD (Linux sysfs)",
            GpuVendor::IntelLinux { .. } => "Intel (Linux sysfs)",
        }
    }
}

// ─── Network interface discovery ─────────────────────────────────────────────

/// Interface name prefixes considered "virtual" / container-related.
const VIRTUAL_PREFIXES: &[&str] = &["docker", "veth", "br-", "virbr", "tun", "tap", "dummy", "lo"];

/// Discover "real" network interfaces to track.
///
/// Excludes loopback and virtual/container interfaces.  The set discovered at
/// startup is used for the lifetime of the collection run.
pub fn discover_interfaces() -> Vec<DiscoveredInterface> {
    let networks = Networks::new_with_refreshed_list();

    let mut interfaces: Vec<DiscoveredInterface> = networks
        .iter()
        .filter(|(name, _)| is_real_interface(name))
        .enumerate()
        .map(|(idx, (name, _))| DiscoveredInterface {
            iface_index: idx,
            name: name.clone(),
        })
        .collect();

    // Sort by name for deterministic column ordering.
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, iface) in interfaces.iter_mut().enumerate() {
        iface.iface_index = i;
    }

    for iface in &interfaces {
        info!("Discovered network interface {}: {}", iface.iface_index, iface.name);
    }

    interfaces
}

fn is_real_interface(name: &str) -> bool {
    for prefix in VIRTUAL_PREFIXES {
        if name == *prefix || name.starts_with(prefix) {
            return false;
        }
    }
    true
}
