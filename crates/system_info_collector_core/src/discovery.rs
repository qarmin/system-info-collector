use log::{info, warn};
use serde::Serialize;
use system_info_probe::disk::{self, DiskNote, DiskQuery, Filesystem, TrackedDisk};
use system_info_probe::gpu::{self, Gpu, GpuDiscovery};
use system_info_probe::network::{self, Interface, InterfaceQuery};

/// A GPU being tracked, at its column index.
#[derive(Debug, Clone)]
pub struct DiscoveredGpu {
    pub gpu_index: usize,
    pub gpu: Gpu,
}

impl DiscoveredGpu {
    pub fn display_name(&self) -> &str {
        &self.gpu.name
    }

    pub fn vendor_kind(&self) -> &'static str {
        self.gpu.kind.vendor()
    }

    /// 0 when unknown.
    pub fn vram_total_mb(&self) -> u64 {
        self.gpu.vram_total_bytes.map_or(0, |b| b / 1024 / 1024)
    }

    /// Empty when unknown.
    pub fn driver_version(&self) -> &str {
        self.gpu.driver_version.as_deref().unwrap_or_default()
    }
}

/// A network interface being tracked, at its column index.
#[derive(Debug, Clone)]
pub struct DiscoveredInterface {
    pub iface_index: usize,
    pub interface: Interface,
}

impl DiscoveredInterface {
    pub fn name(&self) -> &str {
        &self.interface.name
    }

    /// Label for charts and the live view, e.g. `wlan0 (WiFi - Wi-Fi 6 AX201)`.
    pub fn display_label(&self) -> String {
        let interface = &self.interface;
        let mut details = vec![interface.kind.label().to_string()];
        if let Some(model) = &interface.model {
            details.push(model.clone());
        }
        if let Some(speed) = interface.speed_mbps {
            details.push(format!("{speed} Mb/s"));
        }
        sanitize_label(&format!("{} ({})", interface.name, details.join(" - ")))
    }
}

/// Labels end up in the comma-separated CSV metadata line, so they must not contain commas.
fn sanitize_label(label: &str) -> String {
    label.replace(',', " ")
}

/// A disk mount point being tracked, at its column index.
#[derive(Debug, Clone)]
pub struct DiscoveredDisk {
    pub disk_index: usize,
    pub tracked: TrackedDisk,
}

impl DiscoveredDisk {
    pub fn mount_point(&self) -> &str {
        &self.tracked.filesystem.mount_point
    }

    pub fn device(&self) -> &str {
        &self.tracked.filesystem.device
    }

    pub fn total_gb(&self) -> u64 {
        self.tracked.filesystem.total_bytes / 1_073_741_824
    }

