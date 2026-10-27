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

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Error;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use system_info_probe::disk::{self, DiskCounters};
use system_info_probe::process::{
    Coverage, DetailRefresh, IoCounters, ProcessDetail, ProcessInterval, ProcessKey, ProcessSampler, ProcessSamplerConfig,
};
use system_info_probe::system::{MachineInfo, SystemSampler};

pub const SESSION_FORMAT_VERSION: u32 = 1;

/// Upper bound on the sampling rate.  Per-process CPU comes from `utime`/`stime`, which the
/// kernel counts in clock ticks (10 ms at the usual USER_HZ of 100), so one sample can only
/// tell CPU apart in steps of 10 ms over its own length: 4% of a core at 4 Hz, 10% at 10 Hz.
/// Faster sampling makes every figure coarser rather than more detailed.
pub const MAX_SESSION_HZ: f32 = 4.0;

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
#[cfg(target_os = "linux")]
const FD_MAX_PATHS_PER_PROCESS: usize = 64;

/// How often `/proc/<pid>/status` is re-read per process - see `Tracked::extras`.
const STATUS_RESCAN_SECS: f64 = 1.0;

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
                "Sampling rate must be between {MIN_SESSION_HZ} and {MAX_SESSION_HZ} Hz (per-process CPU is counted in \
                 10 ms clock ticks, so faster samples only get coarser)"
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

const fn default_true() -> bool {
    true
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
    /// How many processes had unreadable I/O counters.  When this is most of them,
    /// per-process attribution is structurally incomplete and the unclaimed
    /// remainder in the totals says nothing - see `io_accounting` for whether more
    /// privilege would help or the kernel simply does not keep these counters.
    pub processes_without_io: usize,
    /// The uid the recorder ran as, to compare against `SessionProcess::uid`.
    pub recorder_uid: Option<u32>,
    /// False when the kernel has no `/proc/<pid>/io` at all - built without
    /// `CONFIG_TASK_IO_ACCOUNTING`, as embedded kernels routinely are.  The
    /// distinction matters because then no amount of privilege helps, and telling
    /// the user to re-record as root sends them after a fix that does not exist.
    /// Defaults to true so sessions recorded before this field stay readable.
    #[serde(default = "default_true")]
    pub io_accounting: bool,
    /// Per-device breakdown of the figures above, including the devices that were
    /// deliberately skipped.  Without this a suspicious device total cannot be
    /// audited - you cannot tell a real busy disk from the same write counted
    /// twice under two names.
    pub devices: Vec<DeviceTotals>,
}

