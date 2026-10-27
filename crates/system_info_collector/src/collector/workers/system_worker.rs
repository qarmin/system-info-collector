use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use system_info_probe::process::{
    Coverage, DetailRefresh, Pattern, ProcessDetail, ProcessInterval, ProcessKey, ProcessSampler, ProcessSamplerConfig, ProcessSnapshot, Selection,
    Selector,
};
use system_info_probe::system::{SystemSample, SystemSampler};

use crate::collector::settings::CollectSettings;
use crate::collector::shared_state::{ProcessSnapshot as TrackedProcess, SharedState, SysinfoSnapshot};

/// Machine CPU and memory, and the processes matching each `--process-cmd-to-search` pattern -
/// every matching process, summed, including ones started later.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>) {
    let mut system = match SystemSampler::new() {
        Ok(sampler) => sampler,
        Err(e) => {
            warn!("CPU and memory unavailable ({e}), system_worker stopped");
            return;
        }
    };
    let mut tracker = ProcessTracker::new(&settings);
    let mut previous_system = system.sample().ok();

    let interval_ms = (settings.sysinfo_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    interval.tick().await;

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let start = Instant::now();
        let current = system.sample().inspect_err(|e| warn!("Reading CPU and memory failed: {e}")).ok();
        let snapshot = match (&previous_system, &current) {
            (Some(before), Some(now)) => Some(system_snapshot(before, now)),
            _ => None,
        };
        let processes = tracker.as_mut().map(ProcessTracker::sample).unwrap_or_default();
        debug!("system_worker sampled in {:?}", start.elapsed());

        {
            let mut guard = state.write().expect("SharedState RwLock poisoned");
            guard.latest_sysinfo = snapshot;
            guard.latest_processes = processes;
        }
        previous_system = current;
    }

    info!("system_worker stopped");
}

fn system_snapshot(before: &SystemSample, now: &SystemSample) -> SysinfoSnapshot {
    let usage = now.cpu_usage_since(before).unwrap_or_default();
    let memory = now.memory;
    SysinfoSnapshot {
        cpu_usage_total: f64::from(usage.total_percent),
        cpu_usage_per_core: usage.per_core_percent.iter().map(|c| f64::from(*c)).collect(),
        memory_used_mb: bytes_to_mb(memory.used_bytes),
        memory_free_mb: bytes_to_mb(memory.free_bytes),
        memory_available_mb: bytes_to_mb(memory.available_bytes),
        swap_used_mb: bytes_to_mb(memory.swap_used_bytes()),
        swap_free_mb: bytes_to_mb(memory.swap_free_bytes),
    }
}

/// Reads only the matching processes and newcomers each tick - everything else is checked for
/// presence by listing `/proc`.
struct ProcessTracker {
    sampler: ProcessSampler,
    patterns: Vec<(String, Selection)>,
    previous: Option<ProcessSnapshot>,
    matched: Vec<HashSet<ProcessKey>>,
}

impl ProcessTracker {
    fn new(settings: &CollectSettings) -> Option<Self> {
        if settings.process_cmd_to_search.is_empty() {
            return None;
        }
        let selectors: Vec<Selector> = settings
            .process_cmd_to_search
            .iter()
            .map(|search| Selector::Pattern(Pattern::cmd_contains(&search.search_text)))
            .collect();
        let sampler = ProcessSampler::new(ProcessSamplerConfig {
            detail: ProcessDetail::Basic,
            detail_scope: Some(Selection::new(Selector::Any(selectors.clone()))),
            refresh: DetailRefresh::default(),
            coverage: Coverage::Scope,
        })
        .inspect_err(|e| warn!("Process tracking unavailable ({e})"))
        .ok()?;
        Some(Self {
            sampler,
            patterns: settings
                .process_cmd_to_search
                .iter()
                .map(|search| search.graph_name.clone())
                .zip(selectors.into_iter().map(Selection::new))
                .collect(),
            previous: None,
            matched: vec![HashSet::new(); settings.process_cmd_to_search.len()],
        })
    }

    fn sample(&mut self) -> Vec<Option<TrackedProcess>> {
        let current = match self.sampler.sample() {
            Ok(snapshot) => snapshot,
            Err(e) => {
                warn!("Process tracking error: {e}");
                return vec![None; self.patterns.len()];
            }
        };
        let launchers = own_lineage(&current);
        let changes = self.previous.as_ref().map(|previous| ProcessInterval::between(previous, &current));
        let mut out = Vec::with_capacity(self.patterns.len());
        for ((name, selection), matched) in self.patterns.iter_mut().zip(&mut self.matched) {
            let members: HashSet<ProcessKey> = selection
                .update(&current)
                .into_iter()
                .filter(|key| !launchers.contains(&key.pid))
                .collect();
            log_changes(name, matched, &members, &current);
            *matched = members;
            if matched.is_empty() {
                out.push(None);
                continue;
            }
            let mut keys: Vec<&ProcessKey> = matched.iter().collect();
            keys.sort_unstable_by_key(|key| (key.start, key.pid));
            let cpu = changes.as_ref().map_or(0.0, |c| {
                keys.iter()
                    .filter_map(|key| c.get(key))
                    .map(|delta| c.cpu_percent_of_machine(delta.cpu_time))
                    .sum::<f64>()
            });
            out.push(Some(TrackedProcess {
                cpu_usage: cpu as f32,
                memory_mb: keys.iter().map(|key| bytes_to_mb(current.processes[*key].rss_bytes)).sum(),
            }));
        }
        self.previous = Some(current);
        out
    }
}

/// The collector and every process above it.  Their command lines carry the patterns being
/// searched for as arguments - the collector's own, and wrappers such as `sudo`, `timeout` or the
/// shell that started it - and none of them is what anyone means to watch.
fn own_lineage(snapshot: &ProcessSnapshot) -> HashSet<u32> {
    let mut lineage = HashSet::new();
    let mut pid = Some(std::process::id());
    while let Some(current) = pid {
        if !lineage.insert(current) {
            break;
        }
        pid = snapshot.by_pid(current).and_then(|p| p.ppid);
    }
    lineage
}

fn log_changes(name: &str, before: &HashSet<ProcessKey>, after: &HashSet<ProcessKey>, snapshot: &ProcessSnapshot) {
    for key in after.difference(before) {
        if let Some(process) = snapshot.get(key) {
            info!(
                "Tracking process \"{}\" pid {} for \"{name}\" ({})",
                process.info.name,
                key.pid,
                process.info.cmd_line()
            );
        }
    }
    for key in before.difference(after) {
        info!("Process pid {} for \"{name}\" is gone", key.pid);
    }
}

pub fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}
