use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Error;
use log::{debug, info};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::model::{CustomProcessData, ProcessCache};
use crate::settings::CollectSettings;
use crate::shared_state::{ProcessSnapshot, SharedState, SysinfoSnapshot};

pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>) {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    if settings.need_to_refresh_processes {
        sys.refresh_processes(ProcessesToUpdate::All, true);
    }

    let mut process_cache = ProcessCache::new_with_size(settings.process_cmd_to_search.len(), &sys);
    let interval_ms = (settings.sysinfo_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let start = Instant::now();
        sys.refresh_cpu_usage();
        sys.refresh_memory();

        // If top-N tracking is enabled, refresh all processes.
        // exe is read once (OnlyIfNotSet) and cached; subsequent refreshes are cheap.
        // We need exe so we can group processes by their real binary path instead of
        // the process name, which applications (e.g. Firefox) often change at runtime.
        if settings.top_n_processes > 0 {
            sys.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet),
            );
        } else if settings.need_to_refresh_processes
            && let Err(e) = check_for_new_and_old_process_data(&mut sys, &mut process_cache, &settings)
        {
            log::warn!("Process tracking error: {e}");
        }

        // When top_n AND tracked processes are both active, update tracked processes
        // using data that's already been refreshed by the full process refresh above.
        if settings.top_n_processes > 0
            && settings.need_to_refresh_processes
            && let Err(e) = check_for_new_and_old_process_data(&mut sys, &mut process_cache, &settings)
        {
            log::warn!("Process tracking error: {e}");
        }

        debug!("sysinfo_worker refreshed in {:?}", start.elapsed());

        let cpu_count = sys.cpus().len().max(1);
        let snapshot = SysinfoSnapshot {
            cpu_usage_total: sys.cpus().iter().map(sysinfo::Cpu::cpu_usage).sum::<f32>() as f64 / cpu_count as f64,
            cpu_usage_per_core: sys.cpus().iter().map(|c| c.cpu_usage() as f64).collect(),
            memory_used_mb: bytes_to_mb(sys.used_memory()),
            memory_free_mb: bytes_to_mb(sys.free_memory()),
            memory_available_mb: bytes_to_mb(sys.available_memory()),
            swap_used_mb: bytes_to_mb(sys.used_swap()),
            swap_free_mb: bytes_to_mb(sys.free_swap()),
        };

        let process_snapshots: Vec<Option<ProcessSnapshot>> = process_cache
            .process_used
            .iter()
            .map(|opt| {
                opt.as_ref().map(|p| ProcessSnapshot {
                    pid: p.pid,
                    name: p.name.clone(),
                    cpu_usage: p.cpu_usage / cpu_count as f32,
                    memory_mb: bytes_to_mb(p.memory_usage),
                })
            })
            .collect();

        // Collect top-N processes by CPU% and RAM, grouped by executable name.
        let mut top_cpu_snap: Vec<(String, f32)> = Vec::new();
        let mut top_ram_snap: Vec<(String, f64)> = Vec::new();
        if settings.top_n_processes > 0 {
            let n = settings.top_n_processes;
            let cpu_divisor = cpu_count as f32;

            // Sum CPU% and RAM, grouped by the exe path of each process.
            // Threads are skipped: on Linux they appear as separate entries but carry the
            // parent's RSS, which would massively inflate the RAM totals if summed.
            // We use the full exe path as the grouping key (not proc.name()) because many
            // applications — notably Firefox — rename their processes at runtime via
            // prctl(PR_SET_NAME) to e.g. "Isolated Web Co", while all instances still
            // share the same exe path.  The display name is the basename of that path.
            //
            // key   = full exe path (different installations of the same binary stay separate)
            // value = (display_basename, total_cpu, total_ram)
            let mut by_exe: HashMap<String, (String, f32, u64)> = HashMap::new();
            for proc in sys.processes().values() {
                // Skip kernel and userland threads (e.g. "DefaultDispatch", "DOM Worker").
                if proc.thread_kind().is_some() {
                    continue;
                }
                let (key, display) = match proc.exe() {
                    Some(path) => {
                        let key = path.to_string_lossy().into_owned();
                        let display = path.file_name().map_or_else(|| key.clone(), |n| n.to_string_lossy().into_owned());
                        (key, display)
                    }
                    None => {
                        let n = proc.name().to_string_lossy().into_owned();
                        (n.clone(), n)
                    }
                };
                let entry = by_exe.entry(key).or_insert((display, 0.0, 0));
                entry.1 += proc.cpu_usage();
                entry.2 += proc.memory();
            }

            let mut cpu_vec: Vec<(String, f32)> = by_exe.values().map(|(name, cpu, _)| (name.clone(), *cpu)).collect();
            cpu_vec.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            // Only keep processes using more than 1% of total CPU to avoid noise.
            top_cpu_snap = cpu_vec
                .into_iter()
                .map(|(name, cpu)| (name, cpu / cpu_divisor))
                .filter(|(_, cpu_pct)| *cpu_pct > 1.0)
                .take(n)
                .collect();

            let mut ram_vec: Vec<(String, u64)> = by_exe.into_values().map(|(name, _, ram)| (name, ram)).collect();
            ram_vec.sort_unstable_by(|a, b| b.1.cmp(&a.1));
            top_ram_snap = ram_vec.into_iter().take(n).map(|(name, mem)| (name, bytes_to_mb(mem))).collect();
        }

        {
            let mut guard = state.write().expect("SharedState RwLock poisoned");
            guard.latest_sysinfo = Some(snapshot);
            guard.latest_processes = process_snapshots;
            if settings.top_n_processes > 0 {
                guard.latest_top_cpu = top_cpu_snap;
                guard.latest_top_ram = top_ram_snap;
            }
        }
    }

    info!("sysinfo_worker stopped");
}

