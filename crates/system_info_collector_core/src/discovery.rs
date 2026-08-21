use std::path::PathBuf;

use log::{info, warn};
use sysinfo::{Disks, Networks};

use crate::disk_stats::device_stat_name;

/// Which GPU technology backs this device.
#[derive(Debug, Clone)]
pub enum GpuVendor {
    /// NVIDIA GPU managed via NVML.
    Nvidia {
        /// Index to pass to `nvml.device_by_index()`.
        nvml_index: u32,
        name: String,
    },
    /// AMD GPU on Linux, monitored via `amdgpu-sysfs` (`/sys/class/drm/cardN/device/`).
    AmdLinux {
        handle: amdgpu_sysfs::gpu_handle::GpuHandle,
        name: String,
    },
    /// Intel GPU on Linux, monitored via `/sys/class/drm/cardN/`.
    IntelLinux { card_device_path: PathBuf, name: String },
}

/// A single discovered GPU entry.
#[derive(Debug, Clone)]
pub struct DiscoveredGpu {
    /// Slot index in `SharedState.latest_gpus`.
    pub gpu_index: usize,
    pub vendor: GpuVendor,
    /// Total VRAM in megabytes (0 if unknown).
    pub vram_total_mb: u64,
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
    /// Connection type: "WiFi", "Ethernet" or "Virtual" - "Unknown" outside Linux.
    pub kind: String,
    /// Adapter model from the PCI database, or the kernel driver name as a fallback.
    pub model: Option<String>,
    /// Negotiated link speed in Mb/s, when the driver reports one.
    pub speed_mbps: Option<u64>,
}

impl DiscoveredInterface {
    /// Label for charts and the live view, e.g. `wlan0 (WiFi - Wi-Fi 6 AX201)`.
    pub fn display_label(&self) -> String {
        let mut details = vec![self.kind.clone()];
        if let Some(model) = &self.model {
            details.push(model.clone());
        }
        if let Some(speed) = self.speed_mbps {
            details.push(format!("{speed} Mb/s"));
        }
        sanitize_label(&format!("{} ({})", self.name, details.join(" - ")))
    }
}

/// Labels end up in the comma-separated CSV metadata line, so they must not
/// contain commas themselves.
fn sanitize_label(label: &str) -> String {
    label.replace(',', " ")
}

/// Filesystem types considered virtual/pseudo — excluded from disk tracking.
const VIRTUAL_FS_TYPES: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "debugfs",
    "securityfs",
    "tracefs",
    "pstore",
    "hugetlbfs",
    "mqueue",
    "autofs",
    "fusectl",
    "efivarfs",
    "bpf",
    "ramfs",
    "overlay",
    "squashfs",
    "devpts",
    "configfs",
];

/// Mount points left out of `--all-disks`: boot and EFI partitions are tiny, never
/// change and are not what anyone watches disk usage for.  `--disk /boot` still works.
const AUTO_SKIPPED_MOUNTS: &[&str] = &["/boot", "/efi"];

/// True when `mount_point` is `pattern` itself or sits below it.
fn mount_matches(mount_point: &str, pattern: &str) -> bool {
    mount_point == pattern || mount_point.starts_with(&format!("{}/", pattern.trim_end_matches('/')))
}

/// A single disk mount point the user wants to track.
#[derive(Debug, Clone)]
pub struct DiscoveredDisk {
    /// Slot index used for column naming (DISK_N_USED_MB / DISK_N_AVAIL_MB).
    pub disk_index: usize,
    /// Mount point path string, e.g. "/" or "/home/rafal/Projekty".
    pub mount_point: String,
    /// Device path as reported by sysinfo, e.g. "/dev/nvme0n1p6".
    pub device: String,
    /// Kernel device name to look up in `/proc/diskstats`, e.g. "nvme0n1p6".
    /// `None` when the device has no I/O counters, which disables its I/O metrics.
    pub io_stat_name: Option<String>,
    /// Total filesystem size in GB (0 when unknown).
    pub total_gb: u64,
}

