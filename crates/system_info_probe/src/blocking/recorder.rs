use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{ProbeError, ProbeResult};
use crate::recording::{Recording, RecordingBuilder, RecordingConfig, Tick};
use crate::schedule::next_deadline;
use crate::snapshot::Sampler;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecorderOptions {
    pub interval: Duration,
    pub max_duration: Option<Duration>,
    pub max_ticks: Option<usize>,
}

impl Default for RecorderOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_duration: None,
            max_ticks: None,
        }
    }
}

type TickCallback = Box<dyn FnMut(&Tick) + Send>;

/// Samples on a thread of its own until stopped or a limit is reached.
pub struct Recorder;

impl Recorder {
    /// `on_tick` runs on the recorder thread after every tick, for live views.
    pub fn spawn(
        sampler: Sampler,
        recording: RecordingConfig,
        options: RecorderOptions,
        on_tick: Option<TickCallback>,
    ) -> ProbeResult<RecorderHandle> {
        if options.interval.is_zero() {
            return Err(ProbeError::InvalidConfig("the sampling interval must be above zero".to_string()));
        }
        let (stop, stopped) = mpsc::channel();
        let ticks = Arc::new(AtomicUsize::new(0));
        let thread = {
            let ticks = Arc::clone(&ticks);
            std::thread::Builder::new()
                .name("probe-recorder".to_string())
                .spawn(move || run(sampler, RecordingBuilder::new(recording), options, on_tick, &stopped, &ticks))?
        };
        Ok(RecorderHandle { stop, ticks, thread })
    }
}

pub struct RecorderHandle {
    stop: mpsc::Sender<()>,
    ticks: Arc<AtomicUsize>,
    thread: JoinHandle<ProbeResult<Recording>>,
}

impl RecorderHandle {
    pub fn ticks(&self) -> usize {
        self.ticks.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Stops the recorder; it notices immediately rather than at the next tick.
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }

    /// Waits for a limit to be reached; call [`Self::stop`] first to end it now.
    pub fn join(self) -> ProbeResult<Recording> {
        self.thread.join().map_err(|_| ProbeError::WorkerPanicked)?
    }

    pub fn stop_and_join(self) -> ProbeResult<Recording> {
        self.stop();
        self.join()
    }
}

fn run(
    mut sampler: Sampler,
    mut builder: RecordingBuilder,
    options: RecorderOptions,
    mut on_tick: Option<TickCallback>,
    stopped: &mpsc::Receiver<()>,
    ticks: &AtomicUsize,
) -> ProbeResult<Recording> {
    let start = Instant::now();
    let end = options.max_duration.map(|d| start + d);
    builder.set_sources(sampler.sources());
    builder.push(sampler.sample()?);
    let mut deadline = start;
    loop {
        deadline = next_deadline(deadline, options.interval, Instant::now());
        if let Some(end) = end
            && deadline > end
        {
            break;
        }
        match stopped.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
        }
        if let Some(tick) = builder.push(sampler.sample()?)
            && let Some(callback) = on_tick.as_mut()
        {
            callback(tick);
        }
        let done = ticks.fetch_add(1, Ordering::Relaxed) + 1;
        if options.max_ticks.is_some_and(|max| done >= max) {
            break;
        }
    }
    Ok(builder.finish())
}
