//! Bounded recording of *every* process on the machine, for diagnosing a
//! problem happening right now.
//!
//! The CSV collector answers "how did the machine behave over hours" with a
//! fixed set of columns.  This answers a different question - "which process is
//! doing that, right now" - so it captures the whole process set a few times a
//! second for a bounded period, tracks appearances and deaths, and records
//! enough per-process I/O detail to attribute writes.
//!
//! Everything is held in memory and serialized once at the end; a crash mid
//! session loses the recording, which is an accepted trade for not writing
//! continuously.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Error;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, ThreadKind, UpdateKind};

use crate::disk_stats::{self, DiskCounters};

pub const SESSION_FORMAT_VERSION: u32 = 1;

/// Upper bound on the sampling rate, and the reason this module does not simply
/// sample as fast as it can.
///
/// `sysinfo` gates re-reading `/proc/stat` behind `MINIMUM_CPU_UPDATE_INTERVAL`
/// (200 ms) while still refreshing each process's own `utime`/`stime` on every
/// call.  Sampling faster leaves the per-process numerator covering a short
/// window and the global denominator covering an older, longer one, so CPU% comes
/// out silently deflated instead of failing loudly.  5 Hz is that 200 ms floor
/// expressed as a rate.
pub const MAX_SESSION_HZ: f32 = 5.0;

/// Below this the recording is too coarse to attribute anything.
pub const MIN_SESSION_HZ: f32 = 0.2;

/// Caps ticks so a long session cannot exhaust memory, in the spirit of
/// `MAX_BUFFER_SAMPLES` for the live buffer.
pub const MAX_SESSION_TICKS: usize = 36_000;

/// Only processes that actually wrote get their open files listed, and no more
/// often than this - the paths barely change and `readlink` on every descriptor
/// is the most expensive thing here.
const FD_RESCAN_SECS: f64 = 1.0;
const FD_MAX_PROCESSES_PER_TICK: usize = 10;
const FD_MAX_PATHS_PER_PROCESS: usize = 64;

/// A sample is kept when the process did something; otherwise the tick is
/// omitted entirely and the viewer treats the gap as "idle, unchanged".
const ACTIVE_CPU_PCT: f32 = 0.05;
const ACTIVE_RSS_DELTA_MB: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    pub duration_secs: f64,
    pub hz: f32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            duration_secs: 60.0,
            hz: MAX_SESSION_HZ,
        }
    }
}

impl SessionConfig {
    /// Reject rather than silently clamp the sampling rate: a session recorded
    /// above [`MAX_SESSION_HZ`] would contain wrong CPU numbers that look
    /// perfectly plausible, so it is better to refuse it.
    pub fn validate(&self) -> Result<(), Error> {
        if !self.hz.is_finite() || self.hz < MIN_SESSION_HZ || self.hz > MAX_SESSION_HZ {
            return Err(Error::msg(format!(
                "Sampling rate must be between {MIN_SESSION_HZ} and {MAX_SESSION_HZ} Hz (sysinfo cannot report correct \
                 per-process CPU% faster than {MAX_SESSION_HZ} Hz)"
            )));
        }
        if !self.duration_secs.is_finite() || self.duration_secs < 1.0 {
            return Err(Error::msg("Duration must be at least 1 second"));
        }
        if self.tick_count() > MAX_SESSION_TICKS {
            return Err(Error::msg(format!(
                "{}s at {} Hz needs {} samples, over the {MAX_SESSION_TICKS} limit - lower --hz or shorten the session",
                self.duration_secs,
                self.hz,
                self.tick_count()
            )));
        }
        Ok(())
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.hz))
    }

    pub fn tick_count(&self) -> usize {
        ((self.duration_secs * f64::from(self.hz)).round() as usize).max(1)
    }
}

