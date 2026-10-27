//! Sampling of processes, CPU, memory, disks, network interfaces, GPUs and ping latency.
//!
//! Every source has a sampler returning plain data, and everything that turns samples into rates,
//! group totals and recordings is pure and shared.  [`blocking`] drives it from a thread with std
//! timers, processes and sockets; with the `tokio` feature [`tokio`] does the same natively on tokio.
//!
//! Measuring a program and everything it starts, the way a benchmark script would:
//!
//! ```no_run
//! use std::time::Duration;
//! use system_info_probe::measure::{MeasureOptions, RunLength, TARGET_GROUP};
//!
//! let options = MeasureOptions {
//!     warmup: Duration::from_secs(2),
//!     run: RunLength::Fixed(Duration::from_secs(10)),
//!     ..MeasureOptions::default()
//! };
//! let measurement = system_info_probe::blocking::measure_command(std::process::Command::new("./app").arg("--bench"), &options)?;
//! let group = measurement.recording.group_index(TARGET_GROUP).expect("always present");
//! let cpu: Duration = measurement.recording.ticks.iter().map(|t| t.groups[group].total_cpu_time()).sum();
//! let peak_rss = measurement.recording.ticks.iter().map(|t| t.groups[group].rss_bytes).max();
//! # let _ = (cpu, peak_rss);
//! # Ok::<(), system_info_probe::ProbeError>(())
//! ```
//!
//! Following every process matching a pattern, including ones started later, from a loop of your own:
//!
//! ```no_run
//! use system_info_probe::process::{Pattern, ProcessInterval, ProcessSampler, ProcessSamplerConfig, Selection, Selector};
//!
//! let mut sampler = ProcessSampler::new(ProcessSamplerConfig::default())?;
//! let mut selection = Selection::new(Selector::Pattern(Pattern::cmd_contains("ffmpeg")));
//! let mut previous = sampler.sample()?;
//! loop {
//!     std::thread::sleep(std::time::Duration::from_secs(1));
//!     let current = sampler.sample()?;
//!     let interval = ProcessInterval::between(&previous, &current);
//!     for key in selection.update(&current) {
//!         let percent = interval.cpu_percent_of_core(interval.deltas[&key].cpu_time);
//!         # let _ = percent;
//!     }
//!     previous = current;
//! }
//! # Ok::<(), system_info_probe::ProbeError>(())
//! ```

pub mod backend;
pub mod blocking;
mod child;
#[cfg(feature = "disk")]
pub mod disk;
pub mod error;
pub mod gpu;
pub mod measure;
#[cfg(feature = "network")]
pub mod network;
#[cfg(all(target_os = "linux", any(feature = "network", feature = "gpu-amd", feature = "gpu-intel")))]
mod pci;
#[cfg(all(target_os = "linux", feature = "ping"))]
pub mod ping;
pub mod process;
pub mod recording;
mod schedule;
pub mod snapshot;
pub mod system;
#[cfg(feature = "tokio")]
pub mod tokio;

pub use error::{ProbeError, ProbeResult};
pub use snapshot::{Sampler, SamplerConfig, Snapshot};
