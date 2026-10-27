//! Vivante GPUs behind NXP's `galcore` driver (i.MX).  The driver exposes no load figure, so
//! the 3D core's idle register is sampled every few milliseconds on a thread of its own.

use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::{Gpu, GpuKind, GpuReading};

pub const GALCORE_DEBUGFS: &str = "/sys/kernel/debug/gc";
const GALCORE_REGISTER_BASES: &str = "/sys/module/galcore/parameters/registerBases";
const GALCORE_REGISTER_MEM_BASE: &str = "/sys/module/galcore/parameters/registerMemBase";

const IDLE_REGISTER_OFFSET: usize = 0x004;
const ALL_MODULES_IDLE: u32 = 0x7fff_ffff;
const FRONT_END_BIT: u32 = 1;
const SAMPLE_PERIOD: Duration = Duration::from_millis(4);
const MAP_LEN: usize = 4096;
/// Samples in the high half, busy samples in the low half - one atomic swap reads both consistently.
const ONE_SAMPLE: u64 = 1 << 32;

/// Only the 3D core is tracked: on i.MX6 the 2D (GC320) and VG (GC355) cores sit idle under Qt Quick.
pub fn discover(gpus: &mut Vec<Gpu>) {
    let Ok(info) = std::fs::read_to_string(format!("{GALCORE_DEBUGFS}/info")) else {
        return;
    };
    let Some(model) = parse_model(&info) else {
        log::warn!("galcore: no 3D core in {GALCORE_DEBUGFS}/info, skipping");
        return;
    };
    let register_base = std::fs::read_to_string(GALCORE_REGISTER_BASES)
        .ok()
        .and_then(|s| parse_core_register_base(&s))
        .or_else(|| {
            std::fs::read_to_string(GALCORE_REGISTER_MEM_BASE)
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
        });
    let Some(register_base) = register_base else {
        log::warn!("galcore: cannot read {GALCORE_REGISTER_BASES}, skipping");
        return;
    };
    gpus.push(Gpu {
        name: format!("Vivante GC{model}"),
        kind: GpuKind::Vivante { register_base },
        vram_total_bytes: read_meminfo().map(|m| m.total_bytes),
        driver_version: std::fs::read_to_string(format!("{GALCORE_DEBUGFS}/version"))
            .ok()
            .and_then(|s| parse_version(&s)),
    });
}

/// The front end spins in its WAIT/LINK loop even with no work queued, so only the other modules
/// count; a clock-gated core reads back as 0.
pub fn is_busy(idle: u32) -> bool {
    idle != 0 && idle | FRONT_END_BIT != ALL_MODULES_IDLE
}

fn utilization_percent(counters: u64) -> Option<u32> {
    let samples = counters >> 32;
    let busy = counters & 0xffff_ffff;
    (busy * 100 + samples / 2).checked_div(samples).map(|p| p as u32)
}

/// `registerBases` lists one base per core, 3D first, e.g. `939524096,944766976,0,...`.  The
/// scalar `registerMemBase` is a module default (0x80000000 = RAM) that platform probing never updates.
pub fn parse_core_register_base(register_bases: &str) -> Option<u64> {
    register_bases.trim().split(',').next()?.trim().parse().ok().filter(|&base| base != 0)
}

/// First line of `gc/version` is `6.4.3.p2.336687 built at <hash>`.
pub fn parse_version(version: &str) -> Option<String> {
    Some(format!("galcore {}", version.split_whitespace().next()?))
}