/// Live counters a caller can poll while a recording runs, so a UI can show a
/// countdown without the recorder knowing anything about HTTP.
#[derive(Default)]
pub struct SessionProgress {
    pub ticks_done: AtomicUsize,
    pub processes_seen: AtomicUsize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMeta {
    pub format_version: u32,
    pub app_version: String,
    pub hz: f32,
    /// Ticks actually recorded, which is lower than requested when stopped early.
    pub ticks: usize,
    pub requested_duration_secs: f64,
    pub recorded_duration_secs: f64,
    /// Wall-clock start, so the viewer can show absolute times.
    pub start_unix: f64,
    pub aborted: bool,
    pub process_count: usize,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub total_memory_mb: f64,
    /// False on platforms without `/proc`, where `wchar`/`rchar`, open files and
    /// kernel threads are unavailable and the viewer must say so.
    pub proc_fs: bool,
    /// The recorder's own PID.  It reads several `/proc` files per process per
    /// tick, which lands on its own `rchar` as hundreds of megabytes on a busy
    /// machine - the viewer flags that row so the observer is not mistaken for a
    /// culprit.
    pub recorder_pid: u32,
    /// What the whole machine moved across the block device during the recording,
    /// from `/proc/diskstats`.
    pub device_read_bytes: u64,
    pub device_written_bytes: u64,
    /// The same period summed over every process.  `device_*` above minus
    /// `proc_*_bytes` here is traffic no process claimed - swap, filesystem
    /// metadata, or a process too short-lived to sample.
    pub proc_rchar_bytes: u64,
    pub proc_wchar_bytes: u64,
    pub proc_read_bytes: u64,
    pub proc_written_bytes: u64,
}

/// Cumulative `/proc/<pid>/io` counters: bytes since the process started.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct IoTotals {
    pub rchar: u64,
    pub wchar: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

/// One process as it existed during the session.  Static fields only - anything
/// that changes per tick lives in [`SessionSamples`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionProcess {
    pub id: usize,
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub exe: String,
    pub cmd: String,
    /// Kernel start time in unix seconds.  Compared against `meta.start_unix`
    /// this distinguishes "was already running" from "spawned during the session",
    /// which is the difference between background noise and a suspect.
    pub start_unix: u64,
    /// Kernel threads carry no exe or cmdline, but they are where buffered
    /// writeback is charged, so they are kept and flagged instead of skipped.
    pub kernel: bool,
    pub first_tick: usize,
    pub last_tick: usize,
    /// False when the process was gone before the recording ended.
    pub alive_at_end: bool,
    /// Lifetime counters when the process was last sampled - the same totals a
    /// task manager shows in its "disk read/write total" columns.  A process that
    /// is quiet during the recording but has written gigabytes since boot is
    /// still worth noticing, and only this shows it.
    pub io_total: IoTotals,
    /// The same counters when it was first sampled, so the recording's own share
    /// is `io_total` minus this.
    pub io_at_start: IoTotals,
}

/// Per-tick values for one process, aligned to `t` (tick indices, ascending).
///
/// Ticks where the process did nothing are absent rather than zero-filled, which
/// is what keeps a 400-process recording in the low megabytes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionSamples {
    pub t: Vec<u32>,
    pub cpu: Vec<f32>,
    pub rss_mb: Vec<f32>,
    /// Bytes this process read/wrote through syscalls during the tick.  Charged
    /// to the process that actually called `write()`, including when the data
    /// only reaches the page cache.
    pub rchar: Vec<u64>,
    pub wchar: Vec<u64>,
    /// Bytes that actually reached the block device during the tick.  Charged to
    /// whoever submitted the I/O, which for buffered writes is a kernel flush
    /// thread rather than the process that produced the data - comparing this
    /// against `wchar` is what identifies the real writer.
    pub read_bytes: Vec<u64>,
    pub written_bytes: Vec<u64>,
    /// Bytes the kernel folded in from children reaped during this tick, already
    /// subtracted from the figures above.
    ///
    /// This is how a process that spawns short-lived writers gives itself away:
    /// the children may be too brief to sample, but their totals still surface
    /// here against the parent.
    pub inherited_wchar: Vec<u64>,
    pub inherited_written: Vec<u64>,
    /// One character per entry in `t`: R run, S sleep, D uninterruptible I/O
    /// wait, Z zombie, T stopped, I idle, `?` unknown.
    pub status: String,
    /// Only sampled for active ticks, so 0 means "not measured", not "no threads".
    pub threads: Vec<u32>,
    pub vol_ctx: Vec<u64>,
    pub nonvol_ctx: Vec<u64>,
}

impl SessionSamples {
    fn push(&mut self, tick: usize, s: &TickSample) {
        self.t.push(tick as u32);
        self.cpu.push(s.cpu);
        self.rss_mb.push(s.rss_mb);
        self.rchar.push(s.io.rchar);
        self.wchar.push(s.io.wchar);
        self.read_bytes.push(s.io.read_bytes);
        self.written_bytes.push(s.io.written_bytes);
        self.inherited_wchar.push(s.inherited.wchar);
        self.inherited_written.push(s.inherited.written_bytes);
        self.status.push(s.status);
        self.threads.push(s.threads);
        self.vol_ctx.push(s.vol_ctx);
        self.nonvol_ctx.push(s.nonvol_ctx);
    }
}

/// Machine-wide series, one entry per tick.  This is what the viewer draws at
/// the top and what you select a range on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionSystem {
    /// Milliseconds since `meta.start_unix`, per tick.
    pub t_ms: Vec<u32>,
    pub cpu_pct: Vec<f32>,
    pub mem_used_mb: Vec<f32>,
    /// Whole-device throughput from `/proc/diskstats` - the ground truth for
    /// "the disk is being written to".
    pub disk_read_mbs: Vec<f32>,
    pub disk_write_mbs: Vec<f32>,
    /// Per-process sums for the same tick.  `wchar` above the device series means
    /// writes are being absorbed by the page cache; the device series lagging
    /// behind in bursts is writeback catching up.
    pub proc_wchar_mbs: Vec<f32>,
    pub proc_written_mbs: Vec<f32>,
}

