use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::{info, warn};

use crate::discovery::{DiscoveredGpu, GALCORE_DEBUGFS, GpuVendor, parse_vivante_meminfo};
use crate::settings::CollectSettings;
use crate::shared_state::{GpuSnapshot, SharedState};

const IDLE_REGISTER_OFFSET: usize = 0x004;
const ALL_MODULES_IDLE: u32 = 0x7fff_ffff;
const FRONT_END_BIT: u32 = 1;
const SAMPLE_PERIOD: Duration = Duration::from_millis(4);
const MAP_LEN: usize = 4096;

/// The front end spins in its WAIT/LINK loop even with no work queued, so only the other
/// modules count; a clock-gated core reads back as 0.
pub fn is_busy(idle: u32) -> bool {
    idle != 0 && idle | FRONT_END_BIT != ALL_MODULES_IDLE
}

struct IdleRegister {
    map: *mut libc::c_void,
    register: *const u32,
}

// SAFETY: the mapping is read-only and owned by exactly one thread at a time.
unsafe impl Send for IdleRegister {}

impl IdleRegister {
    fn open(register_base: u64) -> std::io::Result<Self> {
        let page_base = register_base & !(MAP_LEN as u64 - 1);
        let offset = libc::off_t::try_from(page_base).map_err(std::io::Error::other)?;
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_SYNC).open("/dev/mem")?;
        // SAFETY: fresh shared read-only mapping of one page; validity is checked against MAP_FAILED.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                MAP_LEN,
                libc::PROT_READ,
                libc::MAP_SHARED,
                std::os::fd::AsRawFd::as_raw_fd(&file),
                offset,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        let register_offset = (register_base - page_base) as usize + IDLE_REGISTER_OFFSET;
        // SAFETY: register_offset + 4 <= MAP_LEN, the base comes page-aligned from galcore.
        let register = unsafe { map.cast::<u8>().add(register_offset).cast::<u32>() };
        Ok(Self { map, register })
    }

    fn read(&self) -> u32 {
        // SAFETY: points into the live mapping; volatile because it is a hardware register.
        unsafe { self.register.read_volatile() }
    }
}

impl Drop for IdleRegister {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what open() mapped.
        unsafe {
            libc::munmap(self.map, MAP_LEN);
        }
    }
}

/// Samples in the high half, busy samples in the low half - one atomic swap reads both consistently.
const ONE_SAMPLE: u64 = 1 << 32;

fn sample_loop(register: &IdleRegister, counters: &AtomicU64, shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::Relaxed) {
        let busy = u64::from(is_busy(register.read()));
        counters.fetch_add(ONE_SAMPLE | busy, Ordering::Relaxed);
        std::thread::sleep(SAMPLE_PERIOD);
    }
}

fn utilization_percent(counters: u64) -> u32 {
    let samples = counters >> 32;
    let busy = counters & 0xffff_ffff;
    (busy * 100 + samples / 2).checked_div(samples).unwrap_or(0) as u32
}

fn read_memory() -> (u64, u64) {
    std::fs::read_to_string(format!("{GALCORE_DEBUGFS}/meminfo"))
        .ok()
        .and_then(|s| parse_vivante_meminfo(&s))
        .map_or((0, 0), |m| (m.used_bytes / 1024 / 1024, m.total_bytes / 1024 / 1024))
}

pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, gpu: DiscoveredGpu) {
    let GpuVendor::Vivante { register_base, .. } = gpu.vendor else {
        return;
    };
    let register = match IdleRegister::open(register_base) {
        Ok(register) => register,
        Err(e) => {
            warn!("vivante_gpu_worker: cannot map idle register at {register_base:#x} via /dev/mem ({e}), GPU utilization disabled");
            return;
        }
    };
    info!(
        "vivante_gpu_worker: sampling {} idle register every {SAMPLE_PERIOD:?}",
        gpu.display_name()
    );

    let counters = Arc::new(AtomicU64::new(0));
    let sampler = {
        let counters = Arc::clone(&counters);
        let shutdown = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("vivante-sampler".to_string())
            .spawn(move || sample_loop(&register, &counters, &shutdown))
    };
    if let Err(e) = &sampler {
        warn!("vivante_gpu_worker: cannot start sampler thread ({e})");
    }

    let interval_ms = (settings.gpu_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    interval.tick().await;
    counters.store(0, Ordering::Relaxed);

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let (memory_used_mb, memory_total_mb) = read_memory();
        let snapshot = GpuSnapshot {
            utilization_gpu: utilization_percent(counters.swap(0, Ordering::Relaxed)),
            memory_used_mb,
            memory_total_mb,
            temperature: 0,
        };
        let mut guard = state.write().expect("SharedState RwLock poisoned");
        if let Some(slot) = guard.latest_gpus.get_mut(gpu.gpu_index) {
            *slot = Some(snapshot);
        }
    }

    if let Ok(sampler) = sampler {
        let _ = sampler.join();
    }
    info!("vivante_gpu_worker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_end_alone_is_idle() {
        assert!(!is_busy(ALL_MODULES_IDLE));
        assert!(!is_busy(ALL_MODULES_IDLE & !FRONT_END_BIT));
    }

    #[test]
    fn gated_core_is_idle() {
        assert!(!is_busy(0));
    }

    #[test]
    fn any_other_module_is_busy() {
        assert!(is_busy(0x7fff_ffba));
        assert!(is_busy(ALL_MODULES_IDLE & !(1 << 2)));
    }

    #[test]
    fn utilization_rounds_and_handles_no_samples() {
        assert_eq!(utilization_percent(0), 0);
        assert_eq!(utilization_percent((500 * ONE_SAMPLE) | 15), 3);
        assert_eq!(utilization_percent((3 * ONE_SAMPLE) | 3), 100);
    }
}
