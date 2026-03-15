use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Error;
use log::info;
use sysinfo::{ProcessesToUpdate, System};
use tokio::task::JoinSet;

use crate::settings::CollectSettings;
use crate::shared_state::SharedState;
use crate::workers::{file_writer, network_worker, nvidia_worker, sysinfo_worker};

/// The central collection engine.
///
/// Create with [`CollectorEngine::new`], obtain a shutdown handle via
/// [`CollectorEngine::shutdown_handle`] (pass it to the Ctrl-C handler), then
/// call [`CollectorEngine::run`] from your `#[tokio::main]`.
pub struct CollectorEngine {
    settings: Arc<CollectSettings>,
    state: Arc<RwLock<SharedState>>,
    shutdown: Arc<AtomicBool>,
}

impl CollectorEngine {
    pub fn new(settings: CollectSettings) -> Self {
        let state = SharedState {
            latest_processes: vec![None; settings.process_cmd_to_search.len()],
            ..SharedState::default()
        };
        Self {
            settings: Arc::new(settings),
            state: Arc::new(RwLock::new(state)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
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
    pub async fn run<F>(self, app_version: &str, on_row: F) -> Result<(), Error>
    where
        F: Fn(Vec<String>) + Send + Sync + 'static,
    {
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
        file_writer::write_csv_header(&mut data_file, &sys, &self.settings, app_version)?;
        drop(sys); // no longer needed; workers create their own System instances

        let on_row = Arc::new(on_row);

        let needs_network = self.settings.collection_mode.iter().any(|m| m.is_network());
        let needs_gpu = self.settings.collection_mode.iter().any(|m| m.is_gpu());

        let mut join_set: JoinSet<()> = JoinSet::new();

        // sysinfo worker — always runs (CPU / memory / processes)
        join_set.spawn(sysinfo_worker::run(Arc::clone(&self.settings), Arc::clone(&self.state), Arc::clone(&self.shutdown)));

        // network worker — only when network modes are selected
        if needs_network {
            join_set.spawn(network_worker::run(Arc::clone(&self.settings), Arc::clone(&self.state), Arc::clone(&self.shutdown)));
        }

        // nvidia worker — only when GPU modes are selected (gracefully skips if NVML absent)
        if needs_gpu {
            join_set.spawn(nvidia_worker::run(Arc::clone(&self.settings), Arc::clone(&self.state), Arc::clone(&self.shutdown)));
        }

        // file_writer — the "coordinator" that aggregates snapshots into CSV rows
        join_set.spawn(file_writer::run(Arc::clone(&self.settings), Arc::clone(&self.state), Arc::clone(&self.shutdown), data_file, on_row));

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
