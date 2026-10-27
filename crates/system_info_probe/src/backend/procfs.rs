use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{read_bytes_into, read_text_into};
use crate::error::{ProbeError, ProbeResult};
use crate::process::{IoCounters, ProcessState, StatusDetail};
use crate::system::{CoreTicks, CpuTimes, MemoryInfo};

const PF_KTHREAD: u64 = 0x0020_0000;

/// Fields of `/proc/<pid>/stat` the probe uses, in clock ticks and pages as the kernel reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawStat<'a> {
    pub comm: &'a str,
    pub state: ProcessState,
    pub ppid: Option<u32>,
    pub kernel_thread: bool,
    pub utime: u64,
    pub stime: u64,
    pub cutime: u64,
    pub cstime: u64,
    pub threads: u32,
    pub start_ticks: u64,
    pub rss_pages: u64,
}

/// `comm` may contain spaces and parentheses, so it is everything between the first `(` and the
/// last `)`; the numbered fields follow the last `)`.
pub fn parse_stat(content: &str) -> Option<RawStat<'_>> {
    let open = content.find('(')?;
    let close = content.rfind(')')?;
    let comm = content.get(open + 1..close)?;
    let mut fields = [""; 22];
    let mut count = 0;
    for (slot, field) in fields.iter_mut().zip(content.get(close + 1..)?.split_ascii_whitespace()) {
        *slot = field;
        count += 1;
    }
    if count < fields.len() {
        return None;
    }
    let number = |index: usize| fields[index].parse::<u64>().ok();
    // cutime/cstime/rss are signed in the kernel's format but never negative in practice.
    let signed = |index: usize| fields[index].parse::<i64>().ok().map(|v| v.max(0) as u64);
    let ppid = number(1)? as u32;
    Some(RawStat {
        comm,
        state: ProcessState::from_char(fields[0].chars().next()?),
        ppid: (ppid != 0).then_some(ppid),
        kernel_thread: number(6)? & PF_KTHREAD != 0,
        utime: number(11)?,
        stime: number(12)?,
        cutime: signed(13)?,
        cstime: signed(14)?,
        threads: number(17)? as u32,
        start_ticks: number(19)?,
        rss_pages: signed(21)?,
    })
}