/// Open files of a process that wrote during `tick`, as indices into
/// [`SessionData::paths`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FdSnapshot {
    pub tick: u32,
    pub paths: Vec<u32>,
}

/// `meta` is deliberately first and small: listing saved sessions parses only
/// that prefix instead of loading every file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionData {
    pub meta: SessionMeta,
    pub system: SessionSystem,
    pub procs: Vec<SessionProcess>,
    /// Keyed by [`SessionProcess::id`]; processes that never did anything are
    /// absent entirely.
    pub samples: BTreeMap<usize, SessionSamples>,
    /// Interned file paths referenced by `fds`.
    pub paths: Vec<String>,
    pub fds: BTreeMap<usize, Vec<FdSnapshot>>,
}

// ── recording ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default)]
struct IoCounters {
    rchar: u64,
    wchar: u64,
    read_bytes: u64,
    written_bytes: u64,
}

impl From<IoCounters> for IoTotals {
    fn from(counters: IoCounters) -> Self {
        Self {
            rchar: counters.rchar,
            wchar: counters.wchar,
            read_bytes: counters.read_bytes,
            written_bytes: counters.written_bytes,
        }
    }
}

/// Everything the recording moved, accumulated as it goes.
#[derive(Default)]
struct SessionTotals {
    device_read_bytes: u64,
    device_written_bytes: u64,
    proc_rchar_bytes: u64,
    proc_wchar_bytes: u64,
    proc_read_bytes: u64,
    proc_written_bytes: u64,
}

impl IoCounters {
    fn delta(self, previous: Self) -> Self {
        Self {
            rchar: self.rchar.saturating_sub(previous.rchar),
            wchar: self.wchar.saturating_sub(previous.wchar),
            read_bytes: self.read_bytes.saturating_sub(previous.read_bytes),
            written_bytes: self.written_bytes.saturating_sub(previous.written_bytes),
        }
    }

    fn is_zero(self) -> bool {
        self.rchar == 0 && self.wchar == 0 && self.read_bytes == 0 && self.written_bytes == 0
    }

    fn saturating_minus(self, other: Self) -> Self {
        Self {
            rchar: self.rchar.saturating_sub(other.rchar),
            wchar: self.wchar.saturating_sub(other.wchar),
            read_bytes: self.read_bytes.saturating_sub(other.read_bytes),
            written_bytes: self.written_bytes.saturating_sub(other.written_bytes),
        }
    }

    /// How much of `other` this delta can actually account for.  A child's total
    /// is only credited as inherited up to the jump the parent really showed, so
    /// a fold that lands on a later tick is not double-counted.
    fn overlap(self, other: Self) -> Self {
        Self {
            rchar: self.rchar.min(other.rchar),
            wchar: self.wchar.min(other.wchar),
            read_bytes: self.read_bytes.min(other.read_bytes),
            written_bytes: self.written_bytes.min(other.written_bytes),
        }
    }
}

/// One process as seen in a single tick, before deltas are computed.
struct Observation {
    key: ProcKey,
    pid: u32,
    ppid: Option<u32>,
    kernel: bool,
    cpu: f32,
    rss_mb: f32,
    status: char,
    raw_io: IoCounters,
    name: String,
    exe: String,
    cmd: String,
}

struct TickSample {
    cpu: f32,
    rss_mb: f32,
    io: IoCounters,
    inherited: IoCounters,
    status: char,
    threads: u32,
    vol_ctx: u64,
    nonvol_ctx: u64,
}

/// What the recorder remembers about a process between ticks.
struct Tracked {
    id: usize,
    prev_io: IoCounters,
    /// The first tick has nothing to subtract from, so its deltas are dropped
    /// rather than reported as the process's whole lifetime total.
    has_prev_io: bool,
    last_rss_mb: f32,
    last_fd_scan: Option<Instant>,
}

/// Identity that survives PID reuse: the kernel start time changes even when a
/// PID is handed out again.
type ProcKey = (u32, u64);