/// One block device's contribution to the recording.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceTotals {
    pub name: String,
    pub read_bytes: u64,
    pub written_bytes: u64,
    /// False for devices left out of the machine-wide totals because their
    /// traffic is already counted under another name - partitions, device-mapper
    /// targets, software RAID and NVMe multipath aliases.
    pub counted: bool,
    /// Why it was skipped, for the cases where that is not obvious.
    pub skipped_because: Option<String>,
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
    /// False when `/proc/<pid>/io` could not be read at all.
    ///
    /// That file is mode 0400 and needs permission over the target, so a recorder
    /// running as an ordinary user gets nothing for processes it does not own -
    /// every system daemon and every kernel thread.  Reporting zeros for those
    /// would be indistinguishable from "wrote nothing", which is exactly the wrong
    /// conclusion, so the difference is recorded.
    pub io_readable: bool,
    /// Owner of the process, from the ownership of its `/proc` directory.  The
    /// readable set is almost always "processes with this uid", so seeing the uids
    /// makes an incomplete recording obvious.
    pub uid: Option<u32>,
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
    /// name -> (read, written, why it was not counted)
    devices: BTreeMap<String, (u64, u64, Option<String>)>,
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
    last_rss_mb: f32,
    last_fd_scan: Option<Instant>,
}

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

    // I/O counters are read for every process in every sample, so a write by a process that
    // used no measurable CPU is still attributed to the tick it happened in; status, the largest
    // file, only for processes that did something, and at most once a second.
    let mut sampler = ProcessSampler::new(ProcessSamplerConfig {
        detail: ProcessDetail::Full,
        detail_scope: None,
        refresh: DetailRefresh {
            status_every: Duration::from_secs_f64(STATUS_RESCAN_SECS),
            idle_io_every: Duration::ZERO,
            idle_status_every: None,
        },
        coverage: Coverage::All,
    })?;
    let mut system_sampler = SystemSampler::new()?;
    let machine = MachineInfo::read();
    let io_accounting = sampler.io_accounting();

    let mut tracked: HashMap<ProcessKey, Tracked> = HashMap::new();
    let mut procs: Vec<SessionProcess> = Vec::new();
    let mut samples: BTreeMap<usize, SessionSamples> = BTreeMap::new();
    let mut fds: BTreeMap<usize, Vec<FdSnapshot>> = BTreeMap::new();
    let mut path_ids: HashMap<String, u32> = HashMap::new();
    let mut paths: Vec<String> = Vec::new();
    let mut system = SessionSystem::default();

    let mut totals = SessionTotals::default();
    let mut previous = sampler.sample()?;
    let mut previous_system = system_sampler.sample()?;
    let mut disk_prev = disk::read_counters();
    let start_instant = Instant::now();
    let start_unix = unix_now();
    let mut aborted = false;

    for tick in 0..total_ticks {
        let deadline = start_instant + interval.mul_f64((tick + 1) as f64);
        if !sleep_until(deadline, stop) {
            aborted = true;
            break;
        }

        let current = sampler.sample()?;
        let current_system = system_sampler.sample()?;
        let refreshed_at = current.taken_at;
        let elapsed_secs = refreshed_at.duration_since(previous.taken_at).as_secs_f64().max(1e-6);

        let disk_current = disk::read_counters();
        let mut device_read = 0u64;
        let mut device_written = 0u64;
        for (name, read, written) in device_deltas(&disk_current, &disk_prev) {
            let reason = disk::duplicate_reason(&name, &disk_current);
            if reason.is_none() {
                device_read += read;
                device_written += written;
            }
            let entry = totals.devices.entry(name).or_insert((0, 0, reason));
            entry.0 += read;
            entry.1 += written;
        }
        disk_prev = disk_current;
        totals.device_read_bytes += device_read;
        totals.device_written_bytes += device_written;

        // Reaped children's I/O lands in their parent's counters; `ProcessInterval` separates it
        // out as `inherited_io` so shells and service managers are not blamed for it.
        let changes = ProcessInterval::between(&previous, &current);
        let mut writers: Vec<(usize, u64, u32)> = Vec::new();
        let mut proc_wchar_total: u64 = 0;
        let mut proc_written_total: u64 = 0;

        for (key, sample) in &current.processes {
            let Some(delta) = changes.get(key) else { continue };
            let entry = tracked.entry(*key).or_insert_with(|| {
                let id = procs.len();
                let at_start = previous.get(key).and_then(|p| p.io).unwrap_or_default();
                procs.push(SessionProcess {
                    id,
                    pid: key.pid,
                    ppid: sample.ppid,
                    name: sample.info.name.clone(),
                    exe: sample.info.exe.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                    cmd: sample.info.cmd_line(),
                    start_unix: sample.info.start_unix as u64,
                    kernel: sample.info.kernel_thread,
                    first_tick: tick,
                    last_tick: tick,
                    alive_at_end: false,
                    io_total: sample.io.unwrap_or_default().into(),
                    io_at_start: at_start.into(),
                    io_readable: sample.io.is_some(),
                    uid: sample.info.uid,
                });
                Tracked {
                    id,
                    last_rss_mb: f32::MIN,
                    last_fd_scan: None,
                }
            });

            let id = entry.id;
            procs[id].last_tick = tick;
            if let Some(raw) = sample.io {
                procs[id].io_total = raw.into();
                procs[id].io_readable = true;
            }

            // An unreadable counter contributes nothing rather than a zero: the delta from a
            // fabricated zero would be a lie in both directions.
            let io = delta.io.unwrap_or_default();
            let inherited_io = delta.inherited_io.unwrap_or_default();
            proc_wchar_total += io.wchar;
            proc_written_total += io.written_bytes;
            totals.proc_rchar_bytes += io.rchar;
            totals.proc_wchar_bytes += io.wchar;
            totals.proc_read_bytes += io.read_bytes;
            totals.proc_written_bytes += io.written_bytes;

            let cpu = changes.cpu_percent_of_machine(delta.cpu_time) as f32;
            let rss_mb = sample.rss_bytes as f32 / 1_048_576.0;
            let rss_changed = (rss_mb - entry.last_rss_mb).abs() >= ACTIVE_RSS_DELTA_MB;
            let is_new = procs[id].first_tick == tick;
            if !(is_new || rss_changed || cpu >= ACTIVE_CPU_PCT || !io.is_zero() || !inherited_io.is_zero()) {
                continue;
            }
            entry.last_rss_mb = rss_mb;

            let status = sample.status.unwrap_or_default();
            samples.entry(id).or_default().push(
                tick,
                &TickSample {
                    cpu,
                    rss_mb,
                    io,
                    inherited: inherited_io,
                    status: sample.state.as_char(),
                    threads: sample.threads.unwrap_or(0),
                    vol_ctx: status.voluntary_ctx_switches,
                    nonvol_ctx: status.involuntary_ctx_switches,
                },
            );

            if io.wchar > 0 || io.written_bytes > 0 {
                writers.push((id, io.wchar.max(io.written_bytes), key.pid));
            }
        }

        tracked.retain(|key, _| current.processes.contains_key(key));
        capture_writer_fds(&mut writers, &mut tracked, &mut fds, &mut path_ids, &mut paths, tick, refreshed_at);

        system.t_ms.push(refreshed_at.duration_since(start_instant).as_millis() as u32);
        system
            .cpu_pct
            .push(current_system.cpu_usage_since(&previous_system).map_or(0.0, |u| u.total_percent));
        system.mem_used_mb.push(current_system.memory.used_bytes as f32 / 1_048_576.0);
        system.disk_read_mbs.push(bytes_per_sec_to_mbs(device_read, elapsed_secs));
        system.disk_write_mbs.push(bytes_per_sec_to_mbs(device_written, elapsed_secs));
        system.proc_wchar_mbs.push(bytes_per_sec_to_mbs(proc_wchar_total, elapsed_secs));
        system.proc_written_mbs.push(bytes_per_sec_to_mbs(proc_written_total, elapsed_secs));

        previous = current;
        previous_system = current_system;
        progress.ticks_done.store(tick + 1, Ordering::Relaxed);
        progress.processes_seen.store(procs.len(), Ordering::Relaxed);
        debug!("session tick {tick} done, {} processes tracked", procs.len());
    }

    let processes_without_io = procs.iter().filter(|p| !p.io_readable).count();
    if processes_without_io > 0 {
        if io_accounting {
            info!(
                "Could not read /proc/<pid>/io for {processes_without_io} of {} processes - per-process I/O for those \
                 is unknown, not zero. Run with more privilege for full attribution.",
                procs.len()
            );
        } else {
            info!(
                "This kernel has no /proc/<pid>/io, so per-process I/O is unavailable for all {} processes - it was \
                 built without CONFIG_TASK_IO_ACCOUNTING. No privilege changes that; only device-wide I/O is real here.",
                procs.len()
            );
        }
    }

    let recorded_ticks = system.t_ms.len();
    let last_tick = recorded_ticks.saturating_sub(1);
    for process in &mut procs {
        process.alive_at_end = process.last_tick >= last_tick;
    }

    info!(
        "Session finished: {recorded_ticks} ticks, {} processes, {} with activity{}",
        procs.len(),
        samples.len(),
        if aborted { " (stopped early)" } else { "" }
    );

    let recorder_pid = std::process::id();
    let recorder_uid = previous.processes.values().find(|p| p.pid() == recorder_pid).and_then(|p| p.info.uid);
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
            cpu_model: machine.cpu_model.unwrap_or_default(),
            cpu_cores: machine.logical_cores,
            total_memory_mb: machine.total_memory_bytes as f64 / 1_048_576.0,
            proc_fs: cfg!(target_os = "linux"),
            recorder_pid,
            device_read_bytes: totals.device_read_bytes,
            device_written_bytes: totals.device_written_bytes,
            proc_rchar_bytes: totals.proc_rchar_bytes,
            proc_wchar_bytes: totals.proc_wchar_bytes,
            proc_read_bytes: totals.proc_read_bytes,
            proc_written_bytes: totals.proc_written_bytes,
            processes_without_io,
            recorder_uid,
            io_accounting,
            devices: totals
                .devices
                .into_iter()
                .map(|(name, (read_bytes, written_bytes, skipped_because))| DeviceTotals {
                    name,
                    read_bytes,
                    written_bytes,
                    counted: skipped_because.is_none(),
                    skipped_because,
                })
                .collect(),
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