pub fn parse_cmdline(content: &[u8]) -> Vec<String> {
    content
        .split(|&b| b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

pub fn parse_io(content: &str) -> IoCounters {
    let mut counters = IoCounters::default();
    for line in content.lines() {
        let Some((field, value)) = line.split_once(':') else { continue };
        let Ok(value) = value.trim().parse::<u64>() else { continue };
        match field {
            "rchar" => counters.rchar = value,
            "wchar" => counters.wchar = value,
            "read_bytes" => counters.read_bytes = value,
            "write_bytes" => counters.written_bytes = value,
            _ => {}
        }
    }
    counters
}

pub fn parse_status(content: &str) -> StatusDetail {
    let mut detail = StatusDetail::default();
    for line in content.lines() {
        let Some((field, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match field {
            "VmHWM" => detail.peak_rss_bytes = parse_kib(value).unwrap_or(0),
            "voluntary_ctxt_switches" => detail.voluntary_ctx_switches = value.parse().unwrap_or(0),
            "nonvoluntary_ctxt_switches" => detail.involuntary_ctx_switches = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    detail
}

fn parse_kib(value: &str) -> Option<u64> {
    let number = value.strip_suffix("kB").unwrap_or(value).trim();
    number.parse::<u64>().ok().map(|kib| kib * 1024)
}

/// The aggregate `cpu` line and every `cpuN` line of `/proc/stat`, plus `btime`.
pub fn parse_proc_stat(content: &str) -> (Option<CpuTimes>, Option<u64>) {
    let mut total = None;
    let mut cores = Vec::new();
    let mut boot_time = None;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(label) = parts.next() else { continue };
        if label == "btime" {
            boot_time = parts.next().and_then(|v| v.parse().ok());
            continue;
        }
        let Some(index) = label.strip_prefix("cpu") else { continue };
        let mut values = [0u64; 8];
        let mut count = 0;
        for (slot, value) in values.iter_mut().zip(parts) {
            *slot = value.parse().unwrap_or(0);
            count += 1;
        }
        let Some(ticks) = CoreTicks::from_proc_stat_fields(&values[..count]) else {
            continue;
        };
        if index.is_empty() {
            total = Some(ticks);
        } else {
            cores.push(ticks);
        }
    }
    (total.map(|total| CpuTimes { total, cores }), boot_time)
}

pub fn parse_meminfo(content: &str) -> Option<MemoryInfo> {
    let mut total = None;
    let mut free = None;
    let mut available = None;
    let mut swap_total = None;
    let mut swap_free = None;
    for line in content.lines() {
        let Some((field, value)) = line.split_once(':') else { continue };
        let slot = match field {
            "MemTotal" => &mut total,
            "MemFree" => &mut free,
            "MemAvailable" => &mut available,
            "SwapTotal" => &mut swap_total,
            "SwapFree" => &mut swap_free,
            _ => continue,
        };
        *slot = parse_kib(value.trim());
    }
    let total = total?;
    let free = free?;
    // Kernels before 3.14 have no MemAvailable; free memory is the conservative stand-in.
    let available = available.unwrap_or(free);
    Some(MemoryInfo {
        total_bytes: total,
        free_bytes: free,
        available_bytes: available,
        used_bytes: total.saturating_sub(available),
        swap_total_bytes: swap_total.unwrap_or(0),
        swap_free_bytes: swap_free.unwrap_or(0),
    })
}

/// Reads one `/proc`-like tree.  The root is configurable so tests can point it at fixtures.
///
/// Per-process files are read through one reused path and one reused buffer, so a sample of a
/// few hundred processes does not allocate per file.
#[derive(Debug)]
pub struct ProcFs {
    root: PathBuf,
    clock_ticks_per_sec: u64,
    page_size: u64,
    boot_time_unix: u64,
    stat_path: PathBuf,
    meminfo_path: PathBuf,
    scratch: RefCell<Scratch>,
}

#[derive(Clone, Copy)]
enum RootFile {
    Stat,
    Meminfo,
}

#[derive(Debug, Default)]
struct Scratch {
    path: String,
    text: String,
    bytes: Vec<u8>,
}

impl Clone for ProcFs {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            clock_ticks_per_sec: self.clock_ticks_per_sec,
            page_size: self.page_size,
            boot_time_unix: self.boot_time_unix,
            stat_path: self.stat_path.clone(),
            meminfo_path: self.meminfo_path.clone(),
            scratch: RefCell::default(),
        }
    }
}

impl ProcFs {
    pub fn system() -> ProbeResult<Self> {
        Self::at(
            PathBuf::from("/proc"),
            sysconf(libc::_SC_CLK_TCK).unwrap_or(100),
            sysconf(libc::_SC_PAGESIZE).unwrap_or(4096),
        )
    }

    pub fn at(root: PathBuf, clock_ticks_per_sec: u64, page_size: u64) -> ProbeResult<Self> {
        let stat_path = root.join("stat");
        let content = std::fs::read_to_string(&stat_path).map_err(|e| ProbeError::read(&stat_path, e))?;
        let (_, boot_time) = parse_proc_stat(&content);
        let boot_time_unix = boot_time.ok_or_else(|| ProbeError::parse(&stat_path, "no btime line"))?;
        Ok(Self {
            meminfo_path: root.join("meminfo"),
            stat_path,
            root,
            clock_ticks_per_sec: clock_ticks_per_sec.max(1),
            page_size,
            boot_time_unix,
            scratch: RefCell::default(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ticks_to_duration(&self, ticks: u64) -> Duration {
        Duration::from_secs(ticks / self.clock_ticks_per_sec)
            + Duration::from_nanos((ticks % self.clock_ticks_per_sec) * 1_000_000_000 / self.clock_ticks_per_sec)
    }

    pub fn start_unix(&self, start_ticks: u64) -> f64 {
        self.boot_time_unix as f64 + start_ticks as f64 / self.clock_ticks_per_sec as f64
    }

    pub fn pages_to_bytes(&self, pages: u64) -> u64 {
        pages * self.page_size
    }

    /// `/proc` lists thread-group leaders only, so userland threads never show up here.
    pub fn pids_into(&self, pids: &mut Vec<u32>) -> ProbeResult<()> {
        pids.clear();
        let entries = std::fs::read_dir(&self.root).map_err(|e| ProbeError::read(&self.root, e))?;
        pids.extend(
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok())),
        );
        pids.sort_unstable();
        Ok(())
    }

    fn set_pid_path(&self, path: &mut String, pid: u32, file: &str) {
        path.clear();
        path.push_str(&self.root.to_string_lossy());
        let _ = write!(path, "/{pid}");
        if !file.is_empty() {
            path.push('/');
            path.push_str(file);
        }
    }

    /// Reads a small text file into the shared buffer and hands it to `parse`.
    fn with_text<R>(&self, pid: u32, file: &str, parse: impl FnOnce(&str) -> Option<R>) -> Option<R> {
        let mut scratch = self.scratch.borrow_mut();
        let Scratch { path, text, .. } = &mut *scratch;
        self.set_pid_path(path, pid, file);
        read_text_into(&*path, text).ok()?;
        parse(text)
    }

    pub fn with_stat<R>(&self, pid: u32, use_stat: impl FnOnce(RawStat<'_>) -> R) -> Option<R> {
        self.with_text(pid, "stat", |text| parse_stat(text).map(use_stat))
    }

    pub fn cmdline(&self, pid: u32) -> Vec<String> {
        let mut scratch = self.scratch.borrow_mut();
        let Scratch { path, bytes, .. } = &mut *scratch;
        self.set_pid_path(path, pid, "cmdline");
        match read_bytes_into(&*path, bytes) {
            Ok(()) => parse_cmdline(bytes),
            Err(_) => Vec::new(),
        }
    }

    pub fn exe(&self, pid: u32) -> Option<PathBuf> {
        let mut scratch = self.scratch.borrow_mut();
        self.set_pid_path(&mut scratch.path, pid, "exe");
        std::fs::read_link(&scratch.path).ok()
    }

    /// Owner of the process from its `/proc` directory - one `stat`, no file read.
    pub fn uid(&self, pid: u32) -> Option<u32> {
        use std::os::unix::fs::MetadataExt;
        let mut scratch = self.scratch.borrow_mut();
        self.set_pid_path(&mut scratch.path, pid, "");
        std::fs::metadata(&scratch.path).ok().map(|meta| meta.uid())
    }

    pub fn io(&self, pid: u32) -> Option<IoCounters> {
        self.with_text(pid, "io", |text| Some(parse_io(text)))
    }

    pub fn status(&self, pid: u32) -> Option<StatusDetail> {
        self.with_text(pid, "status", |text| Some(parse_status(text)))
    }

    /// False when the kernel was built without `CONFIG_TASK_IO_ACCOUNTING`, so no amount of
    /// privilege makes `/proc/<pid>/io` appear.
    pub fn io_accounting(&self) -> bool {
        self.root.join("self/io").exists()
    }

    fn with_root_file<R>(&self, file: RootFile, parse: impl FnOnce(&str) -> Option<R>, missing: &str) -> ProbeResult<R> {
        let path = match file {
            RootFile::Stat => &self.stat_path,
            RootFile::Meminfo => &self.meminfo_path,
        };
        let mut scratch = self.scratch.borrow_mut();
        let text = &mut scratch.text;
        read_text_into(path, text).map_err(|e| ProbeError::read(path, e))?;
        parse(text).ok_or_else(|| ProbeError::parse(path, missing))
    }

    pub fn cpu_times(&self) -> ProbeResult<CpuTimes> {
        self.with_root_file(RootFile::Stat, |text| parse_proc_stat(text).0, "no cpu line")
    }

    pub fn memory(&self) -> ProbeResult<MemoryInfo> {
        self.with_root_file(RootFile::Meminfo, parse_meminfo, "no MemTotal/MemFree")
    }
}

fn sysconf(name: libc::c_int) -> Option<u64> {
    // SAFETY: sysconf has no preconditions and only reads a configuration value.
    let value = unsafe { libc::sysconf(name) };
    u64::try_from(value).ok().filter(|&v| v > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_parentheses_and_spaces_in_comm() {
        let line = "4242 (my (weird) app) S 1 4242 4242 0 -1 4194560 1200 0 3 0 250 75 30 12 20 0 7 0 98765 123456789 2560 \
                    18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";
        let stat = parse_stat(line).expect("valid stat line");
        assert_eq!(stat.comm, "my (weird) app");
        assert_eq!(stat.state, ProcessState::Sleeping);
        assert_eq!(stat.ppid, Some(1));
        assert!(!stat.kernel_thread);
        assert_eq!((stat.utime, stat.stime, stat.cutime, stat.cstime), (250, 75, 30, 12));
        assert_eq!(stat.threads, 7);
        assert_eq!(stat.start_ticks, 98_765);
        assert_eq!(stat.rss_pages, 2560);
    }

    #[test]
    fn flags_kernel_threads_and_missing_parent() {
        let line = "2 (kthreadd) S 0 0 0 0 -1 2129984 0 0 0 0 0 1 0 0 20 0 1 0 2 0 0 18446744073709551615 0 0 0 0 0 0 0 \
                    2147483647 0 0 0 0 17 5 0 0 0 0 0";
        let stat = parse_stat(line).expect("valid stat line");
        assert!(stat.kernel_thread);
        assert_eq!(stat.ppid, None);
    }

    #[test]
    fn rejects_truncated_stat() {
        assert_eq!(parse_stat("12 (x) S 1 2 3"), None);
        assert_eq!(parse_stat(""), None);
    }

    #[test]
    fn splits_cmdline_on_nul() {
        assert_eq!(
            parse_cmdline(b"/usr/bin/app\0--flag\0value with space\0"),
            vec!["/usr/bin/app", "--flag", "value with space"]
        );
        assert!(parse_cmdline(b"").is_empty());
    }

    #[test]
    fn parses_io_counters() {
        let io = parse_io("rchar: 100\nwchar: 200\nsyscr: 3\nsyscw: 4\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n");
        assert_eq!(
            io,
            IoCounters {
                rchar: 100,
                wchar: 200,
                read_bytes: 4096,
                written_bytes: 8192,
            }
        );
    }

    #[test]
    fn parses_status_detail() {
        let detail = parse_status(
            "Name:\tapp\nVmHWM:\t   10240 kB\nVmRSS:\t 5120 kB\nThreads:\t4\nvoluntary_ctxt_switches:\t15\nnonvoluntary_ctxt_switches:\t3\n",
        );
        assert_eq!(detail.peak_rss_bytes, 10_240 * 1024);
        assert_eq!(detail.voluntary_ctx_switches, 15);
        assert_eq!(detail.involuntary_ctx_switches, 3);
    }

    #[test]
    fn parses_proc_stat_cpu_lines_and_boot_time() {
        let content = "cpu  100 5 50 1000 20 3 2 1 0 0\ncpu0 60 5 30 500 10 2 1 1 0 0\ncpu1 40 0 20 500 10 1 1 0 0 0\nintr 1 2 3\nbtime 1700000000\n";
        let (times, boot) = parse_proc_stat(content);
        let times = times.expect("cpu line present");
        assert_eq!(times.total, CoreTicks { busy: 161, idle: 1020 });
        assert_eq!(times.cores.len(), 2);
        assert_eq!(times.cores[1], CoreTicks { busy: 62, idle: 510 });
        assert_eq!(boot, Some(1_700_000_000));
    }

    #[test]
    fn derives_used_memory_from_available() {
        let info = parse_meminfo("MemTotal:  1000 kB\nMemFree:  200 kB\nMemAvailable:  600 kB\nSwapTotal: 50 kB\nSwapFree: 30 kB\n")
            .expect("required fields present");
        assert_eq!(info.total_bytes, 1000 * 1024);
        assert_eq!(info.used_bytes, 400 * 1024);
        assert_eq!(info.swap_used_bytes(), 20 * 1024);
    }

    #[test]
    fn converts_ticks_without_float_drift() {
        let fs = ProcFs {
            root: PathBuf::new(),
            clock_ticks_per_sec: 100,
            page_size: 4096,
            boot_time_unix: 0,
            stat_path: PathBuf::new(),
            meminfo_path: PathBuf::new(),
            scratch: RefCell::default(),
        };
        assert_eq!(fs.ticks_to_duration(250), Duration::from_millis(2500));
        assert_eq!(fs.ticks_to_duration(1), Duration::from_millis(10));
    }
}