impl DiscoveredDisk {
    /// Label for charts and the live view, e.g. `/home (nvme1n1 916 GB)`.
    pub fn display_label(&self) -> String {
        let device = short_device_name(&self.device);
        let label = if self.total_gb > 0 {
            format!("{} ({device} {} GB)", self.mount_point, self.total_gb)
        } else {
            format!("{} ({device})", self.mount_point)
        };
        sanitize_label(&label)
    }
}

/// `/dev/nvme0n1p6` → `nvme0n1p6`, leaving names like `overlay` untouched.
fn short_device_name(device: &str) -> String {
    std::path::Path::new(device)
        .file_name()
        .map_or_else(|| device.to_string(), |name| name.to_string_lossy().into_owned())
}

/// Everything discovered at startup.  Workers receive an `Arc` of this so
/// they know which GPUs / interfaces to poll.
#[derive(Debug, Clone, Default)]
pub struct RuntimeDiscovery {
    pub gpus: Vec<DiscoveredGpu>,
    pub interfaces: Vec<DiscoveredInterface>,
    pub disks: Vec<DiscoveredDisk>,
}

impl RuntimeDiscovery {
    pub fn gpu_count(&self) -> usize {
        self.gpus.len()
    }
    pub fn interface_count(&self) -> usize {
        self.interfaces.len()
    }
    pub fn disk_count(&self) -> usize {
        self.disks.len()
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
                let vram_total_mb = device.memory_info().map_or(0, |m| m.total / 1024 / 1024);
                let gpu_index = gpus.len();
                gpus.push(DiscoveredGpu {
                    gpu_index,
                    vendor: GpuVendor::Nvidia { nvml_index: i, name },
                    vram_total_mb,
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
                let handle = match amdgpu_sysfs::gpu_handle::GpuHandle::new_from_path(device_path.clone()) {
                    Ok(h) => h,
                    Err(e) => {
                        info!("AMD card {card_name}: failed to open sysfs handle ({e}), skipping");
                        continue;
                    }
                };
                if handle.get_busy_percent().is_err() {
                    info!("AMD card {card_name}: gpu_busy_percent not available, skipping");
                    continue;
                }
                let name = resolve_gpu_name(&device_path, &format!("AMD GPU ({card_name})"));
                let vram_total_mb = handle.get_total_vram().unwrap_or(0) / 1024 / 1024;
                let gpu_index = gpus.len();
                gpus.push(DiscoveredGpu {
                    gpu_index,
                    vendor: GpuVendor::AmdLinux { handle, name },
                    vram_total_mb,
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
                    vram_total_mb: 0,
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
        if let Ok(content) = std::fs::read_to_string(pci_ids_path)
            && let Some(name) = search_pci_ids(&content, &vendor_id, &device_id, &sub_vendor, &sub_device)
        {
            return Some(name);
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
                #[expect(clippy::string_slice)]
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

/// Best-effort name of any PCI device: sysfs label file → pci.ids database.
#[cfg(target_os = "linux")]
fn resolve_pci_device_name(device_path: &std::path::Path) -> Option<String> {
    read_pci_label(device_path).or_else(|| lookup_pci_name(device_path))
}

#[cfg(target_os = "linux")]
fn resolve_gpu_name(device_path: &std::path::Path, fallback: &str) -> String {
    resolve_pci_device_name(device_path).unwrap_or_else(|| fallback.to_string())
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

// ─── Disk discovery ───────────────────────────────────────────────────────────

/// A mounted filesystem as reported by sysinfo.
struct MountedDisk {
    device: String,
    mount_point: String,
    total_gb: u64,
}

/// Returns all real (non-virtual) mounted filesystems.
fn real_disks(disks: &Disks) -> Vec<MountedDisk> {
    disks
        .iter()
        .filter(|d| {
            let fs = d.file_system().to_string_lossy().to_lowercase();
            !VIRTUAL_FS_TYPES.contains(&fs.as_str())
        })
        .map(|d| MountedDisk {
            device: d.name().to_string_lossy().to_string(),
            mount_point: d.mount_point().to_string_lossy().to_string(),
            total_gb: d.total_space() / 1_073_741_824,
        })
        .collect()
}

/// Whether `--all-disks` should track this filesystem, i.e. it is neither a boot/EFI
/// partition nor excluded by `--exclude-disk`.
fn is_auto_tracked(mount: &MountedDisk, excluded: &[String]) -> bool {
    let skipped_by = AUTO_SKIPPED_MOUNTS
        .iter()
        .find(|pattern| mount_matches(&mount.mount_point, pattern))
        .map(|pattern| (*pattern).to_string())
        .or_else(|| {
            excluded
                .iter()
                .find(|pattern| mount_matches(&mount.mount_point, pattern) || &mount.device == *pattern)
                .cloned()
        });

    match skipped_by {
        Some(pattern) => {
            info!("Skipping disk {} ({}) - excluded by \"{pattern}\"", mount.mount_point, mount.device);
            false
        }
        None => true,
    }
}

/// One entry per device, keeping its shortest mount point - bind mounts and btrfs subvolumes
/// otherwise report the same physical disk under a dozen different mount points.
fn unique_devices<'a>(mounts: &[&'a MountedDisk]) -> Vec<&'a MountedDisk> {
    let mut unique: Vec<&'a MountedDisk> = Vec::new();
    for mount in mounts.iter().copied() {
        match unique.iter_mut().find(|kept| kept.device == mount.device) {
            Some(kept) => {
                if mount.mount_point.len() < kept.mount_point.len() {
                    *kept = mount;
                }
            }
            None => unique.push(mount),
        }
    }
    unique
}

/// Print available disks to the log (used by --list-disks).
pub fn list_real_disks(excluded: &[String]) {
    let disks = Disks::new_with_refreshed_list();
    let mounts = real_disks(&disks);
    if mounts.is_empty() {
        info!("No real disks found");
        return;
    }
    info!("Available disks:");
    for mount in &mounts {
        info!("  device: {}  mount: {}  size: {} GB", mount.device, mount.mount_point, mount.total_gb);
    }
    let auto_tracked: Vec<&MountedDisk> = mounts.iter().filter(|m| is_auto_tracked(m, excluded)).collect();
    info!("Disks tracked by --all-disks:");
    for mount in unique_devices(&auto_tracked) {
        info!("  device: {}  mount: {}  size: {} GB", mount.device, mount.mount_point, mount.total_gb);
    }
}

/// Discover disks to track.
///
/// If `all_disks` is true, tracks every real (non-virtual) disk once per device,
/// minus boot/EFI partitions and anything listed in `excluded`.
/// Otherwise each entry in `requested` is matched against either the device
/// name (e.g. `/dev/sda1`) or the mount point (e.g. `/home`) - an explicitly
/// requested disk is always tracked, exclusions only apply to `all_disks`.
pub fn discover_disks(requested: &[String], all_disks: bool, excluded: &[String]) -> Vec<DiscoveredDisk> {
    if !all_disks && requested.is_empty() {
        return vec![];
    }

    let sysinfo_disks = Disks::new_with_refreshed_list();
    let mounts = real_disks(&sysinfo_disks);

    let available_display: Vec<String> = mounts.iter().map(|m| format!("{} ({})", m.device, m.mount_point)).collect();
    info!("Available disks: {}", available_display.join(", "));

    let mut discovered: Vec<DiscoveredDisk> = Vec::new();

    if all_disks {
        let auto_tracked: Vec<&MountedDisk> = mounts.iter().filter(|m| is_auto_tracked(m, excluded)).collect();
        for mount in unique_devices(&auto_tracked) {
            discovered.push(track_disk(discovered.len(), mount));
        }
    } else {
        for req in requested {
            // Match by mount point OR device name.
            match mounts.iter().find(|m| &m.mount_point == req || &m.device == req) {
                Some(mount) => discovered.push(track_disk(discovered.len(), mount)),
                None => {
                    warn!("Requested disk \"{req}\" not found (available: {})", available_display.join(", "));
                }
            }
        }
    }

    discovered
}

fn track_disk(disk_index: usize, mount: &MountedDisk) -> DiscoveredDisk {
    let disk = DiscoveredDisk {
        disk_index,
        mount_point: mount.mount_point.clone(),
        device: mount.device.clone(),
        io_stat_name: device_stat_name(&mount.device),
        total_gb: mount.total_gb,
    };
    match &disk.io_stat_name {
        Some(name) => info!("Tracking disk {disk_index}: {} (I/O counters from {name})", disk.display_label()),
        None => info!("Tracking disk {disk_index}: {} (no I/O counters available)", disk.display_label()),
    }
    disk
}

// ─── Network interface discovery ─────────────────────────────────────────────

/// Print available real network interfaces to the log (used by --list-networks).
pub fn list_real_interfaces(excluded: &[String]) {
    let networks = Networks::new_with_refreshed_list();
    let mut names: Vec<&str> = networks
        .iter()
        .filter(|(name, _)| is_real_interface(name))
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        info!("No real network interfaces found");
        return;
    }
    info!("Available network interfaces:");
    for name in names {
        let excluded_note = if excluded.iter().any(|e| e == name) { " - excluded" } else { "" };
        info!("  {}{excluded_note}", describe_interface(0, name).display_label());
    }
}

/// Connection type, adapter model and link speed of one interface.
/// Everything but the name is best-effort and only available on Linux.
fn describe_interface(iface_index: usize, name: &str) -> DiscoveredInterface {
    #[cfg(target_os = "linux")]
    let (kind, model, speed_mbps) = interface_details_linux(name);
    #[cfg(not(target_os = "linux"))]
    let (kind, model, speed_mbps) = ("Unknown".to_string(), None, None);

    DiscoveredInterface {
        iface_index,
        name: name.to_string(),
        kind,
        model,
        speed_mbps,
    }
}

#[cfg(target_os = "linux")]
fn interface_details_linux(name: &str) -> (String, Option<String>, Option<u64>) {
    let sys_path = std::path::Path::new("/sys/class/net").join(name);
    let device_path = sys_path.join("device");

    // Wireless cards are ARPHRD_ETHER like wired ones, so the wireless directories
    // are what tells them apart.
    let kind = if sys_path.join("wireless").exists() || sys_path.join("phy80211").exists() {
        "WiFi"
    } else if device_path.exists() {
        "Ethernet"
    } else {
        "Virtual"
    };

    let model = resolve_pci_device_name(&device_path).or_else(|| driver_name(&device_path));
    // The speed file reads -1 (or fails) while the link is down.
    let speed_mbps = std::fs::read_to_string(sys_path.join("speed"))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&speed| speed > 0)
        .map(|speed| speed as u64);

    (kind.to_string(), model, speed_mbps)
}

/// Kernel module bound to the device, e.g. `iwlwifi` or `r8169`.
#[cfg(target_os = "linux")]
fn driver_name(device_path: &std::path::Path) -> Option<String> {
    let driver = std::fs::read_link(device_path.join("driver")).ok()?;
    Some(driver.file_name()?.to_string_lossy().into_owned())
}

/// Discover network interfaces to track.
///
/// If `all_networks` is true: discovers all real (non-virtual) interfaces, minus
/// the ones listed in `excluded`.
/// If `all_networks` is false and `requested` is non-empty: discovers only the
/// interfaces whose names are in `requested`, warning about any not found.
/// If `all_networks` is false and `requested` is empty: returns nothing.
pub fn discover_interfaces(requested: &[String], all_networks: bool, excluded: &[String]) -> Vec<DiscoveredInterface> {
    if !all_networks && requested.is_empty() {
        return vec![];
    }

    let networks = Networks::new_with_refreshed_list();

    let mut names: Vec<String> = if all_networks {
        networks
            .iter()
            .filter(|(name, _)| is_real_interface(name))
            .filter(|(name, _)| {
                let keep = !excluded.contains(name);
                if !keep {
                    info!("Skipping network interface {name} - excluded");
                }
                keep
            })
            .map(|(name, _)| name.clone())
            .collect()
    } else {
        let mut result = Vec::new();
        let available: Vec<&str> = networks
            .iter()
            .filter(|(name, _)| is_real_interface(name))
            .map(|(name, _)| name.as_str())
            .collect();
        for req in requested {
            if available.contains(&req.as_str()) {
                result.push(req.clone());
            } else {
                warn!("Requested network interface \"{req}\" not found (available: {})", available.join(", "));
            }
        }
        result
    };

    // Sort by name for deterministic column ordering, then index in that order.
    names.sort_unstable();
    let interfaces: Vec<DiscoveredInterface> = names.iter().enumerate().map(|(index, name)| describe_interface(index, name)).collect();

    for iface in &interfaces {
        info!("Discovered network interface {}: {}", iface.iface_index, iface.display_label());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(mount_point: &str, device: &str, total_gb: u64) -> DiscoveredDisk {
        DiscoveredDisk {
            disk_index: 0,
            mount_point: mount_point.to_string(),
            device: device.to_string(),
            io_stat_name: None,
            total_gb,
        }
    }

    #[test]
    fn labels_disks_with_device_and_size() {
        assert_eq!(disk("/home", "/dev/nvme1n1", 915).display_label(), "/home (nvme1n1 915 GB)");
        // Size is left out when the filesystem does not report one.
        assert_eq!(disk("/mnt/share", "//server/share", 0).display_label(), "/mnt/share (share)");
    }

    fn mounted(mount_point: &str, device: &str) -> MountedDisk {
        MountedDisk {
            device: device.to_string(),
            mount_point: mount_point.to_string(),
            total_gb: 100,
        }
    }

    #[test]
    fn keeps_boot_partitions_out_of_all_disks() {
        assert!(!is_auto_tracked(&mounted("/boot", "/dev/nvme0n1p2"), &[]));
        assert!(!is_auto_tracked(&mounted("/boot/efi", "/dev/nvme0n1p1"), &[]));
        assert!(is_auto_tracked(&mounted("/", "/dev/nvme0n1p3"), &[]));
        // A path that merely starts with the same letters is a different mount.
        assert!(is_auto_tracked(&mounted("/bootcamp", "/dev/sda1"), &[]));
    }

    #[test]
    fn honours_user_exclusions() {
        let excluded = vec!["/mnt/backup".to_string(), "/dev/sdb1".to_string()];
        assert!(!is_auto_tracked(&mounted("/mnt/backup", "/dev/sdc1"), &excluded));
        assert!(!is_auto_tracked(&mounted("/mnt/backup/old", "/dev/sdc1"), &excluded));
        assert!(!is_auto_tracked(&mounted("/data", "/dev/sdb1"), &excluded));
        assert!(is_auto_tracked(&mounted("/mnt/media", "/dev/sdd1"), &excluded));
    }

    #[test]
    fn labels_interfaces_with_what_is_known() {
        let wifi = DiscoveredInterface {
            iface_index: 0,
            name: "wlan0".to_string(),
            kind: "WiFi".to_string(),
            // Commas would break the CSV metadata line they are written to.
            model: Some("Wi-Fi 6 AX201, 160MHz".to_string()),
            speed_mbps: None,
        };
        assert_eq!(wifi.display_label(), "wlan0 (WiFi - Wi-Fi 6 AX201  160MHz)");

        let wired = DiscoveredInterface {
            iface_index: 1,
            name: "eth0".to_string(),
            kind: "Ethernet".to_string(),
            model: None,
            speed_mbps: Some(1000),
        };
        assert_eq!(wired.display_label(), "eth0 (Ethernet - 1000 Mb/s)");
    }
}
