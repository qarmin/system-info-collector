use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use super::{IoCounters, ProcessKey, ProcessSnapshot};

/// What one process did between two snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessDelta {
    pub cpu_time: Duration,
    /// CPU time of children reaped during the interval, Linux only.
    pub reaped_children_cpu_time: Option<Duration>,
    /// The process's own I/O, with what reaped children folded into its counters removed.
    /// `None` when the counters were not readable in both snapshots.
    pub io: Option<IoCounters>,
    /// I/O the kernel folded in from children reaped during the interval.
    ///
    /// Linux adds a reaped child's lifetime I/O to its parent's counters in one jump; left in,
    /// every shell and service manager would be blamed for what its children wrote.  It is also
    /// how a process spawning writers too short-lived to sample gives itself away.
    pub inherited_io: Option<IoCounters>,
    /// The process did not exist in the earlier snapshot, so everything it consumed so far
    /// happened within the interval.
    pub started: bool,
}

#[derive(Clone, Debug)]
pub struct ProcessInterval {
    pub elapsed: Duration,
    pub logical_cores: usize,
    pub deltas: BTreeMap<ProcessKey, ProcessDelta>,
    pub started: Vec<ProcessKey>,
    /// Present in the earlier snapshot only; their last values are in that snapshot.
    pub exited: Vec<ProcessKey>,
}

impl ProcessInterval {
    pub fn between(earlier: &ProcessSnapshot, later: &ProcessSnapshot) -> Self {
        let exited: Vec<ProcessKey> = earlier.processes.keys().filter(|k| !later.processes.contains_key(k)).copied().collect();

        let mut folded: HashMap<u32, IoCounters> = HashMap::new();
        for key in &exited {
            let child = &earlier.processes[key];
            if let (Some(parent), Some(io)) = (child.ppid, child.io) {
                let entry = folded.entry(parent).or_default();
                *entry = entry.saturating_add(io);
            }
        }

        let mut started = Vec::new();
        let mut deltas = BTreeMap::new();
        for (key, now) in &later.processes {
            let before = earlier.processes.get(key);
            if before.is_none() {
                started.push(*key);
            }
            let cpu_time = now.cpu_time.saturating_sub(before.map_or(Duration::ZERO, |b| b.cpu_time));
            let reaped_children_cpu_time = match (now.reaped_children_cpu_time, before) {
                (Some(now), Some(before)) => before.reaped_children_cpu_time.map(|b| now.saturating_sub(b)),
                (Some(now), None) => Some(now),
                (None, _) => None,
            };
            let raw_io = match (now.io, before) {
                (Some(now), Some(before)) => before.io.map(|b| now.saturating_sub(b)),
                (Some(now), None) => Some(now),
                (None, _) => None,
            };
            // Never let the correction go negative: the fold can land a sample after the child
            // vanished, and the parent may also have done I/O itself.
            let fold = folded.get(&key.pid).copied().unwrap_or_default();
            deltas.insert(
                *key,
                ProcessDelta {
                    cpu_time,
                    reaped_children_cpu_time,
                    io: raw_io.map(|raw| raw.saturating_sub(fold)),
                    inherited_io: raw_io.map(|raw| raw.overlap(fold)),
                    started: before.is_none(),
                },
            );
        }

        Self {
            elapsed: later.taken_at.saturating_duration_since(earlier.taken_at),
            logical_cores: later.logical_cores,
            deltas,
            started,
            exited,
        }
    }

    pub fn get(&self, key: &ProcessKey) -> Option<&ProcessDelta> {
        self.deltas.get(key)
    }

    /// 100 means one core fully busy, like `top`.
    pub fn cpu_percent_of_core(&self, cpu_time: Duration) -> f64 {
        let elapsed = self.elapsed.as_secs_f64();
        if elapsed <= 0.0 {
            return 0.0;
        }
        cpu_time.as_secs_f64() * 100.0 / elapsed
    }

