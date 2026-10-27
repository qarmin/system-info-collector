use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "disk")]
use crate::disk::{DiskMetrics, DiskSample, DiskSampler, TrackedDisk};
use crate::error::ProbeResult;
use crate::gpu::{Gpu, GpuSample, GpuSampler};
#[cfg(feature = "network")]
use crate::network::{Interface, NetworkSample, NetworkSampler};
use crate::process::{ProcessSampler, ProcessSamplerConfig, ProcessSnapshot};
use crate::system::{SystemSample, SystemSampler};

/// Which sources a [`Sampler`] reads.  Everything is off by default.
#[derive(Clone, Debug, Default)]
pub struct SamplerConfig {
    pub processes: Option<ProcessSamplerConfig>,
    pub system: bool,
    #[cfg(feature = "disk")]
    pub disks: Option<(Vec<TrackedDisk>, DiskMetrics)>,
    #[cfg(feature = "network")]
    pub interfaces: Vec<Interface>,
    pub gpus: Vec<Gpu>,
}

/// What the per-device entries of samples and ticks refer to, in the same order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sources {
    #[cfg(feature = "disk")]
    pub disks: Vec<TrackedDisk>,
    #[cfg(feature = "network")]
    pub interfaces: Vec<Interface>,
    pub gpus: Vec<Gpu>,
}

/// Everything one [`Sampler::sample`] read; sources that are off are `None`.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub taken_at: Instant,
    pub unix_time: f64,
    pub processes: Option<ProcessSnapshot>,
    pub system: Option<SystemSample>,
    #[cfg(feature = "disk")]
    pub disks: Option<DiskSample>,
    #[cfg(feature = "network")]
    pub networks: Option<NetworkSample>,
    pub gpus: Option<GpuSample>,
}

/// Reads the configured sources one after another.  Blocking: file reads and driver calls.
pub struct Sampler {
    processes: Option<ProcessSampler>,
    system: Option<SystemSampler>,
    #[cfg(feature = "disk")]
    disks: Option<DiskSampler>,
    #[cfg(feature = "network")]
    networks: Option<NetworkSampler>,
    gpus: Option<GpuSampler>,
}

impl Sampler {
    pub fn new(config: SamplerConfig) -> ProbeResult<Self> {
        Ok(Self {
            processes: config.processes.map(ProcessSampler::new).transpose()?,
            system: config.system.then(SystemSampler::new).transpose()?,
            #[cfg(feature = "disk")]
            disks: config.disks.map(|(disks, metrics)| DiskSampler::new(disks, metrics)),
            #[cfg(feature = "network")]
            networks: (!config.interfaces.is_empty()).then(|| NetworkSampler::new(config.interfaces)),
            gpus: (!config.gpus.is_empty()).then(|| GpuSampler::new(config.gpus)),
        })
    }

    pub fn processes_mut(&mut self) -> Option<&mut ProcessSampler> {
        self.processes.as_mut()
    }

    #[cfg(feature = "disk")]
    pub fn disks(&self) -> &[TrackedDisk] {
        self.disks.as_ref().map_or(&[], DiskSampler::disks)
    }

    #[cfg(feature = "network")]
    pub fn interfaces(&self) -> &[Interface] {
        self.networks.as_ref().map_or(&[], NetworkSampler::interfaces)
    }

    pub fn gpus(&self) -> &[Gpu] {
        self.gpus.as_ref().map_or(&[], GpuSampler::gpus)
    }

    pub fn sources(&self) -> Sources {
        Sources {
            #[cfg(feature = "disk")]
            disks: self.disks().to_vec(),
            #[cfg(feature = "network")]
            interfaces: self.interfaces().to_vec(),
            gpus: self.gpus().to_vec(),
        }
    }

    pub fn sample(&mut self) -> ProbeResult<Snapshot> {
        let taken_at = Instant::now();
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        Ok(Snapshot {
            taken_at,
            unix_time,
            system: self.system.as_mut().map(SystemSampler::sample).transpose()?,
            processes: self.processes.as_mut().map(ProcessSampler::sample).transpose()?,
            #[cfg(feature = "disk")]
            disks: self.disks.as_mut().map(DiskSampler::sample),
            #[cfg(feature = "network")]
            networks: self.networks.as_mut().map(NetworkSampler::sample),
            gpus: self.gpus.as_mut().map(GpuSampler::sample),
        })
    }
}
