use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::{IoCounters, ProcessInterval, ProcessKey, ProcessSample, ProcessSnapshot};

/// How many intervals a reaped child's already-counted CPU waits for its parent's counter to
/// jump.  The jump normally lands in the same interval; a sample straddling the reap moves it
/// to the next one.
const PENDING_FOLD_INTERVALS: u32 = 3;

/// What a set of processes consumed over one interval.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GroupUsage {
    /// CPU time of the members themselves.
    pub cpu_time: Duration,
    /// CPU time of children reaped by members that sampling never saw: processes shorter than
    /// one interval, and the tail between a member's last sample and its exit.  Linux only.
    pub reaped_cpu_time: Duration,
    pub rss_bytes: u64,
    /// Sum of every member's own peak RSS (`VmHWM`), an upper bound for the group's peak.
    /// `None` until status detail was read for at least one member.
    pub peak_rss_bytes_sum: Option<u64>,
    /// Summed over members whose counters were readable.
    pub io: Option<IoCounters>,
    pub inherited_io: Option<IoCounters>,
    pub threads: Option<u32>,
    pub processes: usize,
    pub started: usize,
    pub exited: usize,
}

impl GroupUsage {
    pub fn total_cpu_time(&self) -> Duration {
        self.cpu_time + self.reaped_cpu_time
    }
}

/// Folds per-process deltas of a changing member set into per-interval group totals.
///
/// Reaped children are where naive summing goes wrong: when a sampled member exits and is
/// reaped, its parent's `cutime` jumps by the child's whole lifetime, most of which was already
/// counted from the child's own samples.  Only the remainder is new.
#[derive(Clone, Debug, Default)]
pub struct GroupAccumulator {
    previous_members: HashSet<ProcessKey>,
    /// Already-counted CPU of reaped members, per parent, waiting for the parent's jump.
    pending: HashMap<ProcessKey, (Duration, u32)>,
    /// Counted CPU of members that exited in the last interval with no ancestor left in the
    /// group, whose final totals only their reaper outside the group sees.
    orphaned: HashMap<ProcessKey, Duration>,
}

impl GroupAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// How much of `pid`'s lifetime CPU (own plus reaped children) the group has counted so far,
    /// given its last sample if it is still alive.  `None` for a process that is neither alive
    /// nor exited in the last interval.
    pub fn counted_for(&self, pid: u32, alive: Option<&ProcessSample>) -> Option<Duration> {
        match alive {
            Some(sample) => {
                let pending = self.pending.get(&sample.key()).map_or(Duration::ZERO, |(pending, _)| *pending);
                Some(sample.cpu_time_with_reaped() + pending)
            }
            None => self.orphaned.iter().find(|(key, _)| key.pid == pid).map(|(_, counted)| *counted),
        }
    }

    /// `members` are the group members alive in `later`.
    pub fn add(
        &mut self,
        earlier: &ProcessSnapshot,
        later: &ProcessSnapshot,
        interval: &ProcessInterval,
        members: &HashSet<ProcessKey>,
    ) -> GroupUsage {
        let member_by_pid: HashMap<u32, ProcessKey> = members.iter().map(|k| (k.pid, *k)).collect();

        for (_, age) in self.pending.values_mut() {
            *age += 1;
        }

        let mut usage = GroupUsage {
            exited: interval.exited.iter().filter(|k| self.previous_members.contains(k)).count(),
            ..GroupUsage::default()
        };
        // An exited member's counted CPU waits at its nearest ancestor still alive in the group:
        // when a parent exits in the same interval as its child, the whole chain folds into the
        // grandparent, together with anything the dead parent itself still had pending.
        let earlier_by_pid: HashMap<u32, ProcessKey> = earlier.processes.keys().map(|k| (k.pid, *k)).collect();
        self.orphaned.clear();
        for key in &interval.exited {
            if !self.previous_members.contains(key) {
                continue;
            }
            let child = &earlier.processes[key];
            let carried = self.pending.remove(key).map_or(Duration::ZERO, |(pending, _)| pending);
            let counted = child.cpu_time_with_reaped() + carried;
            match surviving_ancestor(*key, earlier, &earlier_by_pid, &member_by_pid, &self.previous_members) {
                Ancestor::Alive(ancestor) => {
                    let entry = self.pending.entry(ancestor).or_insert((Duration::ZERO, 0));
                    entry.0 += counted;
                    entry.1 = 0;
                }
                Ancestor::TopExited(top) => *self.orphaned.entry(top).or_default() += counted,
            }
        }

        self.pending
            .retain(|parent, (_, age)| *age <= PENDING_FOLD_INTERVALS && members.contains(parent));

        let mut io: Option<IoCounters> = None;
        let mut inherited_io: Option<IoCounters> = None;
        for key in members {
            let (Some(process), Some(delta)) = (later.processes.get(key), interval.deltas.get(key)) else {
                continue;
            };
            usage.processes += 1;
            usage.started += usize::from(!self.previous_members.contains(key) && delta.started);
            usage.cpu_time += delta.cpu_time;
            usage.rss_bytes += process.rss_bytes;
            if let Some(threads) = process.threads {
                *usage.threads.get_or_insert(0) += threads;
            }
            if let Some(status) = process.status {
                *usage.peak_rss_bytes_sum.get_or_insert(0) += status.peak_rss_bytes;
            }
            if let Some(delta_io) = delta.io {
                io = Some(io.unwrap_or_default().saturating_add(delta_io));
            }
            if let Some(delta_inherited) = delta.inherited_io {
                inherited_io = Some(inherited_io.unwrap_or_default().saturating_add(delta_inherited));
            }
            if let Some(mut reaped) = delta.reaped_children_cpu_time {
                if let Some((pending, _)) = self.pending.get_mut(key) {
                    let absorbed = reaped.min(*pending);
                    reaped -= absorbed;
                    *pending -= absorbed;
                }
                usage.reaped_cpu_time += reaped;
            }
        }
        self.pending.retain(|_, (pending, _)| !pending.is_zero());
        usage.io = io;
        usage.inherited_io = inherited_io;
        self.previous_members.clone_from(members);
        usage
    }
}