pub fn record(config: SessionConfig, stop: &AtomicBool, progress: &SessionProgress, app_version: &str) -> Result<SessionData, Error> {
    config.validate()?;

    let interval = config.interval();
    let total_ticks = config.tick_count();
    info!(
        "Recording session: {}s at {} Hz ({total_ticks} samples, every {:.0} ms)",
        config.duration_secs,
        config.hz,
        interval.as_secs_f64() * 1000.0
    );

    let refresh = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet);
    // sysinfo reads only read_bytes/write_bytes from /proc/<pid>/io; where the
    // whole file is available this module parses all four fields itself, so
    // asking sysinfo for disk usage as well would read the same file twice.
    let refresh = if cfg!(target_os = "linux") { refresh } else { refresh.with_disk_usage() };

    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);

    let cpu_cores = sys.cpus().len().max(1);
    let meta_cpu_model = sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default();
    let total_memory_mb = sys.total_memory() as f64 / 1_048_576.0;

    let mut tracked: HashMap<ProcKey, Tracked> = HashMap::new();
    let mut procs: Vec<SessionProcess> = Vec::new();
    let mut samples: BTreeMap<usize, SessionSamples> = BTreeMap::new();
    let mut fds: BTreeMap<usize, Vec<FdSnapshot>> = BTreeMap::new();
    let mut path_ids: HashMap<String, u32> = HashMap::new();
    let mut paths: Vec<String> = Vec::new();
    let mut system = SessionSystem::default();

    let mut totals = SessionTotals::default();
    let mut disk_prev = disk_stats::read_counters();
    let start_instant = Instant::now();
    let start_unix = unix_now();
    let mut aborted = false;
    // The first tick has no previous counters to diff against, so it only
    // establishes a baseline and is not recorded.
    let mut previous_tick_at = start_instant;

    for tick in 0..total_ticks {
        let deadline = start_instant + interval.mul_f64((tick + 1) as f64);
        if !sleep_until(deadline, stop) {
            aborted = true;
            break;
        }

        let refreshed_at = Instant::now();
        let elapsed_secs = refreshed_at.duration_since(previous_tick_at).as_secs_f64().max(1e-6);
        previous_tick_at = refreshed_at;

        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);

        let disk_current = disk_stats::read_counters();
        let (device_read, device_written) = whole_device_bytes(&disk_current, &disk_prev);
        disk_prev = disk_current;
        totals.device_read_bytes += device_read;
        totals.device_written_bytes += device_written;

        // Pass one only gathers, because the deltas computed in pass two need to
        // know which processes disappeared during this tick - see `inherited`.
        let mut observed: Vec<Observation> = Vec::with_capacity(procs.len().max(64));
        let mut alive: HashSet<ProcKey> = HashSet::with_capacity(procs.len());
        for process in sys.processes().values() {
            // Userland threads duplicate their parent's RSS and would inflate
            // every total; kernel threads are distinct tasks and are kept.
            let kernel = match process.thread_kind() {
                Some(ThreadKind::Userland) => continue,
                Some(ThreadKind::Kernel) => true,
                None => false,
            };

            let pid = process.pid().as_u32();
            let key = (pid, process.start_time());
            alive.insert(key);
            observed.push(Observation {
                key,
                pid,
                ppid: process.parent().map(|p| p.as_u32()),
                kernel,
                cpu: process.cpu_usage() / cpu_cores as f32,
                rss_mb: process.memory() as f32 / 1_048_576.0,
                status: status_char(process.status()),
                raw_io: read_io_counters(pid).unwrap_or_else(|| {
                    let usage = process.disk_usage();
                    IoCounters {
                        rchar: 0,
                        wchar: 0,
                        read_bytes: usage.total_read_bytes,
                        written_bytes: usage.total_written_bytes,
                    }
                }),
                name: process.name().to_string_lossy().into_owned(),
                exe: process.exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                cmd: process
                    .cmd()
                    .iter()
                    .map(|part| part.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" "),
            });
        }

        // Linux folds a reaped child's I/O accounting into its parent
        // (`wait_task_zombie` -> `task_io_accounting_add`), so the parent's
        // counters jump by the child's lifetime total the moment it is waited on.
        // Left alone that blames every shell and service manager for whatever its
        // children wrote, so the jump is measured here, removed from the parent's
        // own figures, and reported separately.
        let mut inherited: HashMap<u32, IoCounters> = HashMap::new();
        for (key, entry) in &tracked {
            if alive.contains(key) {
                continue;
            }
            let Some(parent_pid) = procs[entry.id].ppid else { continue };
            let child_total = entry.prev_io;
            let folded = inherited.entry(parent_pid).or_default();
            folded.rchar += child_total.rchar;
            folded.wchar += child_total.wchar;
            folded.read_bytes += child_total.read_bytes;
            folded.written_bytes += child_total.written_bytes;
        }

        let mut writers: Vec<(usize, u64, u32)> = Vec::new();
        let mut proc_wchar_total: u64 = 0;
        let mut proc_written_total: u64 = 0;

        for seen in observed {
            let entry = tracked.entry(seen.key).or_insert_with(|| {
                let id = procs.len();
                procs.push(SessionProcess {
                    id,
                    pid: seen.pid,
                    ppid: seen.ppid,
                    name: seen.name.clone(),
                    exe: seen.exe.clone(),
                    cmd: seen.cmd.clone(),
                    start_unix: seen.key.1,
                    kernel: seen.kernel,
                    first_tick: tick,
                    last_tick: tick,
                    alive_at_end: false,
                    io_total: seen.raw_io.into(),
                    io_at_start: seen.raw_io.into(),
                });
                Tracked {
                    id,
                    prev_io: seen.raw_io,
                    has_prev_io: false,
                    last_rss_mb: f32::MIN,
                    last_fd_scan: None,
                }
            });

            let id = entry.id;
            procs[id].last_tick = tick;
            procs[id].io_total = seen.raw_io.into();

            let raw_delta = if entry.has_prev_io {
                seen.raw_io.delta(entry.prev_io)
            } else {
                IoCounters::default()
            };
            entry.prev_io = seen.raw_io;
            entry.has_prev_io = true;

            // Never let the correction go negative: the fold can land a tick after
            // the child vanished, and the parent may also have written itself.
            let folded = inherited.get(&seen.pid).copied().unwrap_or_default();
            let io = raw_delta.saturating_minus(folded);
            let inherited_io = raw_delta.overlap(folded);

            proc_wchar_total += io.wchar;
            proc_written_total += io.written_bytes;
            totals.proc_rchar_bytes += io.rchar;
            totals.proc_wchar_bytes += io.wchar;
            totals.proc_read_bytes += io.read_bytes;
            totals.proc_written_bytes += io.written_bytes;

            let rss_changed = (seen.rss_mb - entry.last_rss_mb).abs() >= ACTIVE_RSS_DELTA_MB;
            let is_new = procs[id].first_tick == tick;
            if !(is_new || rss_changed || seen.cpu >= ACTIVE_CPU_PCT || !io.is_zero() || !inherited_io.is_zero()) {
                continue;
            }
            entry.last_rss_mb = seen.rss_mb;

            // Context switches and thread count need a second /proc file, so they
            // are only read for ticks that are being recorded anyway.
            let extra = read_status_extras(seen.pid).unwrap_or_default();
            samples.entry(id).or_default().push(
                tick,
                &TickSample {
                    cpu: seen.cpu,
                    rss_mb: seen.rss_mb,
                    io,
                    inherited: inherited_io,
                    status: seen.status,
                    threads: extra.threads,
                    vol_ctx: extra.vol_ctx,
                    nonvol_ctx: extra.nonvol_ctx,
                },
            );

            if io.wchar > 0 || io.written_bytes > 0 {
                writers.push((id, io.wchar.max(io.written_bytes), seen.pid));
            }
        }

        tracked.retain(|key, _| alive.contains(key));
        capture_writer_fds(&mut writers, &mut tracked, &mut fds, &mut path_ids, &mut paths, tick, refreshed_at);

        system.t_ms.push(refreshed_at.duration_since(start_instant).as_millis() as u32);
        system
            .cpu_pct
            .push(sys.cpus().iter().map(sysinfo::Cpu::cpu_usage).sum::<f32>() / cpu_cores as f32);
        system.mem_used_mb.push(sys.used_memory() as f32 / 1_048_576.0);
        system.disk_read_mbs.push(bytes_per_sec_to_mbs(device_read, elapsed_secs));
        system.disk_write_mbs.push(bytes_per_sec_to_mbs(device_written, elapsed_secs));
        system.proc_wchar_mbs.push(bytes_per_sec_to_mbs(proc_wchar_total, elapsed_secs));
        system.proc_written_mbs.push(bytes_per_sec_to_mbs(proc_written_total, elapsed_secs));

        progress.ticks_done.store(tick + 1, Ordering::Relaxed);
        progress.processes_seen.store(procs.len(), Ordering::Relaxed);
        debug!("session tick {tick} done, {} processes tracked", procs.len());
    }

    let recorded_ticks = system.t_ms.len();
    let last_tick = recorded_ticks.saturating_sub(1);
    for process in &mut procs {
        process.alive_at_end = process.last_tick >= last_tick;
    }
    // A process seen only on the discarded baseline tick has no samples at all.
    procs.retain(|p| p.first_tick < recorded_ticks);

    info!(
        "Session finished: {recorded_ticks} ticks, {} processes, {} with activity{}",
        procs.len(),
        samples.len(),
        if aborted { " (stopped early)" } else { "" }
    );

    Ok(SessionData {
        meta: SessionMeta {
            format_version: SESSION_FORMAT_VERSION,
            app_version: app_version.to_string(),
            hz: config.hz,
            ticks: recorded_ticks,
            requested_duration_secs: config.duration_secs,
            recorded_duration_secs: system.t_ms.last().map_or(0.0, |ms| f64::from(*ms) / 1000.0),
            start_unix,
            aborted,
            process_count: procs.len(),
            cpu_model: meta_cpu_model,
            cpu_cores,
            total_memory_mb,
            proc_fs: cfg!(target_os = "linux"),
            recorder_pid: std::process::id(),
            device_read_bytes: totals.device_read_bytes,
            device_written_bytes: totals.device_written_bytes,
            proc_rchar_bytes: totals.proc_rchar_bytes,
            proc_wchar_bytes: totals.proc_wchar_bytes,
            proc_read_bytes: totals.proc_read_bytes,
            proc_written_bytes: totals.proc_written_bytes,
        },
        system,
        procs,
        samples,
        paths,
        fds,
    })
}

