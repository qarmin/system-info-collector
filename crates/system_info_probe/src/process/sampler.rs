use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{IoCounters, ProcessInfo, ProcessKey, ProcessSample, ProcessState, Selection, StatusDetail};
use crate::error::ProbeResult;

/// What is read per process beyond the basics (CPU time, RSS, state, parent, threads).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcessDetail {
    #[default]
    Basic,
    /// Adds `/proc/<pid>/io`.
    Io,
    /// Adds `/proc/<pid>/io` and `/proc/<pid>/status` (peak RSS, context switches).
    Full,
}

impl ProcessDetail {
    fn io(self) -> bool {
        matches!(self, Self::Io | Self::Full)
    }

    fn status(self) -> bool {
        self == Self::Full
    }
}

/// How often the detail files are re-read.  A process counts as active in a sample when its CPU
/// time or RSS moved or it is running or in disk wait; idle processes keep their last values.
///
/// Both files hold cumulative or peak values, so a skipped read loses nothing - whatever changed
/// shows up in full at the next read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetailRefresh {
    /// `/proc/<pid>/status` is the largest file read per process and its fields barely move.
    pub status_every: Duration,
    /// Re-read even without visible activity, for work too short to move the tick-granular CPU time.
    pub idle_every: Duration,
}

impl Default for DetailRefresh {
    fn default() -> Self {
        Self {
            status_every: Duration::from_secs(1),
            idle_every: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessSamplerConfig {
    pub detail: ProcessDetail,
    /// Processes that get `detail`; everything else is sampled at `Basic`.  `None` means all of
    /// them, which on a busy machine multiplies the files read per sample.
    pub detail_scope: Option<Selection>,
    pub refresh: DetailRefresh,
}

/// Every process alive at one instant.
#[derive(Clone, Debug)]
pub struct ProcessSnapshot {
    pub taken_at: Instant,
    pub unix_time: f64,
    pub logical_cores: usize,
    pub processes: BTreeMap<ProcessKey, ProcessSample>,
}

impl ProcessSnapshot {
    pub fn get(&self, key: &ProcessKey) -> Option<&ProcessSample> {
        self.processes.get(key)
    }

    pub fn by_pid(&self, pid: u32) -> Option<&ProcessSample> {
        self.processes.values().find(|p| p.pid() == pid)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProcessSample> {
        self.processes.values()
    }

    pub fn len(&self) -> usize {
        self.processes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }

    /// The process with `root` and everything below it, root first.
    pub fn tree(&self, root: ProcessKey) -> Vec<ProcessKey> {
        if !self.processes.contains_key(&root) {
            return Vec::new();
        }
        let mut children: HashMap<u32, Vec<ProcessKey>> = HashMap::new();
        for process in self.processes.values() {
            if let Some(ppid) = process.ppid {
                children.entry(ppid).or_default().push(process.key());
            }
        }
        let mut members = vec![root];
        let mut next = 0;
        while let Some(&current) = members.get(next) {
            if let Some(kids) = children.get(&current.pid) {
                members.extend(kids.iter().copied().filter(|k| *k != current));
            }
            next += 1;
        }
        members
    }
}

/// Reads every process, keeping per-process descriptions between samples so cmdline, exe and
/// owner are read once per process rather than once per sample.
pub struct ProcessSampler {
    config: ProcessSamplerConfig,
    known: HashMap<ProcessKey, Known>,
    logical_cores: usize,
    backend: Backend,
}

struct Known {
    info: Arc<ProcessInfo>,
    /// `comm` changes on `exec`, which is when cmdline and exe have to be read again.
    comm: String,
    last_cpu_time: Duration,
    last_rss_bytes: u64,
    detail: DetailCache,
}

#[derive(Default)]
struct DetailCache {
    io: Option<IoCounters>,
    io_read_at: Option<Instant>,
    status: Option<StatusDetail>,
    status_read_at: Option<Instant>,
}

impl DetailCache {
    fn due(read_at: Option<Instant>, now: Instant, every: Duration) -> bool {
        read_at.is_none_or(|at| now.duration_since(at) >= every)
    }
}

impl ProcessSampler {
    pub fn new(config: ProcessSamplerConfig) -> ProbeResult<Self> {
        Ok(Self::with_backend(config, Backend::system()?))
    }

    #[cfg(target_os = "linux")]
    pub fn with_proc_fs(config: ProcessSamplerConfig, proc_fs: crate::backend::procfs::ProcFs) -> Self {
        Self::with_backend(config, Backend { proc_fs, pids: Vec::new() })
    }

    fn with_backend(config: ProcessSamplerConfig, backend: Backend) -> Self {
        Self {
            logical_cores: backend.logical_cores(),
            config,
            known: HashMap::new(),
            backend,
        }
    }

    pub fn config(&self) -> &ProcessSamplerConfig {
        &self.config
    }

    pub fn set_detail_scope(&mut self, scope: Option<Selection>) {
        self.config.detail_scope = scope;
    }

    /// False when the kernel keeps no per-process I/O counters at all, as embedded kernels built
    /// without `CONFIG_TASK_IO_ACCOUNTING` - then unreadable I/O is not a permission problem.
    pub fn io_accounting(&self) -> bool {
        self.backend.io_accounting()
    }

    pub fn sample(&mut self) -> ProbeResult<ProcessSnapshot> {
        let taken_at = Instant::now();
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let processes = self.backend.read_basic(&mut self.known, self.config.detail.io())?;
        let mut snapshot = ProcessSnapshot {
            taken_at,
            unix_time,
            logical_cores: self.logical_cores,
            processes,
        };

        let detail = self.config.detail;
        if detail != ProcessDetail::Basic {
            let scope: Option<HashSet<ProcessKey>> = self.config.detail_scope.as_mut().map(|s| s.update(&snapshot));
            let refresh = self.config.refresh;
            for (key, process) in &mut snapshot.processes {
                if scope.as_ref().is_some_and(|s| !s.contains(key)) {
                    continue;
                }
                let Some(known) = self.known.get_mut(key) else { continue };
                let active = known.last_cpu_time != process.cpu_time
                    || known.last_rss_bytes != process.rss_bytes
                    || matches!(process.state, ProcessState::Running | ProcessState::DiskSleep);
                let cache = &mut known.detail;
                if detail.io() {
                    if active || DetailCache::due(cache.io_read_at, taken_at, refresh.idle_every) {
                        cache.io = self.backend.read_io(process.pid());
                        cache.io_read_at = Some(taken_at);
                    }
                    process.io = cache.io;
                }
                if detail.status() {
                    let every = if active {
                        refresh.status_every
                    } else {
                        refresh.idle_every.max(refresh.status_every)
                    };
                    if DetailCache::due(cache.status_read_at, taken_at, every) {
                        cache.status = self.backend.read_status(process.pid());
                        cache.status_read_at = Some(taken_at);
                    }
                    process.status = cache.status;
                }
            }
        }

        for (key, process) in &snapshot.processes {
            if let Some(known) = self.known.get_mut(key) {
                known.last_cpu_time = process.cpu_time;
                known.last_rss_bytes = process.rss_bytes;
            }
        }
        Ok(snapshot)
    }
}

fn new_known(info: Arc<ProcessInfo>, comm: String) -> Known {
    Known {
        info,
        comm,
        last_cpu_time: Duration::ZERO,
        last_rss_bytes: 0,
        detail: DetailCache::default(),
    }
}

#[cfg(target_os = "linux")]
struct StatFields {
    ppid: Option<u32>,
    state: ProcessState,
    cpu_time: Duration,
    reaped_children_cpu_time: Duration,
    rss_bytes: u64,
    threads: u32,
    start_ticks: u64,
}

#[cfg(target_os = "linux")]
struct Backend {
    proc_fs: crate::backend::procfs::ProcFs,
    pids: Vec<u32>,
}

#[cfg(target_os = "linux")]
impl Backend {
    fn system() -> ProbeResult<Self> {
        Ok(Self {
            proc_fs: crate::backend::procfs::ProcFs::system()?,
            pids: Vec::new(),
        })
    }

    fn logical_cores(&self) -> usize {
        self.proc_fs.cpu_times().map_or(1, |t| t.cores.len().max(1))
    }

    fn io_accounting(&self) -> bool {
        self.proc_fs.io_accounting()
    }

    /// One `stat` read per process; cmdline, exe and owner only for processes not seen before or
    /// that have `exec`ed since.
    fn read_basic(&mut self, known: &mut HashMap<ProcessKey, Known>, _io: bool) -> ProbeResult<BTreeMap<ProcessKey, ProcessSample>> {
        let fs = &self.proc_fs;
        fs.pids_into(&mut self.pids)?;
        let mut processes = BTreeMap::new();
        for &pid in &self.pids {
            // A process can exit between listing /proc and reading it; it is then simply absent.
            let Some((key, fields, cached, fresh)) = fs.with_stat(pid, |stat| {
                let key = ProcessKey {
                    pid,
                    start: stat.start_ticks,
                };
                let cached = known.get(&key).filter(|k| k.comm == stat.comm).map(|k| Arc::clone(&k.info));
                let fresh = cached.is_none().then(|| (stat.comm.to_string(), stat.kernel_thread));
                let fields = StatFields {
                    ppid: stat.ppid,
                    state: stat.state,
                    cpu_time: fs.ticks_to_duration(stat.utime + stat.stime),
                    reaped_children_cpu_time: fs.ticks_to_duration(stat.cutime + stat.cstime),
                    rss_bytes: fs.pages_to_bytes(stat.rss_pages),
                    threads: stat.threads,
                    start_ticks: stat.start_ticks,
                };
                (key, fields, cached, fresh)
            }) else {
                continue;
            };
            let info = match (cached, fresh) {
                (Some(info), _) => info,
                (None, Some((comm, kernel_thread))) => {
                    let info = Arc::new(ProcessInfo {
                        key,
                        name: comm.clone(),
                        cmd: if kernel_thread { Vec::new() } else { fs.cmdline(pid) },
                        exe: if kernel_thread { None } else { fs.exe(pid) },
                        uid: fs.uid(pid),
                        kernel_thread,
                        start_unix: fs.start_unix(fields.start_ticks),
                    });
                    match known.get_mut(&key) {
                        Some(existing) => {
                            existing.info = Arc::clone(&info);
                            existing.comm = comm;
                        }
                        None => {
                            known.insert(key, new_known(Arc::clone(&info), comm));
                        }
                    }
                    info
                }
                (None, None) => continue,
            };
            processes.insert(
                key,
                ProcessSample {
                    info,
                    ppid: fields.ppid,
                    state: fields.state,
                    cpu_time: fields.cpu_time,
                    reaped_children_cpu_time: Some(fields.reaped_children_cpu_time),
                    rss_bytes: fields.rss_bytes,
                    threads: Some(fields.threads),
                    io: None,
                    status: None,
                },
            );
        }
        known.retain(|key, _| processes.contains_key(key));
        Ok(processes)
    }

    fn read_io(&self, pid: u32) -> Option<IoCounters> {
        self.proc_fs.io(pid)
    }

    fn read_status(&self, pid: u32) -> Option<StatusDetail> {
        self.proc_fs.status(pid)
    }
}

#[cfg(not(target_os = "linux"))]
struct Backend {
    sys: sysinfo::System,
}

#[cfg(not(target_os = "linux"))]
impl Backend {
    #[expect(clippy::unnecessary_wraps, reason = "same signature as the Linux backend")]
    fn system() -> ProbeResult<Self> {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        Ok(Self { sys })
    }

    fn logical_cores(&self) -> usize {
        self.sys.cpus().len().max(1)
    }

    #[expect(clippy::unused_self, reason = "same signature as the Linux backend")]
    fn io_accounting(&self) -> bool {
        false
    }

    #[expect(clippy::unnecessary_wraps, reason = "same signature as the Linux backend")]
    fn read_basic(&mut self, known: &mut HashMap<ProcessKey, Known>, io: bool) -> ProbeResult<BTreeMap<ProcessKey, ProcessSample>> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, ThreadKind, UpdateKind};

        let refresh = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cpu()
            .with_memory()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet);
        let refresh = if io { refresh.with_disk_usage() } else { refresh };
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);

        let mut processes = BTreeMap::new();
        for process in self.sys.processes().values() {
            if process.thread_kind() == Some(ThreadKind::Userland) {
                continue;
            }
            let key = ProcessKey {
                pid: process.pid().as_u32(),
                start: process.start_time(),
            };
            let name = process.name().to_string_lossy();
            let info = match known.get(&key) {
                Some(k) if k.comm == name => Arc::clone(&k.info),
                _ => {
                    let info = Arc::new(ProcessInfo {
                        key,
                        name: name.to_string(),
                        cmd: process.cmd().iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                        exe: process.exe().map(std::path::Path::to_path_buf),
                        uid: process.user_id().and_then(uid_number),
                        kernel_thread: process.thread_kind() == Some(ThreadKind::Kernel),
                        start_unix: process.start_time() as f64,
                    });
                    known.insert(key, new_known(Arc::clone(&info), name.to_string()));
                    info
                }
            };
            processes.insert(
                key,
                ProcessSample {
                    info,
                    ppid: process.parent().map(|p| p.as_u32()),
                    state: state_from_sysinfo(process.status()),
                    cpu_time: Duration::from_millis(process.accumulated_cpu_time()),
                    reaped_children_cpu_time: None,
                    rss_bytes: process.memory(),
                    threads: None,
                    io: None,
                    status: None,
                },
            );
        }
        known.retain(|key, _| processes.contains_key(key));
        Ok(processes)
    }

