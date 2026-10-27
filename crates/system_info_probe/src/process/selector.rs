use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{ProcessInfo, ProcessKey, ProcessSample, ProcessSnapshot};

/// Which processes something applies to.  Turned into a [`Selection`] to be evaluated against
/// snapshots, which is where processes appearing later get picked up.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Selector {
    All,
    /// Exactly this process.  Bound to the first process seen with that PID, so a later process
    /// reusing the PID is not mistaken for it.
    Pid(u32),
    /// The process and every descendant, including ones started later.  Descendants stay members
    /// after being reparented by the death of their parent.
    Tree(u32),
    /// Every process matching, including ones started later.
    Pattern(Pattern),
    Any(Vec<Self>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pattern {
    pub field: MatchField,
    pub mode: MatchMode,
    pub text: String,
}

impl Pattern {
    pub fn cmd_contains(text: impl Into<String>) -> Self {
        Self {
            field: MatchField::Cmd,
            mode: MatchMode::Contains,
            text: text.into(),
        }
    }

    pub fn name_is(text: impl Into<String>) -> Self {
        Self {
            field: MatchField::Name,
            mode: MatchMode::Exact,
            text: text.into(),
        }
    }

    pub fn matches(&self, info: &ProcessInfo) -> bool {
        let check = |value: &str| match self.mode {
            MatchMode::Contains => value.contains(&self.text),
            MatchMode::Exact => value == self.text,
            MatchMode::Prefix => value.starts_with(&self.text),
        };
        match self.field {
            MatchField::Name => check(&info.name),
            MatchField::Cmd => !info.cmd.is_empty() && check(&info.cmd_line()),
            MatchField::Exe => info.exe.as_ref().is_some_and(|exe| check(&exe.to_string_lossy())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MatchField {
    /// The kernel's short name (`comm`, at most 15 bytes on Linux).
    Name,
    /// Arguments joined with spaces.  Kernel threads have none and never match.
    Cmd,
    Exe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MatchMode {
    Contains,
    Exact,
    Prefix,
}

/// A [`Selector`] being tracked across snapshots.
#[derive(Clone, Debug)]
pub struct Selection {
    selector: Selector,
    node: Node,
}

#[derive(Clone, Debug)]
enum Node {
    All,
    Pid {
        pid: u32,
        bound: Option<ProcessKey>,
    },
    Tree {
        pid: u32,
        root: Option<ProcessKey>,
        members: HashSet<ProcessKey>,
    },
    Pattern {
        pattern: Pattern,
        checked: HashMap<ProcessKey, (Arc<ProcessInfo>, bool)>,
    },
    Any(Vec<Self>),
}

impl Selection {
    pub fn new(selector: Selector) -> Self {
        Self {
            node: Node::from(&selector),
            selector,
        }
    }

    pub fn selector(&self) -> &Selector {
        &self.selector
    }

    /// Members alive in `snapshot`.  Snapshots must be passed in the order they were taken.
    pub fn update(&mut self, snapshot: &ProcessSnapshot) -> HashSet<ProcessKey> {
        let mut out = HashSet::new();
        self.node.update(snapshot, &mut out);
        out
    }
}

impl Node {
    fn from(selector: &Selector) -> Self {
        match selector {
            Selector::All => Self::All,
            Selector::Pid(pid) => Self::Pid { pid: *pid, bound: None },
            Selector::Tree(pid) => Self::Tree {
                pid: *pid,
                root: None,
                members: HashSet::new(),
            },
            Selector::Pattern(pattern) => Self::Pattern {
                pattern: pattern.clone(),
                checked: HashMap::new(),
            },
            Selector::Any(selectors) => Self::Any(selectors.iter().map(Self::from).collect()),
        }
    }

    fn update(&mut self, snapshot: &ProcessSnapshot, out: &mut HashSet<ProcessKey>) {
        match self {
            Self::All => out.extend(snapshot.processes.keys().copied()),
            Self::Pid { pid, bound } => {
                if bound.is_none() {
                    *bound = snapshot.by_pid(*pid).map(ProcessSample::key);
                }
                if let Some(key) = bound
                    && snapshot.processes.contains_key(key)
                {
                    out.insert(*key);
                }
            }
            Self::Tree { pid, root, members } => {
                if root.is_none() {
                    *root = snapshot.by_pid(*pid).map(|p| p.key());
                }
                let Some(root) = *root else { return };
                members.retain(|key| snapshot.processes.contains_key(key));
                if snapshot.processes.contains_key(&root) {
                    members.insert(root);
                }
                // Parents always start before their children, so walking in start order lets a
                // grandchild that appeared together with its parent join in the same pass.
                let mut candidates: Vec<_> = snapshot.iter().filter(|p| !members.contains(&p.key())).collect();
                candidates.sort_by_key(|p| p.info.key.start);
                let mut member_pids: HashSet<u32> = members.iter().map(|k| k.pid).collect();
                for process in candidates {
                    if process.ppid.is_some_and(|ppid| member_pids.contains(&ppid)) {
                        members.insert(process.key());
                        member_pids.insert(process.pid());
                    }
                }
                out.extend(members.iter().copied());
            }
            Self::Pattern { pattern, checked } => {
                checked.retain(|key, _| snapshot.processes.contains_key(key));
                for process in snapshot.iter() {
                    let key = process.key();
                    let matched = match checked.get(&key) {
                        Some((info, matched)) if Arc::ptr_eq(info, &process.info) => *matched,
                        _ => {
                            let matched = pattern.matches(&process.info);
                            checked.insert(key, (Arc::clone(&process.info), matched));
                            matched
                        }
                    };
                    if matched {
                        out.insert(key);
                    }
                }
            }
            Self::Any(nodes) => {
                for node in nodes {
                    node.update(snapshot, out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::process::ProcessState;

    fn process(pid: u32, ppid: u32, start: u64, name: &str, cmd: &[&str]) -> ProcessSample {
        ProcessSample {
            info: Arc::new(ProcessInfo {
                key: ProcessKey { pid, start },
                name: name.to_string(),
                cmd: cmd.iter().map(ToString::to_string).collect(),
                exe: None,
                uid: None,
                kernel_thread: false,
                start_unix: start as f64,
            }),
            ppid: (ppid != 0).then_some(ppid),
            state: ProcessState::Sleeping,
            cpu_time: Duration::ZERO,
            reaped_children_cpu_time: None,
            rss_bytes: 0,
            threads: None,
            io: None,
            status: None,
        }
    }

    fn snapshot(processes: Vec<ProcessSample>) -> ProcessSnapshot {
        ProcessSnapshot {
            taken_at: Instant::now(),
            unix_time: 0.0,
            logical_cores: 1,
            processes: processes.into_iter().map(|p| (p.key(), p)).collect::<BTreeMap<_, _>>(),
        }
    }

    fn pids(keys: &HashSet<ProcessKey>) -> Vec<u32> {
        let mut pids: Vec<u32> = keys.iter().map(|k| k.pid).collect();
        pids.sort_unstable();
        pids
    }

    #[test]
    fn tree_picks_up_children_and_grandchildren_started_later() {
        let mut selection = Selection::new(Selector::Tree(10));
        let first = snapshot(vec![process(1, 0, 1, "init", &[]), process(10, 1, 5, "app", &["app"])]);
        assert_eq!(pids(&selection.update(&first)), vec![10]);

        let second = snapshot(vec![
            process(1, 0, 1, "init", &[]),
            process(10, 1, 5, "app", &["app"]),
            process(11, 10, 7, "worker", &["worker"]),
            process(12, 11, 8, "helper", &["helper"]),
            process(13, 1, 9, "other", &["other"]),
        ]);
        assert_eq!(pids(&selection.update(&second)), vec![10, 11, 12]);
    }

    #[test]
    fn tree_keeps_orphans_after_the_root_exits() {
        let mut selection = Selection::new(Selector::Tree(10));
        selection.update(&snapshot(vec![process(10, 1, 5, "app", &[]), process(11, 10, 7, "worker", &[])]));
        let orphaned = snapshot(vec![process(1, 0, 1, "init", &[]), process(11, 1, 7, "worker", &[])]);
        assert_eq!(pids(&selection.update(&orphaned)), vec![11]);
    }

    #[test]
    fn pid_is_not_confused_by_reuse() {
        let mut selection = Selection::new(Selector::Pid(10));
        assert_eq!(pids(&selection.update(&snapshot(vec![process(10, 1, 5, "app", &[])]))), vec![10]);
        let reused = snapshot(vec![process(10, 1, 99, "stranger", &[])]);
        assert!(selection.update(&reused).is_empty());
    }

    #[test]
    fn pid_binds_once_it_appears() {
        let mut selection = Selection::new(Selector::Pid(10));
        assert!(selection.update(&snapshot(vec![process(1, 0, 1, "init", &[])])).is_empty());
        assert_eq!(pids(&selection.update(&snapshot(vec![process(10, 1, 5, "app", &[])]))), vec![10]);
    }

    #[test]
    fn pattern_matches_every_process_including_new_ones() {
        let mut selection = Selection::new(Selector::Pattern(Pattern::cmd_contains("--render")));
        let first = snapshot(vec![process(20, 1, 3, "app", &["app", "--render"]), process(21, 1, 3, "app", &["app"])]);
        assert_eq!(pids(&selection.update(&first)), vec![20]);
        let second = snapshot(vec![
            process(20, 1, 3, "app", &["app", "--render"]),
            process(21, 1, 3, "app", &["app"]),
            process(22, 1, 9, "app", &["app", "--render", "2"]),
        ]);
        assert_eq!(pids(&selection.update(&second)), vec![20, 22]);
    }

    #[test]
    fn pattern_re_evaluates_after_exec() {
        let mut selection = Selection::new(Selector::Pattern(Pattern::name_is("target")));
        assert!(selection.update(&snapshot(vec![process(30, 1, 3, "sh", &["sh"])])).is_empty());
        let after_exec = snapshot(vec![process(30, 1, 3, "target", &["target"])]);
        assert_eq!(pids(&selection.update(&after_exec)), vec![30]);
    }

    #[test]
    fn kernel_threads_never_match_on_cmd() {
        let mut kthread = process(2, 0, 1, "kworker/0:1", &[]);
        Arc::make_mut(&mut kthread.info).kernel_thread = true;
        assert!(!Pattern::cmd_contains("").matches(&kthread.info));
    }

    #[test]
    fn any_is_the_union() {
        let mut selection = Selection::new(Selector::Any(vec![Selector::Pid(1), Selector::Pattern(Pattern::name_is("b"))]));
        let snap = snapshot(vec![process(1, 0, 1, "a", &[]), process(2, 1, 2, "b", &[]), process(3, 1, 3, "c", &[])]);
        assert_eq!(pids(&selection.update(&snap)), vec![1, 2]);
    }
}