// ── open files ────────────────────────────────────────────────────────────────

/// Where a process's writes are actually going.  Reading every descriptor of
/// every process would dominate the cost of a tick, so this is limited to the
/// biggest writers and rescanned at most once a second.
fn capture_writer_fds(
    writers: &mut Vec<(usize, u64, u32)>,
    tracked: &mut HashMap<ProcessKey, Tracked>,
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

/// Bytes moved by every device since the previous sample, keyed by device name.
///
/// Every line of `/proc/diskstats` is reported, counted or not, so the machine
/// total can be audited afterwards.
fn device_deltas(current: &HashMap<String, DiskCounters>, previous: &HashMap<String, DiskCounters>) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    for (name, counters) in current {
        let Some(before) = previous.get(name) else { continue };
        let read = counters.read_sectors.saturating_sub(before.read_sectors) * 512;
        let written = counters.write_sectors.saturating_sub(before.write_sectors) * 512;
        if read > 0 || written > 0 {
            out.push((name.clone(), read, written));
        }
    }
    out
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
    fn rejects_rates_that_would_coarsen_cpu() {
        let too_fast = SessionConfig {
            duration_secs: 60.0,
            hz: 10.0,
        };
        too_fast.validate().expect_err("10 Hz would make per-process CPU% too coarse");
        let ok = SessionConfig {
            duration_secs: 60.0,
            hz: MAX_SESSION_HZ,
        };
        ok.validate().expect("the documented maximum must be accepted");
    }

    /// At the maximum rate one sample still spans 25 clock ticks of 10 ms, so a single busy
    /// core is resolved to 4% of itself.
    #[test]
    fn keeps_cpu_resolution_at_the_maximum_rate() {
        let interval = SessionConfig {
            duration_secs: 60.0,
            hz: MAX_SESSION_HZ,
        }
        .interval();
        assert!(interval >= Duration::from_millis(250), "{interval:?}");
    }

    #[test]
    fn rejects_sessions_over_the_sample_cap() {
        let huge = SessionConfig {
            duration_secs: 100_000.0,
            hz: 4.0,
        };
        huge.validate().expect_err("must refuse a session past the sample cap");
    }

    #[test]
    fn derives_tick_count_and_interval() {
        let config = SessionConfig {
            duration_secs: 60.0,
            hz: 4.0,
        };
        assert_eq!(config.tick_count(), 240);
        assert_eq!(config.interval(), Duration::from_millis(250));
    }

    #[test]
    fn reports_every_device_that_moved_bytes() {
        let names = ["nvme0n1", "nvme0n1p3", "quiet0"];
        let previous: HashMap<String, DiskCounters> = names.into_iter().map(|name| (name.to_string(), DiskCounters::default())).collect();
        let busy = DiskCounters {
            read_sectors: 2048,
            write_sectors: 4096,
            busy_ms: 0,
        };
        let mut current: HashMap<String, DiskCounters> = ["nvme0n1", "nvme0n1p3"].into_iter().map(|name| (name.to_string(), busy)).collect();
        current.insert("quiet0".to_string(), DiskCounters::default());

        let deltas = device_deltas(&current, &previous);
        // Idle devices are left out; both views of the busy one are reported so the
        // duplicate stays visible rather than being silently dropped.
        assert_eq!(deltas.len(), 2);
        assert!(deltas.iter().all(|(_, read, written)| *read == 2048 * 512 && *written == 4096 * 512));

        let counted: u64 = deltas
            .iter()
            .filter(|(name, _, _)| disk::duplicate_reason(name, &current).is_none())
            .map(|(_, _, written)| written)
            .sum();
        assert_eq!(counted, 4096 * 512, "the partition must not be added to its disk");
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
                processes_without_io: 0,
                recorder_uid: Some(1000),
                io_accounting: true,
                devices: vec![],
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