    fn read_io(&self, pid: u32) -> Option<IoCounters> {
        let usage = self.sys.process(sysinfo::Pid::from_u32(pid))?.disk_usage();
        Some(IoCounters {
            rchar: 0,
            wchar: 0,
            read_bytes: usage.total_read_bytes,
            written_bytes: usage.total_written_bytes,
        })
    }

    #[expect(clippy::unused_self, reason = "same signature as the Linux backend")]
    fn read_status(&self, _pid: u32) -> Option<StatusDetail> {
        None
    }
}

#[cfg(all(not(target_os = "linux"), unix))]
fn uid_number(uid: &sysinfo::Uid) -> Option<u32> {
    use std::ops::Deref;
    Some(*uid.deref())
}

#[cfg(all(not(target_os = "linux"), not(unix)))]
fn uid_number(_uid: &sysinfo::Uid) -> Option<u32> {
    None
}

#[cfg(not(target_os = "linux"))]
fn state_from_sysinfo(status: sysinfo::ProcessStatus) -> ProcessState {
    use sysinfo::ProcessStatus;
    match status {
        ProcessStatus::Run => ProcessState::Running,
        ProcessStatus::Sleep => ProcessState::Sleeping,
        ProcessStatus::UninterruptibleDiskSleep => ProcessState::DiskSleep,
        ProcessStatus::Zombie => ProcessState::Zombie,
        ProcessStatus::Stop | ProcessStatus::Suspended => ProcessState::Stopped,
        ProcessStatus::Idle | ProcessStatus::Parked => ProcessState::Idle,
        ProcessStatus::Tracing => ProcessState::Tracing,
        ProcessStatus::Dead => ProcessState::Dead,
        _ => ProcessState::Unknown,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::backend::procfs::ProcFs;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("probe-fixture-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("temp dir");
            std::fs::write(
                root.join("stat"),
                "cpu  10 0 10 100 0 0 0 0 0 0\ncpu0 10 0 10 100 0 0 0 0 0 0\nbtime 1700000000\n",
            )
            .expect("write");
            Self { root }
        }

        fn process(&self, pid: u32, comm: &str, utime: u64, cmdline: &str) {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(&dir).expect("pid dir");
            let stat = format!("{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194560 0 0 0 0 {utime} 0 0 0 20 0 1 0 500 1000 100 0");
            std::fs::write(dir.join("stat"), stat).expect("write");
            std::fs::write(dir.join("cmdline"), cmdline.replace(' ', "\0")).expect("write");
        }

        fn file(&self, pid: u32, name: &str, content: &str) {
            std::fs::write(self.root.join(pid.to_string()).join(name), content).expect("write");
        }

        fn remove(&self, pid: u32) {
            std::fs::remove_dir_all(self.root.join(pid.to_string())).expect("remove");
        }

        fn sampler(&self, config: ProcessSamplerConfig) -> ProcessSampler {
            ProcessSampler::with_proc_fs(config, ProcFs::at(self.root.clone(), 100, 4096).expect("fixture proc"))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn only(snapshot: &ProcessSnapshot) -> &ProcessSample {
        snapshot.iter().next().expect("one process")
    }

    #[test]
    fn reads_cmdline_once_and_again_only_after_exec() {
        let fixture = Fixture::new("exec");
        fixture.process(42, "app", 100, "app --one");
        let mut sampler = fixture.sampler(ProcessSamplerConfig::default());
        let first = sampler.sample().expect("sample");
        assert_eq!(only(&first).info.cmd, vec!["app", "--one"]);
        assert_eq!(only(&first).cpu_time, Duration::from_secs(1));
        assert_eq!(only(&first).rss_bytes, 100 * 4096);

        // A changed cmdline alone is not re-read; the same Arc is handed out.
        fixture.file(42, "cmdline", "app\0--two");
        let second = sampler.sample().expect("sample");
        assert!(Arc::ptr_eq(&only(&first).info, &only(&second).info));

        // exec changes comm, which triggers the re-read.
        fixture.process(42, "worker", 100, "worker --three");
        let third = sampler.sample().expect("sample");
        assert_eq!(only(&third).info.cmd, vec!["worker", "--three"]);
        assert_eq!(only(&third).key(), only(&first).key());
    }

    #[test]
    fn forgets_exited_processes() {
        let fixture = Fixture::new("exit");
        fixture.process(1, "init", 0, "init");
        fixture.process(42, "app", 0, "app");
        let mut sampler = fixture.sampler(ProcessSamplerConfig::default());
        assert_eq!(sampler.sample().expect("sample").len(), 2);
        fixture.remove(42);
        assert_eq!(sampler.sample().expect("sample").len(), 1);
        assert_eq!(sampler.known.len(), 1);
    }

    #[test]
    fn skips_detail_reads_for_idle_processes() {
        let fixture = Fixture::new("idle");
        fixture.process(42, "app", 100, "app");
        fixture.file(42, "io", "rchar: 1\nwchar: 2\nread_bytes: 0\nwrite_bytes: 0\n");
        fixture.file(42, "status", "VmHWM:\t1 kB\n");
        let mut sampler = fixture.sampler(ProcessSamplerConfig {
            detail: ProcessDetail::Full,
            detail_scope: None,
            refresh: DetailRefresh {
                status_every: Duration::ZERO,
                idle_every: Duration::from_secs(3600),
            },
        });
        let first = sampler.sample().expect("sample");
        assert_eq!(only(&first).io.map(|io| io.wchar), Some(2));

        // Idle (CPU time and RSS unchanged, sleeping): the cached values are reused.
        fixture.file(42, "io", "rchar: 1\nwchar: 99\nread_bytes: 0\nwrite_bytes: 0\n");
        fixture.file(42, "status", "VmHWM:\t5 kB\n");
        let second = sampler.sample().expect("sample");
        assert_eq!(only(&second).io.map(|io| io.wchar), Some(2));
        assert_eq!(only(&second).status.map(|s| s.peak_rss_bytes), Some(1024));

        // Activity makes both current again.
        fixture.process(42, "app", 101, "app");
        let third = sampler.sample().expect("sample");
        assert_eq!(only(&third).io.map(|io| io.wchar), Some(99));
        assert_eq!(only(&third).status.map(|s| s.peak_rss_bytes), Some(5 * 1024));
    }

    #[test]
    fn reads_detail_only_within_the_scope() {
        let fixture = Fixture::new("scope");
        fixture.process(10, "a", 0, "a");
        fixture.process(20, "b", 0, "b");
        for pid in [10, 20] {
            fixture.file(pid, "io", "wchar: 5\n");
        }
        let mut sampler = fixture.sampler(ProcessSamplerConfig {
            detail: ProcessDetail::Io,
            detail_scope: Some(Selection::new(crate::process::Selector::Pid(20))),
            refresh: DetailRefresh::default(),
        });
        let snapshot = sampler.sample().expect("sample");
        let io: Vec<(u32, bool)> = snapshot.iter().map(|p| (p.pid(), p.io.is_some())).collect();
        assert_eq!(io, vec![(10, false), (20, true)]);
    }
}