/// Sleeps in short slices so a stop request is noticed promptly rather than at
/// the end of the tick.  Returns false when the session was asked to stop.
fn sleep_until(deadline: Instant, stop: &AtomicBool) -> bool {
    const SLICE: Duration = Duration::from_millis(20);
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        std::thread::sleep((deadline - now).min(SLICE));
    }
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn bytes_per_sec_to_mbs(bytes: u64, elapsed_secs: f64) -> f32 {
    (bytes as f64 / 1_048_576.0 / elapsed_secs) as f32
}

fn status_char(status: ProcessStatus) -> char {
    match status {
        ProcessStatus::Run => 'R',
        ProcessStatus::Sleep => 'S',
        ProcessStatus::UninterruptibleDiskSleep => 'D',
        ProcessStatus::Zombie => 'Z',
        ProcessStatus::Stop | ProcessStatus::Suspended => 'T',
        ProcessStatus::Idle | ProcessStatus::Parked => 'I',
        ProcessStatus::Tracing => 't',
        ProcessStatus::Dead => 'X',
        _ => '?',
    }
}

// ── /proc readers ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct StatusExtras {
    threads: u32,
    vol_ctx: u64,
    nonvol_ctx: u64,
}

/// All four counters from `/proc/<pid>/io`.
///
/// `rchar`/`wchar` are syscall-level and charged to the process that made the
/// call.  `read_bytes`/`write_bytes` are block-device level and charged to
/// whoever submitted the I/O.  `sysinfo` exposes only the latter pair, which is
/// why this is read directly.
#[cfg(target_os = "linux")]
fn read_io_counters(pid: u32) -> Option<IoCounters> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut counters = IoCounters::default();
    for line in content.lines() {
        let Some((field, value)) = line.split_once(": ") else { continue };
        let Ok(value) = value.trim().parse::<u64>() else { continue };
        match field {
            "rchar" => counters.rchar = value,
            "wchar" => counters.wchar = value,
            "read_bytes" => counters.read_bytes = value,
            "write_bytes" => counters.written_bytes = value,
            _ => {}
        }
    }
    Some(counters)
}

