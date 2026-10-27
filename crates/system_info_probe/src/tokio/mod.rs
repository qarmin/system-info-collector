//! Driving the samplers on tokio: tokio timers, cancellation, channels and sockets.
//!
//! Reading `/proc` and calling GPU drivers is blocking work with no asynchronous kernel interface,
//! so a whole sample runs as one `spawn_blocking` job - one hand-off per sample instead of one per
//! file, which is what `tokio::fs` would cost.

mod measure;
#[cfg(all(target_os = "linux", feature = "ping"))]
mod ping;
mod recorder;

pub use measure::{measure_command, measure_pid};
#[cfg(all(target_os = "linux", feature = "ping"))]
pub use ping::{AsyncPinger, resolve};
pub use recorder::{AsyncRecorder, AsyncRecorderHandle};

use crate::error::{ProbeError, ProbeResult};
use crate::snapshot::{Sampler, SamplerConfig, Snapshot};

/// A [`Sampler`] whose blocking reads run on tokio's blocking pool.
pub struct AsyncSampler {
    inner: Option<Sampler>,
}

impl AsyncSampler {
    pub async fn new(config: SamplerConfig) -> ProbeResult<Self> {
        let sampler = ::tokio::task::spawn_blocking(move || Sampler::new(config))
            .await
            .map_err(|_| ProbeError::WorkerPanicked)??;
        Ok(Self::from_sampler(sampler))
    }

    pub fn from_sampler(sampler: Sampler) -> Self {
        Self { inner: Some(sampler) }
    }

    /// After a panic inside a sample the sampler is gone and every later call fails.
    pub async fn sample(&mut self) -> ProbeResult<Snapshot> {
        let mut sampler = self.inner.take().ok_or(ProbeError::WorkerPanicked)?;
        let (sampler, snapshot) = ::tokio::task::spawn_blocking(move || {
            let snapshot = sampler.sample();
            (sampler, snapshot)
        })
        .await
        .map_err(|_| ProbeError::WorkerPanicked)?;
        self.inner = Some(sampler);
        snapshot
    }

    pub fn sampler_mut(&mut self) -> Option<&mut Sampler> {
        self.inner.as_mut()
    }

    pub fn into_inner(self) -> Option<Sampler> {
        self.inner
    }
}