    /// Label for charts and the live view, e.g. `/home (nvme1n1 916 GB)`.
    pub fn display_label(&self) -> String {
        let device = self
            .tracked
            .stat_name
            .clone()
            .unwrap_or_else(|| disk::short_device_name(self.device()).to_string());
        let label = match self.total_gb() {
            0 => format!("{} ({device})", self.mount_point()),
            total => format!("{} ({device} {total} GB)", self.mount_point()),
        };
        sanitize_label(&label)
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredPingTarget {
    pub ping_index: usize,
    /// Host name, IP, or `gateway`.
    pub host: String,
}

/// Everything discovered at startup.
#[derive(Debug, Clone, Default)]
pub struct RuntimeDiscovery {
    pub gpus: Vec<DiscoveredGpu>,
    pub interfaces: Vec<DiscoveredInterface>,
    pub disks: Vec<DiscoveredDisk>,
    /// Empty when no ping metric was requested or pinging is unavailable.
    pub ping_targets: Vec<DiscoveredPingTarget>,
    /// Problems worth showing to the user, e.g. a requested metric that cannot be collected.
    pub warnings: Vec<String>,
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

/// Reading the Vivante idle register through /dev/mem while galcore has the GPU power-gated hangs
/// the whole SoC, so Vivante GPUs are not looked for.
pub fn discover_gpus() -> Vec<DiscoveredGpu> {
    let gpus: Vec<DiscoveredGpu> = gpu::discover(GpuDiscovery {
        allow_vivante_register_reads: false,
    })
    .into_iter()
    .enumerate()
    .map(|(gpu_index, gpu)| DiscoveredGpu { gpu_index, gpu })
    .collect();
    if gpus.is_empty() {
        info!("No supported GPUs discovered");
    }
    for gpu in &gpus {
        info!(
            "Discovered GPU {}: {} ({}), driver {}",
            gpu.gpu_index,
            gpu.display_name(),
            gpu.vendor_kind(),
            gpu.driver_version()
        );
    }
    gpus
}

/// A mounted filesystem as shown in the live view, regardless of whether its usage is tracked.
#[derive(Debug, Clone, Serialize)]
pub struct MountedFilesystem {
    pub mount_point: String,
    pub device: String,
    /// Filesystem type as reported by the OS, e.g. `ext4`, `btrfs`, `ntfs`.
    pub file_system: String,
    pub total_bytes: u64,
    /// Hardware model of the drive, e.g. `Samsung SSD 980 1TB` - Linux only, and absent for
    /// LVM/RAID mappings.
    pub model: Option<String>,
    /// `DISK_N` index when this filesystem is tracked, `None` when it is only listed.
    pub tracked_index: Option<usize>,
}

impl MountedFilesystem {
    /// One-line summary, e.g. `/home - 915.8 GiB ext4, nvme1n1 (Samsung SSD 980 1TB)`.
    pub fn describe(&self) -> String {
        let size = humansize::format_size(self.total_bytes, humansize::BINARY);
        let model = self.model.as_ref().map_or_else(String::new, |m| format!(" ({m})"));
        format!(
            "{} - {size} {}, {}{model}",
            self.mount_point,
            self.file_system,
            disk::short_device_name(&self.device)
        )
    }
}

/// Every attached filesystem, one entry per device, sorted by mount point, with the `DISK_N` index
/// of the tracked ones.  Boot partitions and excluded mounts are listed all the same.
pub fn mounted_filesystems(tracked: &[DiscoveredDisk]) -> Vec<MountedFilesystem> {
    let all = disk::filesystems();
    let mut listed: Vec<MountedFilesystem> = disk::unique_devices(&all)
        .into_iter()
        .map(|fs| MountedFilesystem {
            mount_point: fs.mount_point.clone(),
            device: fs.device.clone(),
            file_system: fs.file_system.clone(),
            total_bytes: fs.total_bytes,
            model: disk::device_model(&fs.device),
            tracked_index: tracked
                .iter()
                .find(|d| d.device() == fs.device || d.mount_point() == fs.mount_point)
                .map(|d| d.disk_index),
        })
        .collect();
    listed.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    listed
}

fn describe_filesystem(fs: &Filesystem) -> String {
    format!(
        "device: {}  mount: {}  size: {} GB  fs: {}",
        fs.device,
        fs.mount_point,
        fs.total_bytes / 1_073_741_824,
        fs.file_system
    )
}

/// Print available disks to the log (used by --list-disks).
pub fn list_real_disks(excluded: &[String]) {
    let all = disk::filesystems();
    if all.is_empty() {
        info!("No real disks found");
        return;
    }
    info!("Available disks:");
    for fs in &all {
        info!("  {}", describe_filesystem(fs));
    }
    info!("Disks tracked by --all-disks:");
    let (tracked, notes) = disk::select(&all, &DiskQuery::All { excluded: excluded.to_vec() });
    log_disk_notes(&notes);
    for disk in tracked {
        info!("  {}", describe_filesystem(&disk.filesystem));
    }
}

fn log_disk_notes(notes: &[DiskNote]) {
    for note in notes {
        match note {
            DiskNote::Skipped {
                mount_point,
                device,
                because,
            } => info!("Skipping disk {mount_point} ({device}) - excluded by \"{because}\""),
            DiskNote::NotFound { requested } => warn!("Requested disk \"{requested}\" not found"),
        }
    }
}

/// Disks to track: with `all_disks` every real disk once per device, minus boot/EFI/recovery
/// partitions and `excluded`; otherwise each entry of `requested` matched against a mount point
/// or device name - an explicitly requested disk is always tracked.
pub fn discover_disks(requested: &[String], all_disks: bool, excluded: &[String]) -> Vec<DiscoveredDisk> {
    if !all_disks && requested.is_empty() {
        return vec![];
    }
    let all = disk::filesystems();
    info!(
        "Available disks: {}",
        all.iter()
            .map(|fs| format!("{} ({})", fs.device, fs.mount_point))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let query = if all_disks {
        DiskQuery::All { excluded: excluded.to_vec() }
    } else {
        DiskQuery::Requested(requested.to_vec())
    };
    let (tracked, notes) = disk::select(&all, &query);
    log_disk_notes(&notes);
    let disks: Vec<DiscoveredDisk> = tracked
        .into_iter()
        .enumerate()
        .map(|(disk_index, tracked)| DiscoveredDisk { disk_index, tracked })
        .collect();
    for disk in &disks {
        match &disk.tracked.stat_name {
            Some(name) => info!("Tracking disk {}: {} (I/O counters from {name})", disk.disk_index, disk.display_label()),
            None => info!("Tracking disk {}: {} (no I/O counters available)", disk.disk_index, disk.display_label()),
        }
    }
    disks
}

/// Print available real network interfaces to the log (used by --list-networks).
pub fn list_real_interfaces(excluded: &[String]) {
    let (interfaces, _) = network::select(&InterfaceQuery::All { excluded: Vec::new() });
    if interfaces.is_empty() {
        info!("No real network interfaces found");
        return;
    }
    info!("Available network interfaces:");
    for interface in interfaces {
        let note = if excluded.contains(&interface.name) { " - excluded" } else { "" };
        let label = DiscoveredInterface { iface_index: 0, interface }.display_label();
        info!("  {label}{note}");
    }
}

/// Interfaces to track, sorted by name: every real one minus `excluded` with `all_networks`,
/// otherwise the `requested` ones that exist.
pub fn discover_interfaces(requested: &[String], all_networks: bool, excluded: &[String]) -> Vec<DiscoveredInterface> {
    if !all_networks && requested.is_empty() {
        return vec![];
    }
    let query = if all_networks {
        InterfaceQuery::All { excluded: excluded.to_vec() }
    } else {
        InterfaceQuery::Requested(requested.to_vec())
    };
    let (found, missing) = network::select(&query);
    for name in missing {
        warn!("Requested network interface \"{name}\" not found");
    }
    let interfaces: Vec<DiscoveredInterface> = found
        .into_iter()
        .enumerate()
        .map(|(iface_index, interface)| DiscoveredInterface { iface_index, interface })
        .collect();
    for interface in &interfaces {
        info!("Discovered network interface {}: {}", interface.iface_index, interface.display_label());
    }
    interfaces
}

#[cfg(test)]
mod tests {
    use system_info_probe::network::InterfaceKind;

    use super::*;

    fn disk(mount_point: &str, device: &str, total_gb: u64) -> DiscoveredDisk {
        DiscoveredDisk {
            disk_index: 0,
            tracked: TrackedDisk {
                filesystem: Filesystem {
                    mount_point: mount_point.to_string(),
                    device: device.to_string(),
                    file_system: "ext4".to_string(),
                    total_bytes: total_gb * 1_073_741_824,
                },
                stat_name: None,
                model: None,
            },
        }
    }

    #[test]
    fn labels_disks_with_device_and_size() {
        assert_eq!(disk("/home", "/dev/nvme1n1", 915).display_label(), "/home (nvme1n1 915 GB)");
        // Size is left out when the filesystem does not report one.
        assert_eq!(disk("/mnt/share", "//server/share", 0).display_label(), "/mnt/share (share)");
    }

    #[test]
    fn labels_interfaces_with_what_is_known() {
        let interface = |name: &str, kind, model: Option<&str>, speed_mbps| DiscoveredInterface {
            iface_index: 0,
            interface: Interface {
                name: name.to_string(),
                kind,
                model: model.map(ToString::to_string),
                speed_mbps,
            },
        };
        // Commas would break the CSV metadata line they are written to.
        assert_eq!(
            interface("wlan0", InterfaceKind::Wifi, Some("Wi-Fi 6 AX201, 160MHz"), None).display_label(),
            "wlan0 (WiFi - Wi-Fi 6 AX201  160MHz)"
        );
        assert_eq!(
            interface("eth0", InterfaceKind::Ethernet, None, Some(1000)).display_label(),
            "eth0 (Ethernet - 1000 Mb/s)"
        );
    }
}
