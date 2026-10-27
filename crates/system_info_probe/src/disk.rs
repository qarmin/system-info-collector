//! Filesystem space and block-device I/O.
//!
//! I/O counters come from `/proc/diskstats`, the source `iostat` reads, so they exist on Linux
//! only; elsewhere the I/O figures are `None`.

use std::collections::HashMap;
use std::time::Instant;

const SECTOR_BYTES: u64 = 512;

/// Filesystem types that are virtual or pseudo and never worth tracking.
pub const VIRTUAL_FS_TYPES: &[&str] = &[
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

/// Boot, EFI and recovery partitions are tiny, never change and are not what anyone watches
/// disk usage for.  Lowercase, matched case-insensitively - recovery volumes are spelled
/// `Recovery` on macOS and Windows.
pub const AUTO_SKIPPED_MOUNTS: &[&str] = &["/boot", "/efi", "/recovery", "/volumes/recovery", "/system/volumes/recovery"];

/// A mounted, non-virtual filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Filesystem {
    pub mount_point: String,
    /// Device path as the OS reports it, e.g. `/dev/nvme0n1p6`.
    pub device: String,
    pub file_system: String,
    pub total_bytes: u64,
}

impl Filesystem {
    /// `/dev/nvme0n1p6` -> `nvme0n1p6`, leaving names like `overlay` untouched.
    pub fn short_device_name(&self) -> &str {
        short_device_name(&self.device)
    }
}

pub fn short_device_name(device: &str) -> &str {
    device.rsplit('/').next().unwrap_or(device)
}

/// Every mounted non-virtual filesystem, in the order the OS lists them.
pub fn filesystems() -> Vec<Filesystem> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| {
            let fs = d.file_system().to_string_lossy().to_lowercase();
            !VIRTUAL_FS_TYPES.contains(&fs.as_str())
        })
        .map(|d| Filesystem {
            device: d.name().to_string_lossy().into_owned(),
            mount_point: d.mount_point().to_string_lossy().into_owned(),
            file_system: d.file_system().to_string_lossy().into_owned(),
            total_bytes: d.total_space(),
        })
        .collect()
}

/// True when `mount_point` is `pattern` itself or sits below it.
pub fn mount_matches(mount_point: &str, pattern: &str) -> bool {
    mount_point == pattern || mount_point.starts_with(&format!("{}/", pattern.trim_end_matches('/')))
}

/// The [`AUTO_SKIPPED_MOUNTS`] entry covering this mount point, if any.  Windows mount points
/// (`D:\Recovery`) are folded into the unix shape first.
pub fn auto_skipped_by(mount_point: &str) -> Option<&'static str> {
    let mut normalized = mount_point.replace('\\', "/").to_lowercase();
    if normalized.chars().nth(1) == Some(':') {
        normalized.drain(..2);
    }
    AUTO_SKIPPED_MOUNTS.iter().copied().find(|pattern| mount_matches(&normalized, pattern))
}

/// One entry per device, keeping its shortest mount point - bind mounts and btrfs subvolumes
/// otherwise report the same disk under a dozen mount points.
pub fn unique_devices<'a>(filesystems: impl IntoIterator<Item = &'a Filesystem>) -> Vec<&'a Filesystem> {
    let mut unique: Vec<&'a Filesystem> = Vec::new();
    for fs in filesystems {
        match unique.iter_mut().find(|kept| kept.device == fs.device) {
            Some(kept) => {
                if fs.mount_point.len() < kept.mount_point.len() {
                    *kept = fs;
                }
            }
            None => unique.push(fs),
        }
    }
    unique
}

/// Which filesystems to track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskQuery {
    /// Every device once, minus [`AUTO_SKIPPED_MOUNTS`] and mount points or devices in
    /// `excluded` (a mount point also excludes everything below it).
    All { excluded: Vec<String> },
    /// Each entry is a mount point or a device path; always tracked, even a boot partition.
    Requested(Vec<String>),
}

/// Why a filesystem was left out, or a request that matched nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskNote {
    Skipped { mount_point: String, device: String, because: String },
    NotFound { requested: String },
}

/// A filesystem chosen for tracking.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackedDisk {
    pub filesystem: Filesystem,
    /// Name in `/proc/diskstats`, e.g. `nvme0n1p6`; `None` when the device has no I/O counters.
    pub stat_name: Option<String>,
    /// Hardware model of the drive, e.g. `Samsung SSD 980 1TB`; Linux only.
    pub model: Option<String>,
}

