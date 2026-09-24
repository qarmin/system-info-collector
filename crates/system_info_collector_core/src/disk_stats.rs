//! Cumulative per-device I/O counters, the same source `iostat` reads.
//!
//! Only Linux exposes them (`/proc/diskstats`); elsewhere the maps come back
//! empty and the disk I/O metrics report -1.

use std::collections::HashMap;

use crate::shared_state::DiskIoSnapshot;

const SECTOR_BYTES: f64 = 512.0;

/// Cumulative counters for one block device.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskCounters {
    pub read_sectors: u64,
    pub write_sectors: u64,
    /// Wall-clock milliseconds during which at least one request was in flight
    /// (`io_ticks`) - the basis of iostat's %util.
    pub busy_ms: u64,
}

fn sectors_to_mb(sectors: u64) -> f64 {
    sectors as f64 * SECTOR_BYTES / 1_048_576.0
}

/// Busy% and throughput of one device over `elapsed` seconds, exactly how
/// `iostat` derives them from two reads of the cumulative counters.
/// `None` when the device is missing from either sample.
pub fn rates(
    stat_name: &str,
    current: &HashMap<String, DiskCounters>,
    previous: &HashMap<String, DiskCounters>,
    elapsed: f64,
) -> Option<DiskIoSnapshot> {
    let current = current.get(stat_name)?;
    let previous = previous.get(stat_name)?;

    // io_ticks is wall time with I/O in flight, so it can drift a millisecond or
    // two past the sampling window - clamped instead of reported above 100%.
    let busy_pct = (current.busy_ms.saturating_sub(previous.busy_ms) as f64 / (elapsed * 1000.0) * 100.0).clamp(0.0, 100.0);

    Some(DiskIoSnapshot {
        busy_pct,
        read_mb_per_sec: sectors_to_mb(current.read_sectors.saturating_sub(previous.read_sectors)) / elapsed,
        write_mb_per_sec: sectors_to_mb(current.write_sectors.saturating_sub(previous.write_sectors)) / elapsed,
    })
}

/// Counters for every block device, keyed by kernel device name (`nvme0n1p6`, `dm-0`, …).
pub fn read_counters() -> HashMap<String, DiskCounters> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/diskstats")
            .map(|content| parse_counters(&content))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        HashMap::new()
    }
}

/// The kernel device name to look up in [`read_counters`] for a device path
/// such as `/dev/nvme0n1p6` or `/dev/mapper/vg-root`, or `None` when the device
/// has no I/O counters (missing on this platform, network filesystem, …).
pub fn device_stat_name(device_path: &str) -> Option<String> {
    let canonical = std::fs::canonicalize(device_path).ok();
    let path = canonical.as_deref().unwrap_or_else(|| std::path::Path::new(device_path));
    let name = path.file_name()?.to_string_lossy().into_owned();
    read_counters().contains_key(&name).then_some(name)
}

/// Field layout per line: `major minor name reads rd_merged rd_sectors rd_ms writes
/// wr_merged wr_sectors wr_ms in_flight io_ticks …` - newer kernels append discard
/// and flush counters, which are ignored.
fn parse_counters(content: &str) -> HashMap<String, DiskCounters> {
    let mut counters = HashMap::new();
    for line in content.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // Pre-4.18 kernels wrote a 7-field short line for partitions, without io_ticks.
        if fields.len() < 13 {
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
    counters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diskstats_lines() {
        let content = "\
 259       6 nvme0n1p6 357178 154991 17962490 73616 427910 944284 31786800 735973 0 156055 809590 0 0 0 0 0 0
 259       7 nvme1n1 456624 89303 30354394 459022 1430359 419612 314360144 306344256 0 2257802 307065394
   8       1 sda1 12 0 34 0
";
        let counters = parse_counters(content);

        assert_eq!(counters.len(), 2);
        let partition = counters["nvme0n1p6"];
        assert_eq!(partition.read_sectors, 17_962_490);
        assert_eq!(partition.write_sectors, 31_786_800);
        assert_eq!(partition.busy_ms, 156_055);
        assert_eq!(counters["nvme1n1"].busy_ms, 2_257_802);
        // Short pre-4.18 partition line carries no io_ticks, so it is not usable.
        assert!(!counters.contains_key("sda1"));
    }

    #[test]
    fn converts_sectors_to_mb() {
        assert!((sectors_to_mb(2048) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn derives_rates_from_two_samples() {
        let previous = HashMap::from([("sda".to_string(), DiskCounters::default())]);
        let current = HashMap::from([(
            "sda".to_string(),
            DiskCounters {
                read_sectors: 2048,
                write_sectors: 4096,
                busy_ms: 500,
            },
        )]);

        let snapshot = rates("sda", &current, &previous, 2.0).expect("device present in both samples");
        assert!((snapshot.busy_pct - 25.0).abs() < 0.001);
        assert!((snapshot.read_mb_per_sec - 0.5).abs() < 0.001);
        assert!((snapshot.write_mb_per_sec - 1.0).abs() < 0.001);

        assert!(rates("nvme0n1", &current, &previous, 2.0).is_none());
    }

    #[test]
    fn clamps_busy_over_sampling_window() {
        let previous = HashMap::from([("sda".to_string(), DiskCounters::default())]);
        let current = HashMap::from([(
            "sda".to_string(),
            DiskCounters {
                busy_ms: 1100,
                ..DiskCounters::default()
            },
        )]);

        let snapshot = rates("sda", &current, &previous, 1.0).expect("device present in both samples");
        assert!((snapshot.busy_pct - 100.0).abs() < f64::EPSILON);
    }
}