enum Ancestor {
    /// The nearest ancestor that is a member alive now.
    Alive(ProcessKey),
    /// No member up the chain survived; the highest member that exited, whose reaper outside
    /// the group received everything below it.
    TopExited(ProcessKey),
}

fn surviving_ancestor(
    exited: ProcessKey,
    earlier: &ProcessSnapshot,
    earlier_by_pid: &HashMap<u32, ProcessKey>,
    member_by_pid: &HashMap<u32, ProcessKey>,
    previous_members: &HashSet<ProcessKey>,
) -> Ancestor {
    let mut top = exited;
    let mut ppid = earlier.processes[&exited].ppid;
    for _ in 0..64 {
        let Some(pid) = ppid else { break };
        if let Some(member) = member_by_pid.get(&pid) {
            return Ancestor::Alive(*member);
        }
        let Some(parent) = earlier_by_pid.get(&pid).filter(|key| previous_members.contains(key)) else {
            break;
        };
        top = *parent;
        ppid = earlier.processes[parent].ppid;
    }
    Ancestor::TopExited(top)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::process::{ProcessInfo, ProcessSample, ProcessState};

    fn sample(pid: u32, ppid: u32, cpu_ms: u64, reaped_ms: u64) -> ProcessSample {
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
            rss_bytes: 1 << 20,
            threads: Some(1),
            io: None,
            status: None,
        }
    }

    fn snapshot(at: Instant, processes: Vec<ProcessSample>) -> ProcessSnapshot {
        ProcessSnapshot {
            taken_at: at,
            unix_time: 0.0,
            logical_cores: 1,
            processes: processes.into_iter().map(|p| (p.key(), p)).collect(),
        }
    }

    fn keys(snapshot: &ProcessSnapshot) -> HashSet<ProcessKey> {
        snapshot.processes.keys().copied().collect()
    }

    fn step(acc: &mut GroupAccumulator, earlier: &ProcessSnapshot, later: &ProcessSnapshot) -> GroupUsage {
        let interval = ProcessInterval::between(earlier, later);
        acc.add(earlier, later, &interval, &keys(later))
    }

    #[test]
    fn counts_only_the_unsampled_tail_of_a_reaped_member() {
        let t = Instant::now();
        let s = |secs| t + Duration::from_secs(secs);
        let mut acc = GroupAccumulator::new();
        let first = snapshot(s(0), vec![sample(10, 1, 100, 0), sample(11, 10, 200, 0)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));

        // The child used 300 ms more and was then reaped, so the parent's cutime jumps by its
        // lifetime total of 500 ms - of which 200 ms were already counted from its samples.
        let second = snapshot(s(1), vec![sample(10, 1, 150, 500)]);
        let usage = step(&mut acc, &first, &second);
        assert_eq!(usage.cpu_time, Duration::from_millis(50));
        assert_eq!(usage.reaped_cpu_time, Duration::from_millis(300));
        assert_eq!(usage.exited, 1);
    }

    #[test]
    fn counts_children_too_short_to_sample_in_full() {
        let t = Instant::now();
        let mut acc = GroupAccumulator::new();
        let first = snapshot(t, vec![sample(10, 1, 100, 0)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));
        let second = snapshot(t + Duration::from_secs(1), vec![sample(10, 1, 100, 70)]);
        assert_eq!(step(&mut acc, &first, &second).reaped_cpu_time, Duration::from_millis(70));
    }

    #[test]
    fn a_fold_landing_one_sample_late_is_not_double_counted() {
        let t = Instant::now();
        let s = |secs| t + Duration::from_secs(secs);
        let mut acc = GroupAccumulator::new();
        let first = snapshot(s(0), vec![sample(10, 1, 0, 0), sample(11, 10, 400, 0)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));

        let second = snapshot(s(1), vec![sample(10, 1, 0, 0)]);
        assert_eq!(step(&mut acc, &first, &second).reaped_cpu_time, Duration::ZERO);
        let third = snapshot(s(2), vec![sample(10, 1, 0, 450)]);
        assert_eq!(step(&mut acc, &second, &third).reaped_cpu_time, Duration::from_millis(50));
    }

    #[test]
    fn a_chain_exiting_together_folds_into_the_grandparent_once() {
        let t = Instant::now();
        let s = |secs| t + Duration::from_secs(secs);
        let mut acc = GroupAccumulator::new();
        // sh -> timeout -> busy loop; the loop is killed, timeout reaps it and exits, and sh
        // reaps timeout - all between two samples.
        let first = snapshot(s(0), vec![sample(10, 1, 5, 0), sample(11, 10, 1, 0), sample(12, 11, 900, 0)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));
        let second = snapshot(s(1), vec![sample(10, 1, 5, 1000)]);
        let usage = step(&mut acc, &first, &second);
        assert_eq!(usage.reaped_cpu_time, Duration::from_millis(99), "only the unsampled tail is new");
        assert_eq!(usage.exited, 2);
        let root = ProcessKey { pid: 10, start: 10 };
        assert_eq!(acc.counted_for(10, second.get(&root)), Some(Duration::from_millis(1005)));
    }

    #[test]
    fn remembers_what_was_counted_for_a_member_exiting_without_ancestor() {
        let t = Instant::now();
        let mut acc = GroupAccumulator::new();
        let first = snapshot(t, vec![sample(10, 1, 300, 50)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));
        let second = snapshot(t + Duration::from_secs(1), vec![]);
        step(&mut acc, &first, &second);
        assert_eq!(acc.counted_for(10, None), Some(Duration::from_millis(350)));
    }

    #[test]
    fn a_whole_tree_exiting_at_once_is_counted_at_its_root() {
        let t = Instant::now();
        let mut acc = GroupAccumulator::new();
        let first = snapshot(t, vec![sample(10, 1, 100, 0), sample(11, 10, 50, 0), sample(12, 11, 2000, 0)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));
        step(&mut acc, &first, &snapshot(t + Duration::from_secs(1), vec![]));
        assert_eq!(acc.counted_for(10, None), Some(Duration::from_millis(2150)));
    }

    #[test]
    fn sums_members_and_counts_new_ones() {
        let t = Instant::now();
        let mut acc = GroupAccumulator::new();
        let first = snapshot(t, vec![sample(10, 1, 100, 0)]);
        acc.add(&first, &first, &ProcessInterval::between(&first, &first), &keys(&first));
        let second = snapshot(t + Duration::from_secs(1), vec![sample(10, 1, 200, 0), sample(12, 10, 30, 0)]);
        let usage = step(&mut acc, &first, &second);
        assert_eq!(usage.cpu_time, Duration::from_millis(130));
        assert_eq!(usage.processes, 2);
        assert_eq!(usage.started, 1);
        assert_eq!(usage.rss_bytes, 2 << 20);
        assert_eq!(usage.threads, Some(2));
    }
}