pub fn select(filesystems: &[Filesystem], query: &DiskQuery) -> (Vec<TrackedDisk>, Vec<DiskNote>) {
    let mut notes = Vec::new();
    let chosen: Vec<&Filesystem> = match query {
        DiskQuery::All { excluded } => {
            let kept: Vec<&Filesystem> = filesystems
                .iter()
                .filter(|fs| {
                    let because = auto_skipped_by(&fs.mount_point).map(ToString::to_string).or_else(|| {
                        excluded
                            .iter()
                            .find(|pattern| mount_matches(&fs.mount_point, pattern) || &fs.device == *pattern)
                            .cloned()
                    });
                    match because {
                        Some(because) => {
                            notes.push(DiskNote::Skipped {
                                mount_point: fs.mount_point.clone(),
                                device: fs.device.clone(),
                                because,
                            });
                            false
                        }
                        None => true,
                    }
                })
                .collect();
            unique_devices(kept)
        }
        DiskQuery::Requested(requested) => requested
            .iter()
            .filter_map(|req| {
                let found = filesystems.iter().find(|fs| &fs.mount_point == req || &fs.device == req);
                if found.is_none() {
                    notes.push(DiskNote::NotFound { requested: req.clone() });
                }
                found
            })
            .collect(),
    };
    let counters = read_counters();
    let tracked = chosen
        .into_iter()
        .map(|fs| TrackedDisk {
            stat_name: stat_name(&fs.device, &fs.mount_point, &counters),
            model: device_model(&fs.device),
            filesystem: fs.clone(),
        })
        .collect();
    (tracked, notes)
}

/// Cumulative counters for one block device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiskCounters {
    pub read_sectors: u64,
    pub write_sectors: u64,
    /// Wall-clock milliseconds with at least one request in flight (`io_ticks`), the basis of
    /// iostat's %util.
    pub busy_ms: u64,
}

impl DiskCounters {
    pub fn read_bytes(&self) -> u64 {
        self.read_sectors * SECTOR_BYTES
    }

    pub fn written_bytes(&self) -> u64 {
        self.write_sectors * SECTOR_BYTES
    }
}

/// Busy share and throughput of one device over an interval, as iostat derives them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiskIoRates {
    pub busy_percent: f64,
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
}

impl DiskIoRates {
    pub fn between(earlier: DiskCounters, later: DiskCounters, elapsed_secs: f64) -> Option<Self> {
        if elapsed_secs <= 0.0 {
            return None;
        }
        // io_ticks can drift a millisecond or two past the window - clamped, not reported above 100%.
        let busy_percent = (later.busy_ms.saturating_sub(earlier.busy_ms) as f64 / (elapsed_secs * 1000.0) * 100.0).clamp(0.0, 100.0);
        Some(Self {
            busy_percent,
            read_bytes_per_sec: later.read_bytes().saturating_sub(earlier.read_bytes()) as f64 / elapsed_secs,
            write_bytes_per_sec: later.written_bytes().saturating_sub(earlier.written_bytes()) as f64 / elapsed_secs,
        })
    }
}

/// Counters for every block device, keyed by kernel device name (`nvme0n1p6`, `dm-0`, ...).
pub fn read_counters() -> HashMap<String, DiskCounters> {
    let mut counters = HashMap::new();
    if cfg!(target_os = "linux")
        && let Ok(content) = std::fs::read_to_string("/proc/diskstats")
    {
        parse_counters_into(&content, &mut counters);
    }
    counters
}

/// Field layout per line: `major minor name reads rd_merged rd_sectors rd_ms writes wr_merged
/// wr_sectors wr_ms in_flight io_ticks ...` - newer kernels append discard and flush counters.
pub fn parse_counters_into(content: &str, counters: &mut HashMap<String, DiskCounters>) {
    counters.clear();
    for line in content.lines() {
        let mut fields = [""; 13];
        let mut count = 0;
        for (slot, field) in fields.iter_mut().zip(line.split_ascii_whitespace()) {
            *slot = field;
            count += 1;
        }
        // Pre-4.18 kernels wrote a 7-field short line for partitions, without io_ticks.
        if count < fields.len() {
            continue;
        }
        let parsed = (|| {
            Some(DiskCounters {
                read_sectors: fields[5].parse().ok()?,
                write_sectors: fields[9].parse().ok()?,
                busy_ms: fields[12].parse().ok()?,
            })
        })();
        if let Some(parsed) = parsed {
            counters.insert(fields[2].to_string(), parsed);
        }
    }
}

