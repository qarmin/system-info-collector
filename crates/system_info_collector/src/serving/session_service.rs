//! Drives recordings requested from the browser.
//!
//! One recording at a time: a second request is refused rather than queued, so
//! two overlapping recorders never compete for `/proc` and skew each other's
//! numbers.
//!
//! These are the only endpoints in this server that *do* something rather than
//! just read, and like the rest of it they are unauthenticated and reachable
//! from wherever the port is.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use log::{error, info};
use serde::{Deserialize, Serialize};
use system_info_collector_core::session_recorder::{self, SessionConfig, SessionProgress};

use crate::session_store::SessionStore;

pub struct SessionService {
    store: SessionStore,
    active: Mutex<Option<ActiveRecording>>,
    /// File name of the most recent finished recording, so the page can jump
    /// straight to it when the countdown ends.
    finished: Mutex<Option<String>>,
    app_version: &'static str,
}

struct ActiveRecording {
    started: Instant,
    config: SessionConfig,
    stop: Arc<AtomicBool>,
    progress: Arc<SessionProgress>,
}

#[derive(Deserialize)]
pub struct StartRequest {
    pub seconds: f64,
    pub hz: Option<f32>,
}

#[derive(Serialize)]
pub struct SessionStatus {
    pub active: bool,
    pub elapsed_secs: f64,
    pub remaining_secs: f64,
    pub duration_secs: f64,
    pub hz: f32,
    pub ticks_done: usize,
    pub ticks_total: usize,
    pub processes_seen: usize,
    /// Set once a recording completes, cleared when the next one starts.
    pub finished_file: Option<String>,
    pub max_hz: f32,
}

pub enum StartError {
    Busy,
    Invalid(String),
}

impl SessionService {
    pub fn new(store: SessionStore, app_version: &'static str) -> Self {
        Self {
            store,
            active: Mutex::new(None),
            finished: Mutex::new(None),
            app_version,
        }
    }

    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    pub fn status(&self) -> SessionStatus {
        let active = self.active.lock().expect("session lock poisoned");
        let finished_file = self.finished.lock().expect("session lock poisoned").clone();

        let Some(recording) = active.as_ref() else {
            return SessionStatus {
                active: false,
                elapsed_secs: 0.0,
                remaining_secs: 0.0,
                duration_secs: 0.0,
                hz: 0.0,
                ticks_done: 0,
                ticks_total: 0,
                processes_seen: 0,
                finished_file,
                max_hz: session_recorder::MAX_SESSION_HZ,
            };
        };

        let elapsed = recording.started.elapsed().as_secs_f64();
        SessionStatus {
            active: true,
            elapsed_secs: elapsed,
            remaining_secs: (recording.config.duration_secs - elapsed).max(0.0),
            duration_secs: recording.config.duration_secs,
            hz: recording.config.hz,
            ticks_done: recording.progress.ticks_done.load(Ordering::Relaxed),
            ticks_total: recording.config.tick_count(),
            processes_seen: recording.progress.processes_seen.load(Ordering::Relaxed),
            finished_file,
            max_hz: session_recorder::MAX_SESSION_HZ,
        }
    }

    /// Starts a recording in the background.  `Err` means the request was
    /// rejected outright - the configuration is invalid, or one is already running.
    pub fn start(self: &Arc<Self>, request: &StartRequest) -> Result<SessionStatus, StartError> {
        let config = SessionConfig {
            duration_secs: request.seconds,
            hz: request.hz.unwrap_or(session_recorder::MAX_SESSION_HZ),
        };
        config.validate().map_err(|e| StartError::Invalid(e.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(SessionProgress::default());

        {
            let mut active = self.active.lock().expect("session lock poisoned");
            if active.is_some() {
                return Err(StartError::Busy);
            }
            *self.finished.lock().expect("session lock poisoned") = None;
            *active = Some(ActiveRecording {
                started: Instant::now(),
                config,
                stop: Arc::clone(&stop),
                progress: Arc::clone(&progress),
            });
        }

        info!("Session recording started from the web UI: {}s at {} Hz", config.duration_secs, config.hz);

        let service = Arc::clone(self);
        let app_version = self.app_version;
        tokio::spawn(async move {
            let recorder = Arc::clone(&service);
            // The recorder blocks and sleeps between ticks, so it must not sit on
            // one of the server's two async worker threads.
            let outcome = tokio::task::spawn_blocking(move || {
                session_recorder::record(config, &stop, &progress, app_version).and_then(|data| recorder.store.save(&data))
            })
            .await;

            let finished = match outcome {
                Ok(Ok(path)) => path.file_name().map(|name| name.to_string_lossy().into_owned()),
                Ok(Err(e)) => {
                    error!("Session recording failed: {e}");
                    None
                }
                Err(e) => {
                    error!("Session recording task panicked: {e}");
                    None
                }
            };
            // Runs on every path, so a failed recording still releases the slot
            // instead of blocking every later request with 409.
            service.complete(finished);
        });

        Ok(self.status())
    }

    pub fn stop(&self) -> bool {
        let active = self.active.lock().expect("session lock poisoned");
        let Some(recording) = active.as_ref() else { return false };
        info!("Session stop requested from the web UI");
        recording.stop.store(true, Ordering::Relaxed);
        true
    }

    fn complete(&self, finished_file: Option<String>) {
        *self.active.lock().expect("session lock poisoned") = None;
        *self.finished.lock().expect("session lock poisoned") = finished_file;
    }
}
