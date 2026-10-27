use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::AsyncSampler;
use crate::blocking::RecorderOptions;
use crate::error::{ProbeError, ProbeResult};
use crate::recording::{Recording, RecordingBuilder, RecordingConfig, Tick};

/// Samples in a tokio task until stopped or a limit is reached.
pub struct AsyncRecorder;

impl AsyncRecorder {
    /// Every tick is also sent to `ticks`, for live views; a full channel delays sampling
    /// rather than dropping ticks.
    pub fn spawn(
        sampler: AsyncSampler,
        recording: RecordingConfig,
        options: RecorderOptions,
        ticks: Option<mpsc::Sender<Tick>>,
    ) -> ProbeResult<AsyncRecorderHandle> {
        if options.interval.is_zero() {
            return Err(ProbeError::InvalidConfig("the sampling interval must be above zero".to_string()));
        }
        let (stop, stopped) = watch::channel(false);
        let count = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(run(
            sampler,
            RecordingBuilder::new(recording),
            options,
            ticks,
            stopped,
            Arc::clone(&count),
        ));
        Ok(AsyncRecorderHandle { stop, count, task })
    }
}

pub struct AsyncRecorderHandle {
    stop: watch::Sender<bool>,
    count: Arc<AtomicUsize>,
    task: JoinHandle<ProbeResult<Recording>>,
}

impl AsyncRecorderHandle {
    pub fn ticks(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// The recorder notices immediately, also in the middle of waiting for a tick.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    pub async fn join(self) -> ProbeResult<Recording> {
        self.task.await.map_err(|_| ProbeError::WorkerPanicked)?
    }

    pub async fn stop_and_join(self) -> ProbeResult<Recording> {
        self.stop();
        self.join().await
    }
}

async fn run(
    mut sampler: AsyncSampler,
    mut builder: RecordingBuilder,
    options: RecorderOptions,
    ticks: Option<mpsc::Sender<Tick>>,
    mut stopped: watch::Receiver<bool>,
    count: Arc<AtomicUsize>,
) -> ProbeResult<Recording> {
    let start = Instant::now();
    builder.push(sampler.sample().await?);
    let end = options.max_duration.map(|d| start + d);
    let mut interval = tokio::time::interval_at(start + options.interval, options.interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            at = interval.tick() => {
                if end.is_some_and(|end| at > end) {
                    break;
                }
            }
            _ = stopped.wait_for(|stop| *stop) => break,
        }
        if let Some(tick) = builder.push(sampler.sample().await?)
            && let Some(sender) = &ticks
        {
            let _ = sender.send(tick.clone()).await;
        }
        let done = count.fetch_add(1, Ordering::Relaxed) + 1;
        if options.max_ticks.is_some_and(|max| done >= max) {
            break;
        }
    }
    Ok(builder.finish())
}