/// Model of the `gpu : 0` block (the 3D core), e.g. `2000` for GC2000.
pub fn parse_model(info: &str) -> Option<String> {
    let mut in_core_0 = false;
    for line in info.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        match key.trim() {
            "gpu" => in_core_0 = value.trim() == "0",
            "model" if in_core_0 => return Some(value.trim().to_string()),
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemInfo {
    /// Sum of `Used` over every pool (contiguous SYSTEM + MMU-mapped VIRTUAL).
    pub used_bytes: u64,
    /// `Total` of the contiguous SYSTEM pool, the only pool with a fixed size.
    pub total_bytes: u64,
}

pub fn parse_meminfo(meminfo: &str) -> Option<MemInfo> {
    let mut used_bytes = 0;
    let mut total_bytes = 0;
    let mut found = false;
    for line in meminfo.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let Some(bytes) = value.trim().strip_suffix('B').and_then(|v| v.trim().parse::<u64>().ok()) else {
            continue;
        };
        match key.trim() {
            "Used" => {
                used_bytes += bytes;
                found = true;
            }
            "Total" => total_bytes = total_bytes.max(bytes),
            _ => {}
        }
    }
    found.then_some(MemInfo { used_bytes, total_bytes })
}

fn read_meminfo() -> Option<MemInfo> {
    parse_meminfo(&std::fs::read_to_string(format!("{GALCORE_DEBUGFS}/meminfo")).ok()?)
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
        let offset = libc::off64_t::try_from(page_base).map_err(std::io::Error::other)?;
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_SYNC).open("/dev/mem")?;
        // SAFETY: fresh shared read-only mapping of one page; validity is checked against MAP_FAILED.
        let map = unsafe {
            libc::mmap64(
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

/// Owns the sampling thread; dropping it stops the thread.
pub struct VivanteReader {
    counters: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl VivanteReader {
    pub fn start(register_base: u64) -> std::io::Result<Self> {
        let register = IdleRegister::open(register_base)?;
        let counters = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let counters = Arc::clone(&counters);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new().name("vivante-sampler".to_string()).spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    counters.fetch_add(ONE_SAMPLE | u64::from(is_busy(register.read())), Ordering::Relaxed);
                    std::thread::sleep(SAMPLE_PERIOD);
                }
            })?
        };
        Ok(Self {
            counters,
            stop,
            thread: Some(thread),
        })
    }

    /// Utilization since the previous read.
    pub fn read(&mut self) -> GpuReading {
        let memory = read_meminfo();
        GpuReading {
            utilization_percent: utilization_percent(self.counters.swap(0, Ordering::Relaxed)),
            memory_used_bytes: memory.map(|m| m.used_bytes),
            memory_total_bytes: memory.map(|m| m.total_bytes),
            temperature_celsius: None,
        }
    }
}

impl Drop for VivanteReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "gpu      : 0\nmodel    : 2000\nrevision : ffff5450\n\ngpu      : 8\nmodel    :  320\n\ngpu      : 9\nmodel    :  355\n";
    const MEMINFO: &str = "VIDEO MEMORY:\n  POOL SYSTEM:\n    Free :     112002796 B\n    Used :      22214932 B\n    Total :    134217728 B\n  POOL VIRTUAL:\n    Used :      21549056 B\n";

    #[test]
    fn front_end_alone_and_gated_core_are_idle() {
        assert!(!is_busy(ALL_MODULES_IDLE));
        assert!(!is_busy(ALL_MODULES_IDLE & !FRONT_END_BIT));
        assert!(!is_busy(0));
        assert!(is_busy(0x7fff_ffba));
    }

    #[test]
    fn utilization_rounds_and_needs_samples() {
        assert_eq!(utilization_percent(0), None);
        assert_eq!(utilization_percent((500 * ONE_SAMPLE) | 15), Some(3));
        assert_eq!(utilization_percent((3 * ONE_SAMPLE) | 3), Some(100));
    }

    #[test]
    fn parses_galcore_files() {
        assert_eq!(parse_model(INFO).as_deref(), Some("2000"));
        assert_eq!(parse_model("gpu      : 8\nmodel    :  320\n"), None);
        assert_eq!(parse_core_register_base("939524096,944766976,0,0\n"), Some(0x3800_0000));
        assert_eq!(parse_core_register_base("0,0,0"), None);
        assert_eq!(
            parse_version("6.4.3.p2.336687 built at 7f42315f44fc\n").as_deref(),
            Some("galcore 6.4.3.p2.336687")
        );
        assert_eq!(
            parse_meminfo(MEMINFO),
            Some(MemInfo {
                used_bytes: 22_214_932 + 21_549_056,
                total_bytes: 134_217_728,
            })
        );
    }
}