// ─── Process tracking helpers (moved from collector.rs) ───────────────────────

#[cfg(target_os = "linux")]
fn get_system_pids(_sys: &mut System) -> Result<HashSet<usize>, Error> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Err(Error::msg("Failed to read /proc directory"));
    };

    let mut pids = HashSet::new();
    for entry in entries.flatten() {
        if let Ok(file_type) = entry.file_type() {
            if !file_type.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && let Ok(pid) = name.parse::<usize>()
            {
                pids.insert(pid);
            }
        }
    }

    Ok(pids)
}

#[cfg(not(target_os = "linux"))]
fn get_system_pids(sys: &mut System) -> Result<HashSet<usize>, Error> {
    use sysinfo::RefreshKind;
    sys.refresh_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet)));
    Ok(sys.processes().keys().map(|pid| pid.as_u32() as usize).collect())
}

pub fn check_for_new_and_old_process_data(sys: &mut System, process_cache: &mut ProcessCache, settings: &CollectSettings) -> Result<(), Error> {
    let system_pids = get_system_pids(sys)?;

    if process_cache
        .process_used
        .iter()
        .all(|e| e.as_ref().is_some_and(|p| system_pids.contains(&p.pid)))
    {
        update_usage_of_tracked_process(process_cache, sys);
        return Ok(());
    }

    update_new_processes_stats(process_cache, sys, &system_pids);
    remove_tracking_of_removed_processes(process_cache, &system_pids);
    check_which_process_to_track(process_cache, sys, settings, &system_pids);
    update_usage_of_tracked_process(process_cache, sys);
    process_cache.replace_checked_to_be_used_processes(system_pids.iter());

    Ok(())
}

fn check_which_process_to_track(process_cache: &mut ProcessCache, sys: &System, settings: &CollectSettings, system_pids: &HashSet<usize>) {
    for (idx, search) in settings.process_cmd_to_search.iter().enumerate() {
        if process_cache.process_used[idx].is_some() {
            continue;
        }

        let mut best: Option<(usize, &sysinfo::Process, String)> = None;
        let mut best_len = usize::MAX;

        for (pid, process) in sys.processes() {
            let pid_num: usize = (*pid).into();
            if process_cache.processes_checked_to_be_used.contains(&pid_num) || !system_pids.contains(&pid_num) {
                continue;
            }
            let cmd = process.cmd().iter().map(|e| e.to_string_lossy()).collect::<Vec<_>>().join(" ");
            if cmd.contains(&search.search_text) && cmd.len() < best_len {
                best_len = cmd.len();
                best = Some((pid_num, process, cmd));
            }
        }

        if let Some((pid_num, process, cmd)) = best {
            info!(
                "Found process \"{}\" pid \"{}\" matching \"{}\" (\"{}\")",
                process.name().to_string_lossy(),
                pid_num,
                search.graph_name,
                cmd
            );
            process_cache.processes_checked_to_be_used.insert(pid_num);
            process_cache.process_used[idx] = Some(CustomProcessData::from_process(process));
        }
    }
}

fn remove_tracking_of_removed_processes(process_cache: &mut ProcessCache, system_pids: &HashSet<usize>) {
    process_cache.process_used = process_cache
        .process_used
        .clone()
        .into_iter()
        .map(|e| {
            if let Some(e) = e {
                if system_pids.contains(&e.pid) {
                    Some(e)
                } else {
                    info!("Process \"{}\" pid \"{}\" gone, removing from monitoring", e.name, e.pid);
                    None
                }
            } else {
                None
            }
        })
        .collect();
}

fn update_new_processes_stats(process_cache: &mut ProcessCache, sys: &mut System, system_pids: &HashSet<usize>) {
    let new_pids = process_cache.get_differences_in_usage_processes(system_pids.iter());

    if !new_pids.is_empty() {
        let t = Instant::now();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&new_pids.iter().map(|i| Pid::from(*i)).collect::<Vec<_>>()),
            true,
            ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_cwd(UpdateKind::OnlyIfNotSet)
                .with_cpu()
                .with_memory(),
        );
        info!("Refreshed {} new processes in {:?}", new_pids.len(), t.elapsed());
    }
    process_cache.replace_checked_usage_processes(system_pids.iter());
}

fn update_usage_of_tracked_process(process_cache: &mut ProcessCache, sys: &mut System) {
    let count = process_cache.process_used.iter().flatten().count();
    if count == 0 {
        return;
    }
    debug!("Updating {count} tracked processes");

    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&process_cache.process_used.iter().flatten().map(|p| Pid::from(p.pid)).collect::<Vec<_>>()),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );

    for proc_data in process_cache.process_used.iter_mut().flatten() {
        let Some(process) = sys.processes().get(&Pid::from(proc_data.pid)) else {
            continue;
        };
        proc_data.memory_usage = process.memory();
        proc_data.cpu_usage = process.cpu_usage();
    }
}

pub fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

pub fn bytes_to_mb_string(bytes: u64) -> String {
    format!("{:.2}", bytes_to_mb(bytes))
}