#[cfg(not(target_os = "linux"))]
fn read_io_counters(_pid: u32) -> Option<IoCounters> {
    None
}

#[cfg(target_os = "linux")]
fn read_status_extras(pid: u32) -> Option<StatusExtras> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let mut extras = StatusExtras::default();
    for line in content.lines() {
        let Some((field, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match field {
            "Threads" => extras.threads = value.parse().unwrap_or(0),
            "voluntary_ctxt_switches" => extras.vol_ctx = value.parse().unwrap_or(0),
            "nonvoluntary_ctxt_switches" => extras.nonvol_ctx = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    Some(extras)
}

#[cfg(not(target_os = "linux"))]
fn read_status_extras(_pid: u32) -> Option<StatusExtras> {
    None
}

/// Where a process's writes are actually going.  Reading every descriptor of
/// every process would dominate the cost of a tick, so this is limited to the
/// biggest writers and rescanned at most once a second.
fn capture_writer_fds(
    writers: &mut Vec<(usize, u64, u32)>,
    tracked: &mut HashMap<ProcKey, Tracked>,
    fds: &mut BTreeMap<usize, Vec<FdSnapshot>>,
    path_ids: &mut HashMap<String, u32>,
    paths: &mut Vec<String>,
    tick: usize,
    now: Instant,
) {
    if !cfg!(target_os = "linux") || writers.is_empty() {
        return;
    }
    writers.sort_unstable_by_key(|&(_, bytes, _)| std::cmp::Reverse(bytes));
    writers.truncate(FD_MAX_PROCESSES_PER_TICK);

    for &(id, _, pid) in writers.iter() {
        let Some(entry) = tracked.values_mut().find(|t| t.id == id) else {
            continue;
        };
        if entry
            .last_fd_scan
            .is_some_and(|last| now.duration_since(last).as_secs_f64() < FD_RESCAN_SECS)
        {
            continue;
        }
        entry.last_fd_scan = Some(now);

        let found = read_writable_fd_paths(pid);
        if found.is_empty() {
            continue;
        }
        let indices = found
            .into_iter()
            .map(|path| {
                *path_ids.entry(path.clone()).or_insert_with(|| {
                    paths.push(path);
                    (paths.len() - 1) as u32
                })
            })
            .collect();
        fds.entry(id).or_default().push(FdSnapshot {
            tick: tick as u32,
            paths: indices,
        });
    }
}

/// Regular files a process has open, as resolved from `/proc/<pid>/fd`.
/// Sockets, pipes and anonymous inodes are dropped - they say nothing about
/// what is being written to disk.
#[cfg(target_os = "linux")]
fn read_writable_fd_paths(pid: u32) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten().take(512) {
        let Ok(target) = std::fs::read_link(entry.path()) else { continue };
        let path = target.to_string_lossy();
        if path.starts_with('/') && !path.starts_with("/proc/") && !path.starts_with("/dev/") && !path.starts_with("/sys/") {
            let path = path.into_owned();
            if !out.contains(&path) {
                out.push(path);
            }
            if out.len() >= FD_MAX_PATHS_PER_PROCESS {
                break;
            }
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn read_writable_fd_paths(_pid: u32) -> Vec<String> {
    Vec::new()
}

/// Bytes moved across physical devices since the previous sample.
///
/// `/proc/diskstats` lists partitions and device-mapper targets alongside the
/// devices they sit on, so summing every line would count the same write two or
/// three times.
fn whole_device_bytes(current: &HashMap<String, DiskCounters>, previous: &HashMap<String, DiskCounters>) -> (u64, u64) {
    let mut read_sectors = 0u64;
    let mut write_sectors = 0u64;
    for (name, counters) in current {
        if !is_whole_device(name, current) {
            continue;
        }
        let Some(before) = previous.get(name) else { continue };
        read_sectors += counters.read_sectors.saturating_sub(before.read_sectors);
        write_sectors += counters.write_sectors.saturating_sub(before.write_sectors);
    }
    (read_sectors * 512, write_sectors * 512)
}

fn is_whole_device(name: &str, all: &HashMap<String, DiskCounters>) -> bool {
    if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") || name.starts_with("dm-") {
        return false;
    }
    // A partition's name is its parent's name plus a number, optionally after a
    // "p" separator (`nvme0n1` -> `nvme0n1p3`, `sda` -> `sda1`).
    !all.keys().any(|parent| {
        if parent == name {
            return false;
        }
        let Some(suffix) = name.strip_prefix(parent.as_str()) else {
            return false;
        };
        let digits = suffix.strip_prefix('p').unwrap_or(suffix);
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
    })
}

/// How much of a session file [`parse_meta_prefix`] needs.  `meta` carries only
/// scalars and two short strings, so this is generous by a wide margin.
pub const PREFIX_READ_BYTES: usize = 8192;

/// Deserializes only the leading `meta` object, so listing saved sessions does
/// not read entire files.  `meta` is the first field of [`SessionData`] and holds
/// no unbounded collections, which is what makes the prefix read sufficient.
///
/// Scanning bytes is safe despite the JSON possibly holding multi-byte text:
/// every structural character is ASCII, and UTF-8 continuation bytes are all
/// >= 0x80, so they can never be mistaken for a brace or quote.
pub fn parse_meta_prefix(prefix: &[u8]) -> Option<SessionMeta> {
    const KEY: &[u8] = b"\"meta\":";
    let key_at = prefix.windows(KEY.len()).position(|window| window == KEY)?;
    let after_key = key_at + KEY.len();
    let open = after_key + prefix.get(after_key..)?.iter().position(|&byte| byte == b'{')?;

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in prefix.get(open..)?.iter().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_slice(prefix.get(open..=open + offset)?).ok();
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_rates_that_would_deflate_cpu() {
        let too_fast = SessionConfig {
            duration_secs: 60.0,
            hz: 10.0,
        };
        too_fast.validate().expect_err("10 Hz would deflate per-process CPU%");
        let ok = SessionConfig {
            duration_secs: 60.0,
            hz: 5.0,
        };
        ok.validate().expect("5 Hz is the documented maximum");
    }

    #[test]
    fn rejects_sessions_over_the_sample_cap() {
        let huge = SessionConfig {
            duration_secs: 100_000.0,
            hz: 5.0,
        };
        huge.validate().expect_err("must refuse a session past the sample cap");
    }

    #[test]
    fn derives_tick_count_and_interval() {
        let config = SessionConfig {
            duration_secs: 60.0,
            hz: 5.0,
        };
        assert_eq!(config.tick_count(), 300);
        assert_eq!(config.interval(), Duration::from_millis(200));
    }

    #[test]
    fn treats_partitions_and_mappers_as_duplicates() {
        let all: HashMap<String, DiskCounters> = ["nvme0n1", "nvme0n1p1", "nvme0n1p3", "sda", "sda1", "dm-0", "loop2"]
            .into_iter()
            .map(|name| (name.to_string(), DiskCounters::default()))
            .collect();

        assert!(is_whole_device("nvme0n1", &all));
        assert!(is_whole_device("sda", &all));
        assert!(!is_whole_device("nvme0n1p3", &all));
        assert!(!is_whole_device("sda1", &all));
        assert!(!is_whole_device("dm-0", &all));
        assert!(!is_whole_device("loop2", &all));
    }

    #[test]
    fn sums_only_physical_devices() {
        let previous: HashMap<String, DiskCounters> = ["nvme0n1", "nvme0n1p3"]
            .into_iter()
            .map(|name| (name.to_string(), DiskCounters::default()))
            .collect();
        let counters = DiskCounters {
            read_sectors: 2048,
            write_sectors: 4096,
            busy_ms: 0,
        };
        let current: HashMap<String, DiskCounters> = ["nvme0n1", "nvme0n1p3"].into_iter().map(|name| (name.to_string(), counters)).collect();

        let (read_bytes, written_bytes) = whole_device_bytes(&current, &previous);
        // Both lines carry the same write; only the parent device is counted.
        assert_eq!(read_bytes, 2048 * 512);
        assert_eq!(written_bytes, 4096 * 512);
    }

    #[test]
    fn parses_meta_from_a_truncated_file() {
        let data = SessionData {
            meta: SessionMeta {
                format_version: SESSION_FORMAT_VERSION,
                app_version: "1.2.3".to_string(),
                hz: 5.0,
                ticks: 300,
                requested_duration_secs: 60.0,
                recorded_duration_secs: 59.8,
                start_unix: 1_789_000_000.0,
                aborted: false,
                process_count: 412,
                cpu_model: "Test CPU".to_string(),
                cpu_cores: 12,
                total_memory_mb: 31823.0,
                proc_fs: true,
                recorder_pid: 4242,
                device_read_bytes: 1 << 30,
                device_written_bytes: 2 << 30,
                proc_rchar_bytes: 1 << 29,
                proc_wchar_bytes: 1 << 31,
                proc_read_bytes: 1 << 28,
                proc_written_bytes: 1 << 20,
            },
            system: SessionSystem::default(),
            procs: vec![],
            samples: BTreeMap::new(),
            // Bulk that must stay unread for the prefix parse to be worth doing.
            paths: (0..5_000).map(|i| format!("/home/user/some/long/path/file_{i}")).collect(),
            fds: BTreeMap::new(),
        };
        let json = serde_json::to_vec(&data).expect("serializes");
        assert!(json.len() > 100_000, "test needs a file far larger than the prefix");
        let truncated = json.get(..PREFIX_READ_BYTES).expect("file is larger than the prefix");

        let meta = parse_meta_prefix(truncated).expect("meta lives in the prefix");
        assert_eq!(meta.process_count, 412);
        assert_eq!(meta.device_written_bytes, 2 << 30);
        assert_eq!(meta.ticks, 300);
        assert_eq!(meta.cpu_model, "Test CPU");
    }

    #[test]
    fn credits_a_reaped_child_to_the_child_not_the_parent() {
        // The parent wrote 1 MB itself; its child had written 8 MB before being
        // reaped, which the kernel adds to the parent's counters in one jump.
        let own = 1 << 20;
        let child_total = 8 << 20;
        let parent_delta = IoCounters {
            wchar: own + child_total,
            ..IoCounters::default()
        };
        let folded = IoCounters {
            wchar: child_total,
            ..IoCounters::default()
        };

        assert_eq!(parent_delta.saturating_minus(folded).wchar, own);
        assert_eq!(parent_delta.overlap(folded).wchar, child_total);
    }

    #[test]
    fn never_reports_negative_own_io_when_the_fold_lands_late() {
        // The child vanished but its bytes have not reached the parent yet, so
        // the correction must not push the parent's own figure below zero or
        // claim more inherited bytes than actually appeared.
        let parent_delta = IoCounters::default();
        let folded = IoCounters {
            wchar: 8 << 20,
            ..IoCounters::default()
        };

        assert_eq!(parent_delta.saturating_minus(folded).wchar, 0);
        assert_eq!(parent_delta.overlap(folded).wchar, 0);
    }

    #[test]
    fn keeps_status_string_aligned_with_tick_indices() {
        let mut samples = SessionSamples::default();
        for (tick, status) in [(3usize, 'R'), (7, 'D')] {
            samples.push(
                tick,
                &TickSample {
                    cpu: 1.0,
                    rss_mb: 2.0,
                    io: IoCounters::default(),
                    inherited: IoCounters::default(),
                    status,
                    threads: 4,
                    vol_ctx: 0,
                    nonvol_ctx: 0,
                },
            );
        }
        assert_eq!(samples.t, vec![3, 7]);
        assert_eq!(samples.status, "RD");
        assert_eq!(samples.cpu.len(), samples.t.len());
    }
}
