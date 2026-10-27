mod delta;
mod group;
mod sampler;
mod selector;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use delta::{ProcessDelta, ProcessInterval};
pub use group::{GroupAccumulator, GroupUsage};
pub use sampler::{DetailRefresh, ProcessDetail, ProcessSampler, ProcessSamplerConfig, ProcessSnapshot};
pub use selector::{MatchField, MatchMode, Pattern, Selection, Selector};

/// Identity that survives PID reuse: a reused PID comes with a different start stamp.
///
/// `start` is opaque and only comparable between keys from the same backend - clock ticks
/// since boot on Linux, unix seconds elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessKey {
    pub pid: u32,
    pub start: u64,
}

/// What does not change while a process lives, read once when it is first seen and again
/// after an `exec`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessInfo {
    pub key: ProcessKey,
    pub name: String,
    /// Empty for kernel threads and for processes whose cmdline could not be read.
    pub cmd: Vec<String>,
    pub exe: Option<PathBuf>,
    pub uid: Option<u32>,
    pub kernel_thread: bool,
    pub start_unix: f64,
}

impl ProcessInfo {
    pub fn cmd_line(&self) -> String {
        self.cmd.join(" ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ProcessState {
    Running,
    Sleeping,
    DiskSleep,
    Zombie,
    Stopped,
    Tracing,
    Dead,
    Idle,
    Unknown,
}

impl ProcessState {
    /// The letter `ps` and `/proc/<pid>/stat` use.
    pub fn as_char(self) -> char {
        match self {
            Self::Running => 'R',
            Self::Sleeping => 'S',
            Self::DiskSleep => 'D',
            Self::Zombie => 'Z',
            Self::Stopped => 'T',
            Self::Tracing => 't',
            Self::Dead => 'X',
            Self::Idle => 'I',
            Self::Unknown => '?',
        }
    }

    pub fn from_char(c: char) -> Self {
        match c {
            'R' => Self::Running,
            'S' => Self::Sleeping,
            'D' => Self::DiskSleep,
            'Z' => Self::Zombie,
            'T' => Self::Stopped,
            't' => Self::Tracing,
            'X' | 'x' => Self::Dead,
            'I' => Self::Idle,
            _ => Self::Unknown,
        }
    }
}

/// Cumulative `/proc/<pid>/io` counters.
///
/// `rchar`/`wchar` are syscall level and charged to the caller of `read()`/`write()`;
/// `read_bytes`/`written_bytes` reached the block device and are charged to whoever submitted
/// the I/O - for buffered writes that is a kernel flush thread rather than the writer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IoCounters {
    pub rchar: u64,
    pub wchar: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

impl IoCounters {
    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            rchar: self.rchar.saturating_sub(other.rchar),
            wchar: self.wchar.saturating_sub(other.wchar),
            read_bytes: self.read_bytes.saturating_sub(other.read_bytes),
            written_bytes: self.written_bytes.saturating_sub(other.written_bytes),
        }
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            rchar: self.rchar.saturating_add(other.rchar),
            wchar: self.wchar.saturating_add(other.wchar),
            read_bytes: self.read_bytes.saturating_add(other.read_bytes),
            written_bytes: self.written_bytes.saturating_add(other.written_bytes),
        }
    }

    /// Field-wise minimum - how much of `other` this delta can actually account for.
    pub fn overlap(self, other: Self) -> Self {
        Self {
            rchar: self.rchar.min(other.rchar),
            wchar: self.wchar.min(other.wchar),
            read_bytes: self.read_bytes.min(other.read_bytes),
            written_bytes: self.written_bytes.min(other.written_bytes),
        }
    }

    pub fn is_zero(self) -> bool {
        self == Self::default()
    }
}

/// Fields from `/proc/<pid>/status`, the largest file read per process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StatusDetail {
    /// `VmHWM` - the highest RSS the process ever had.
    pub peak_rss_bytes: u64,
    pub voluntary_ctx_switches: u64,
    pub involuntary_ctx_switches: u64,
}

/// One process as seen by one sample.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessSample {
    pub info: Arc<ProcessInfo>,
    /// Changes when the parent dies and the process is reparented.
    pub ppid: Option<u32>,
    pub state: ProcessState,
    /// User plus system time of the process itself.
    pub cpu_time: Duration,
    /// User plus system time of children it has reaped (`cutime + cstime`), Linux only.
    pub reaped_children_cpu_time: Option<Duration>,
    pub rss_bytes: u64,
    pub threads: Option<u32>,
    /// `None` when not requested, or when `/proc/<pid>/io` is unreadable (mode 0400, so other
    /// users' processes need privilege) - unknown, not zero.
    pub io: Option<IoCounters>,
    pub status: Option<StatusDetail>,
}

impl ProcessSample {
    pub fn key(&self) -> ProcessKey {
        self.info.key
    }

    pub fn pid(&self) -> u32 {
        self.info.key.pid
    }

    /// Own CPU time plus reaped children, i.e. everything this process and its finished
    /// descendants have consumed.
    pub fn cpu_time_with_reaped(&self) -> Duration {
        self.cpu_time + self.reaped_children_cpu_time.unwrap_or_default()
    }
}
