use std::path::{Path, PathBuf};
use std::time::Instant;

use super::GpuReading;

/// Utilization from RC6 residency, the time the GPU spent power-saving:
/// `busy = 1 - rc6_delta / elapsed`.
pub struct IntelReader {
    rc6_path: PathBuf,
    previous: Option<(u64, Instant)>,
}

impl IntelReader {
    pub fn new(device_path: &Path) -> Self {
        Self {
            rc6_path: device_path.join("gt/gt0/rc6_residency_ms"),
            previous: None,
        }
    }

    pub fn read(&mut self, now: Instant) -> GpuReading {
        let rc6_ms = std::fs::read_to_string(&self.rc6_path).ok().and_then(|s| s.trim().parse::<u64>().ok());
        let utilization_percent = match (rc6_ms, self.previous) {
            (Some(rc6), Some((previous_rc6, previous_at))) => {
                busy_percent(rc6.saturating_sub(previous_rc6), now.duration_since(previous_at).as_millis() as u64)
            }
            _ => None,
        };
        self.previous = rc6_ms.map(|rc6| (rc6, now));
        GpuReading {
            utilization_percent,
            ..GpuReading::default()
        }
    }
}

fn busy_percent(rc6_delta_ms: u64, elapsed_ms: u64) -> Option<u32> {
    (elapsed_ms > 0).then(|| ((1.0 - rc6_delta_ms as f64 / elapsed_ms as f64) * 100.0).clamp(0.0, 100.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_busy_share_from_rc6_residency() {
        assert_eq!(busy_percent(750, 1000), Some(25));
        assert_eq!(busy_percent(1010, 1000), Some(0));
        assert_eq!(busy_percent(0, 0), None);
    }
}