/// The `/proc/diskstats` name for a device path such as `/dev/nvme0n1p6` or
/// `/dev/mapper/vg-root`, falling back to the mount point's own device number for roots mounted
/// as `/dev/root` - a name the kernel invents that exists neither in `/dev` nor in diskstats.
pub fn stat_name(device_path: &str, mount_point: &str, counters: &HashMap<String, DiskCounters>) -> Option<String> {
    let canonical = std::fs::canonicalize(device_path).ok();
    let path = canonical.as_deref().unwrap_or_else(|| std::path::Path::new(device_path));
    let by_path = path.file_name().map(|name| name.to_string_lossy().into_owned());
    by_path
        .into_iter()
        .chain(mounted_device_name(mount_point))
        .find(|name| counters.contains_key(name))
}

#[cfg(target_os = "linux")]
fn mounted_device_name(mount_point: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let (major, minor) = split_dev(std::fs::metadata(mount_point).ok()?.dev());
    let target = std::fs::read_link(format!("/sys/dev/block/{major}:{minor}")).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
fn mounted_device_name(_mount_point: &str) -> Option<String> {
    None
}

/// glibc's `gnu_dev_major`/`gnu_dev_minor` encoding of `st_dev`.
#[cfg(target_os = "linux")]
fn split_dev(dev: u64) -> (u64, u64) {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    (major, minor)
}

/// Hardware model of the drive backing `device`, from sysfs.  Partitions carry no model of their
/// own, so the parent block device is asked.
#[cfg(target_os = "linux")]
pub fn device_model(device: &str) -> Option<String> {
    let block = std::fs::canonicalize(std::path::Path::new("/sys/class/block").join(short_device_name(device))).ok()?;
    let drive = if block.join("partition").exists() {
        block.parent()?.to_path_buf()
    } else {
        block
    };
    let read = |file: &str| {
        std::fs::read_to_string(drive.join("device").join(file))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let model = read("model")?;
    // SATA drives report the controller protocol as their vendor, which says nothing about the
    // drive; USB enclosures and SCSI disks report a real manufacturer.
    match read("vendor").filter(|v| v != "ATA" && !model.starts_with(v.as_str())) {
        Some(vendor) => Some(format!("{vendor} {model}")),
        None => Some(model),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn device_model(_device: &str) -> Option<String> {
    None
}

/// Why a device's traffic is already counted under another name, or `None` when it should be
/// counted.  `/proc/diskstats` lists several views of the same hardware: partitions next to
/// their disk, device-mapper and RAID targets next to their members, and NVMe multipath aliases.
pub fn duplicate_reason(name: &str, all: &HashMap<String, DiskCounters>) -> Option<String> {
    for (prefix, why) in [
        ("loop", "loopback device"),
        ("ram", "ramdisk"),
        ("zram", "compressed ramdisk"),
        ("dm-", "device-mapper target, counted on the underlying device"),
        ("md", "software RAID, counted on the member devices"),
    ] {
        if name.starts_with(prefix) {
            return Some(why.to_string());
        }
    }
    if let Some(alias) = nvme_multipath_target(name)
        && all.contains_key(&alias)
    {
        return Some(format!("NVMe multipath alias of {alias}"));
    }
    // A partition is its disk's name plus a number, optionally after a "p".
    for parent in all.keys() {
        if parent == name {
            continue;
        }
        let Some(suffix) = name.strip_prefix(parent.as_str()) else { continue };
        let digits = suffix.strip_prefix('p').unwrap_or(suffix);
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return Some(format!("partition of {parent}"));
        }
    }
    None
}

/// `nvme0c1n2` -> `nvme0n2`, or `None` when the name is not a multipath alias.
pub fn nvme_multipath_target(name: &str) -> Option<String> {
    let rest = name.strip_prefix("nvme")?;
    let (controller, rest) = rest.split_once('c')?;
    let (_path, namespace) = rest.split_once('n')?;
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (numeric(controller) && numeric(namespace)).then(|| format!("nvme{controller}n{namespace}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiskSpace {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

impl DiskSpace {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }
}

/// Space of one mount point.  On unix a single `statvfs`, rather than re-listing every mount.
#[cfg(unix)]
pub fn space(mount_point: &str) -> Option<DiskSpace> {
    let path = std::ffi::CString::new(mount_point).ok()?;
    // SAFETY: zeroed statvfs is a valid out-parameter and the path is NUL-terminated.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    if unsafe { libc::statvfs(path.as_ptr(), &raw mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    Some(DiskSpace {
        total_bytes: stat.f_blocks as u64 * block,
        available_bytes: stat.f_bavail as u64 * block,
    })
}

#[cfg(not(unix))]
pub fn space(mount_point: &str) -> Option<DiskSpace> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .find(|d| d.mount_point().to_string_lossy() == mount_point)
        .map(|d| DiskSpace {
            total_bytes: d.total_space(),
            available_bytes: d.available_space(),
        })
}

/// Which of space and I/O a [`DiskSampler`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskMetrics {
    Space,
    Io,
    SpaceAndIo,
}

impl DiskMetrics {
    fn space(self) -> bool {
        matches!(self, Self::Space | Self::SpaceAndIo)
    }

    fn io(self) -> bool {
        matches!(self, Self::Io | Self::SpaceAndIo)
    }
}

/// Space and I/O counters of the tracked disks, aligned with [`DiskSampler::disks`].
#[derive(Clone, Debug)]
pub struct DiskSample {
    pub taken_at: Instant,
    pub space: Vec<Option<DiskSpace>>,
    pub counters: Vec<Option<DiskCounters>>,
}

impl DiskSample {
    pub fn io_since(&self, earlier: &Self) -> Vec<Option<DiskIoRates>> {
        let elapsed = self.taken_at.saturating_duration_since(earlier.taken_at).as_secs_f64();
        self.counters
            .iter()
            .zip(&earlier.counters)
            .map(|(now, before)| DiskIoRates::between((*before)?, (*now)?, elapsed))
            .collect()
    }
}

pub struct DiskSampler {
    disks: Vec<TrackedDisk>,
    metrics: DiskMetrics,
    counters: HashMap<String, DiskCounters>,
    #[cfg(target_os = "linux")]
    content: String,
}

impl DiskSampler {
    pub fn new(disks: Vec<TrackedDisk>, metrics: DiskMetrics) -> Self {
        Self {
            disks,
            metrics,
            counters: HashMap::new(),
            #[cfg(target_os = "linux")]
            content: String::new(),
        }
    }

    pub fn disks(&self) -> &[TrackedDisk] {
        &self.disks
    }

    pub fn sample(&mut self) -> DiskSample {
        let taken_at = Instant::now();
        let space = if self.metrics.space() {
            self.disks.iter().map(|d| space(&d.filesystem.mount_point)).collect()
        } else {
            vec![None; self.disks.len()]
        };
        let counters = if self.metrics.io() && self.disks.iter().any(|d| d.stat_name.is_some()) {
            #[cfg(target_os = "linux")]
            self.read_diskstats();
            self.disks
                .iter()
                .map(|d| d.stat_name.as_ref().and_then(|name| self.counters.get(name).copied()))
                .collect()
        } else {
            vec![None; self.disks.len()]
        };
        DiskSample { taken_at, space, counters }
    }

    #[cfg(target_os = "linux")]
    fn read_diskstats(&mut self) {
        if crate::backend::read_text_into("/proc/diskstats", &mut self.content).is_ok() {
            parse_counters_into(&self.content, &mut self.counters);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs(mount_point: &str, device: &str) -> Filesystem {
        Filesystem {
            mount_point: mount_point.to_string(),
            device: device.to_string(),
            file_system: "ext4".to_string(),
            total_bytes: 100 << 30,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn splits_device_numbers_like_glibc() {
        assert_eq!(split_dev(0x0801), (8, 1));
        assert_eq!(split_dev(0x0001_0301), (259, 1));
        assert_eq!(split_dev(0xb300), (179, 0));
    }

    #[test]
    fn parses_diskstats_lines() {
        let content = "\
 259       6 nvme0n1p6 357178 154991 17962490 73616 427910 944284 31786800 735973 0 156055 809590 0 0 0 0 0 0
 259       7 nvme1n1 456624 89303 30354394 459022 1430359 419612 314360144 306344256 0 2257802 307065394
   8       1 sda1 12 0 34 0
";
        let mut counters = HashMap::new();
        parse_counters_into(content, &mut counters);
        assert_eq!(counters.len(), 2);
        assert_eq!(counters["nvme0n1p6"].read_sectors, 17_962_490);
        assert_eq!(counters["nvme0n1p6"].write_sectors, 31_786_800);
        assert_eq!(counters["nvme0n1p6"].busy_ms, 156_055);
        assert!(!counters.contains_key("sda1"));
    }

    #[test]
    fn derives_rates_and_clamps_busy() {
        let earlier = DiskCounters::default();
        let later = DiskCounters {
            read_sectors: 2048,
            write_sectors: 4096,
            busy_ms: 500,
        };
        let rates = DiskIoRates::between(earlier, later, 2.0).expect("positive window");
        assert!((rates.busy_percent - 25.0).abs() < 1e-9);
        assert!((rates.read_bytes_per_sec - 524_288.0).abs() < 1e-9);
        assert!((rates.write_bytes_per_sec - 1_048_576.0).abs() < 1e-9);
        let over = DiskCounters { busy_ms: 1100, ..earlier };
        assert!((DiskIoRates::between(earlier, over, 1.0).expect("positive window").busy_percent - 100.0).abs() < 1e-9);
    }

    #[test]
    fn keeps_boot_and_recovery_partitions_out_of_all() {
        let all = [
            fs("/", "/dev/nvme0n1p3"),
            fs("/boot", "/dev/nvme0n1p2"),
            fs("/boot/efi", "/dev/nvme0n1p1"),
            fs("/bootcamp", "/dev/sda1"),
            fs("D:\\Recovery", "\\\\?\\Volume{1}"),
        ];
        let kept: Vec<&str> = all
            .iter()
            .filter(|f| auto_skipped_by(&f.mount_point).is_none())
            .map(|f| f.mount_point.as_str())
            .collect();
        assert_eq!(kept, vec!["/", "/bootcamp"]);
    }

    #[test]
    fn keeps_the_shortest_mount_per_device() {
        let all = [fs("/home", "/dev/sda1"), fs("/", "/dev/sda1"), fs("/data", "/dev/sdb1")];
        let unique: Vec<&str> = unique_devices(&all).into_iter().map(|f| f.mount_point.as_str()).collect();
        assert_eq!(unique, vec!["/", "/data"]);
    }

    #[test]
    fn reports_requests_that_match_nothing() {
        let all = [fs("/", "/dev/sda1")];
        let (tracked, notes) = select(&all, &DiskQuery::Requested(vec!["/".to_string(), "/nope".to_string()]));
        assert_eq!(tracked.len(), 1);
        assert_eq!(
            notes,
            vec![DiskNote::NotFound {
                requested: "/nope".to_string()
            }]
        );
    }

    #[test]
    fn honours_exclusions_in_all() {
        let all = [fs("/", "/dev/sda1"), fs("/mnt/backup/old", "/dev/sdc1"), fs("/data", "/dev/sdb1")];
        let (tracked, notes) = select(
            &all,
            &DiskQuery::All {
                excluded: vec!["/mnt/backup".to_string(), "/dev/sdb1".to_string()],
            },
        );
        assert_eq!(tracked.iter().map(|t| t.filesystem.mount_point.as_str()).collect::<Vec<_>>(), vec!["/"]);
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn counts_each_write_once_across_the_views_of_one_device() {
        let all: HashMap<String, DiskCounters> = [
            "nvme0n1",
            "nvme0n1p1",
            "nvme0n1p3",
            "nvme0c0n1",
            "nvme1n1",
            "nvme1c1n1",
            "sda",
            "sda1",
            "dm-0",
            "md0",
            "loop2",
            "zram0",
        ]
        .into_iter()
        .map(|name| (name.to_string(), DiskCounters::default()))
        .collect();
        for counted in ["nvme0n1", "nvme1n1", "sda"] {
            assert!(duplicate_reason(counted, &all).is_none(), "{counted} is real hardware");
        }
        for duplicate in [
            "nvme0n1p1",
            "nvme0n1p3",
            "sda1",
            "dm-0",
            "md0",
            "loop2",
            "zram0",
            "nvme0c0n1",
            "nvme1c1n1",
        ] {
            assert!(duplicate_reason(duplicate, &all).is_some(), "{duplicate} is already counted elsewhere");
        }
    }

    #[test]
    fn resolves_nvme_multipath_aliases() {
        assert_eq!(nvme_multipath_target("nvme0c0n1").as_deref(), Some("nvme0n1"));
        assert_eq!(nvme_multipath_target("nvme3c17n2").as_deref(), Some("nvme3n2"));
        assert_eq!(nvme_multipath_target("nvme0n1"), None);
        assert_eq!(nvme_multipath_target("nvme0n1p2"), None);
        assert_eq!(nvme_multipath_target("sda"), None);
    }

    #[cfg(unix)]
    #[test]
    fn reads_space_of_the_root_filesystem() {
        let root = space("/").expect("statvfs on / succeeds");
        assert!(root.total_bytes > 0);
        assert!(root.available_bytes <= root.total_bytes);
        assert!(space("/definitely/not/a/mount/point").is_none());
    }
}
