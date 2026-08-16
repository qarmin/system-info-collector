use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Error;
use log::info;
use sysinfo::{ProcessesToUpdate, System};
use tokio::task::JoinSet;

use crate::discovery::{GpuVendor, RuntimeDiscovery, discover_disks, discover_gpus, discover_interfaces};
use crate::settings::CollectSettings;
use crate::shared_state::SharedState;
use crate::workers::{file_writer, network_worker, nvidia_worker, sysinfo_worker};

#[cfg(target_os = "linux")]
use crate::workers::amd_intel_gpu_worker;

/// The central collection engine.
///
/// Create with [`CollectorEngine::new`], obtain a shutdown handle via
/// [`CollectorEngine::shutdown_handle`] (pass it to the Ctrl-C handler), then
/// call [`CollectorEngine::run`] from your `#[tokio::main]`.
pub struct CollectorEngine {
    settings: Arc<CollectSettings>,
    state: Arc<RwLock<SharedState>>,
    shutdown: Arc<AtomicBool>,
    discovery: Arc<RuntimeDiscovery>,
}

impl CollectorEngine {
    /// Create the engine and run hardware discovery once.
    /// Discovery results are stored and exposed via [`CollectorEngine::discovery`].
    pub fn new(settings: Arc<CollectSettings>) -> Self {
        // Hardware discovery always runs, independent of which `-m` metrics were
        // requested, so startup logging / CSV header metadata (detected GPU model,
        // available network interfaces, ...) is accurate even when e.g. no GPU
        // metric is being collected. `discover_interfaces`/`discover_disks` already
        // self-gate on the --network/--all-networks/--disk/--all-disks CLI flags.
        let gpus = discover_gpus();
        let interfaces = discover_interfaces(&settings.network_interfaces, settings.all_networks);
        let disks = discover_disks(&settings.disk_mount_points, settings.all_disks);

        let state = SharedState {
            latest_processes: vec![None; settings.process_cmd_to_search.len()],
            latest_gpus: vec![None; gpus.len()],
            latest_networks: vec![None; interfaces.len()],
            ..SharedState::default()
        };

        let discovery = Arc::new(RuntimeDiscovery { gpus, interfaces, disks });

        Self {
            settings,
            state: Arc::new(RwLock::new(state)),
            shutdown: Arc::new(AtomicBool::new(false)),
            discovery,
        }
    }

    /// Returns a reference to the hardware discovery results.
    pub fn discovery(&self) -> &RuntimeDiscovery {
        &self.discovery
    }

    /// Returns a clone of the shutdown flag.  Set it to `true` to request a
    /// graceful stop of all workers.
    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Run the collection engine until the shutdown flag is set.
    ///
    /// `app_version` should be `env!("CARGO_PKG_VERSION")` from the calling
    /// binary crate (so the version in the CSV header belongs to the binary).
    ///
    /// `on_row` is called with the raw CSV column values after each successful
    /// write.  The CLI uses this to push data into the HTTP server buffer.
    ///
    /// `on_top_row`, when provided, is called each tick when `--top-n-processes`
    /// is active: `(seconds_since_start, top_cpu_vec, top_ram_vec)`.
    #[expect(clippy::type_complexity)]
    pub async fn run<F>(
        self,
        app_version: &str,
        on_row: F,
        on_top_row: Option<Arc<dyn Fn(f64, Vec<(String, f32)>, Vec<(String, f64)>) + Send + Sync>>,
    ) -> Result<(), Error>
    where
        F: Fn(Vec<String>) + Send + Sync + 'static,
    {
        let needs_gpu = self.settings.collection_mode.iter().any(|m| m.is_gpu());
        let needs_network = self.settings.collection_mode.iter().any(|m| m.is_network());

        let discovery = Arc::clone(&self.discovery);

        // Initial System refresh — used only to read metadata for the CSV header.
        let mut sys = System::new_all();
        sys.refresh_memory();
        sys.refresh_cpu_all();
        if self.settings.need_to_refresh_processes {
            sys.refresh_processes(ProcessesToUpdate::All, true);
        }

        // Rotate old backups and open the data file.
        file_writer::backup_old_file(&self.settings)?;
        let mut data_file = file_writer::open_data_file(&self.settings)?;
        let csv_header = file_writer::write_csv_header(&mut data_file, &sys, &self.settings, &discovery, app_version)?;
        drop(sys); // no longer needed; workers create their own System instances

        let on_row = Arc::new(on_row);

        let mut join_set: JoinSet<()> = JoinSet::new();

        // sysinfo worker — always runs (CPU / memory / processes)
        join_set.spawn(sysinfo_worker::run(
            Arc::clone(&self.settings),
            Arc::clone(&self.state),
            Arc::clone(&self.shutdown),
        ));

        // network worker — only when network modes are selected and interfaces found
        if needs_network && !discovery.interfaces.is_empty() {
            join_set.spawn(network_worker::run(
                Arc::clone(&self.settings),
                Arc::clone(&self.state),
                Arc::clone(&self.shutdown),
                Arc::clone(&discovery),
            ));
        }

        if needs_gpu && !discovery.gpus.is_empty() {
            // NVIDIA worker — handles all discovered NVIDIA GPUs
            let nvidia_gpus: Vec<_> = discovery
                .gpus
                .iter()
                .filter(|g| matches!(g.vendor, GpuVendor::Nvidia { .. }))
                .cloned()
                .collect();
            if !nvidia_gpus.is_empty() {
                join_set.spawn(nvidia_worker::run(
                    Arc::clone(&self.settings),
                    Arc::clone(&self.state),
                    Arc::clone(&self.shutdown),
                    Arc::new(nvidia_gpus),
                ));
            }

            // AMD/Intel GPU worker — Linux only
            #[cfg(target_os = "linux")]
            {
                let amd_intel_gpus: Vec<_> = discovery
                    .gpus
                    .iter()
                    .filter(|g| !matches!(g.vendor, GpuVendor::Nvidia { .. }))
                    .cloned()
                    .collect();
                if !amd_intel_gpus.is_empty() {
                    join_set.spawn(amd_intel_gpu_worker::run(
                        Arc::clone(&self.settings),
                        Arc::clone(&self.state),
                        Arc::clone(&self.shutdown),
                        Arc::new(amd_intel_gpus),
                    ));
                }
            }
        }

        // file_writer — the "coordinator" that aggregates snapshots into CSV rows
        join_set.spawn(file_writer::run(
            Arc::clone(&self.settings),
            Arc::clone(&self.state),
            Arc::clone(&self.shutdown),
            data_file,
            on_row,
            Arc::clone(&discovery),
            csv_header,
            on_top_row,
        ));

        info!("All workers started, collecting data…");

        // Wait for all workers to finish (they exit when shutdown flag is set).
        while join_set.join_next().await.is_some() {}

        info!("CollectorEngine stopped");
        Ok(())
    }

    /// Poll the shutdown flag.  Useful in tests or embedding scenarios.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
}