    /// 100 means every core fully busy.
    pub fn cpu_percent_of_machine(&self, cpu_time: Duration) -> f64 {
        self.cpu_percent_of_core(cpu_time) / self.logical_cores.max(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::process::{ProcessInfo, ProcessSample, ProcessState};

    fn sample(pid: u32, ppid: u32, cpu_ms: u64, reaped_ms: u64, io: Option<IoCounters>) -> ProcessSample {
        ProcessSample {
            info: Arc::new(ProcessInfo {
                key: ProcessKey { pid, start: u64::from(pid) },
                name: format!("p{pid}"),
                cmd: vec![],
                exe: None,
                uid: None,
                kernel_thread: false,
                start_unix: 0.0,
            }),
            ppid: Some(ppid),
            state: ProcessState::Sleeping,
            cpu_time: Duration::from_millis(cpu_ms),
            reaped_children_cpu_time: Some(Duration::from_millis(reaped_ms)),
            rss_bytes: 0,
            threads: None,
            io,
            status: None,
        }
    }

    fn snapshot(at: Instant, processes: Vec<ProcessSample>) -> ProcessSnapshot {
        ProcessSnapshot {
            taken_at: at,
            unix_time: 0.0,
            logical_cores: 4,
            processes: processes.into_iter().map(|p| (p.key(), p)).collect(),
        }
    }

    fn wchar(bytes: u64) -> IoCounters {
        IoCounters {
            wchar: bytes,
            ..IoCounters::default()
        }
    }

    #[test]
    fn reports_lifecycle_and_full_counts_for_new_processes() {
        let start = Instant::now();
        let earlier = snapshot(start, vec![sample(1, 0, 1000, 0, None), sample(2, 1, 500, 0, None)]);
        let later = snapshot(
            start + Duration::from_secs(1),
            vec![sample(1, 0, 1250, 0, None), sample(3, 1, 40, 0, None)],
        );
        let interval = ProcessInterval::between(&earlier, &later);

        assert_eq!(interval.exited.iter().map(|k| k.pid).collect::<Vec<_>>(), vec![2]);
        assert_eq!(interval.started.iter().map(|k| k.pid).collect::<Vec<_>>(), vec![3]);
        let first = interval.deltas.values().next().expect("pid 1 present");
        assert_eq!(first.cpu_time, Duration::from_millis(250));
        assert!((interval.cpu_percent_of_core(first.cpu_time) - 25.0).abs() < 1e-9);
        assert!((interval.cpu_percent_of_machine(first.cpu_time) - 6.25).abs() < 1e-9);
        let new = interval.deltas.values().nth(1).expect("pid 3 present");
        assert!(new.started);
        assert_eq!(new.cpu_time, Duration::from_millis(40));
    }

    #[test]
    fn credits_a_reaped_childs_io_to_the_child_not_the_parent() {
        let start = Instant::now();
        let own = 1 << 20;
        let child_total = 8 << 20;
        let earlier = snapshot(
            start,
            vec![sample(1, 0, 0, 0, Some(wchar(0))), sample(2, 1, 0, 0, Some(wchar(child_total)))],
        );
        let later = snapshot(start + Duration::from_secs(1), vec![sample(1, 0, 0, 0, Some(wchar(own + child_total)))]);
        let delta = ProcessInterval::between(&earlier, &later)
            .deltas
            .values()
            .next()
            .copied()
            .expect("parent present");
        assert_eq!(delta.io.map(|io| io.wchar), Some(own));
        assert_eq!(delta.inherited_io.map(|io| io.wchar), Some(child_total));
    }

    #[test]
    fn never_reports_negative_own_io_when_the_fold_lands_late() {
        let start = Instant::now();
        let earlier = snapshot(start, vec![sample(1, 0, 0, 0, Some(wchar(0))), sample(2, 1, 0, 0, Some(wchar(8 << 20)))]);
        let later = snapshot(start + Duration::from_secs(1), vec![sample(1, 0, 0, 0, Some(wchar(0)))]);
        let delta = ProcessInterval::between(&earlier, &later)
            .deltas
            .values()
            .next()
            .copied()
            .expect("parent present");
        assert_eq!(delta.io.map(|io| io.wchar), Some(0));
        assert_eq!(delta.inherited_io.map(|io| io.wchar), Some(0));
    }

    #[test]
    fn io_unreadable_in_either_snapshot_is_unknown() {
        let start = Instant::now();
        let earlier = snapshot(start, vec![sample(1, 0, 0, 0, None)]);
        let later = snapshot(start + Duration::from_secs(1), vec![sample(1, 0, 0, 0, Some(wchar(10)))]);
        let delta = ProcessInterval::between(&earlier, &later)
            .deltas
            .values()
            .next()
            .copied()
            .expect("present");
        assert_eq!(delta.io, None);
    }
}
