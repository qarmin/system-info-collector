use std::process::Command;
use std::time::Duration;

use tokio::time::Instant;

use super::AsyncSampler;
use crate::child::{WatchedChild, own_process_group, program_name};
use crate::error::{ProbeError, ProbeResult};
use crate::measure::{ChildExit, Ending, MeasureLoop, MeasureOptions, MeasureScope, Measurement, Stopped};
use crate::recording::RecordingBuilder;

/// [`crate::blocking::measure_command`] on tokio.  Takes a `std` command: spawning does not
/// block, and the child is watched through a pidfd registered with tokio and reaped with `wait4`
/// for its exact CPU totals, which `tokio::process` cannot provide.
pub async fn measure_command(command: &mut Command, options: &MeasureOptions) -> ProbeResult<Measurement> {
    options.validate()?;
    if options.scope == MeasureScope::Tree {
        own_process_group(command);
    }
    let child = command.spawn().map_err(|source| ProbeError::Spawn {
        program: program_name(command),
        source,
    })?;
    let mut child = ExitWatch::new(WatchedChild::new(child));
    let pid = child.child.pid();
    let result = run(Some(&mut child), pid, options).await;
    let killed = child.child.kill_and_reap(options.scope == MeasureScope::Tree);
    let stopped = result?;
    Ok(stopped.into_measurement(pid, Some(killed?)))
}

/// [`crate::blocking::measure_pid`] on tokio.
pub async fn measure_pid(pid: u32, options: &MeasureOptions) -> ProbeResult<Measurement> {
    options.validate()?;
    Ok(run(None, pid, options).await?.into_measurement(pid, None))
}

async fn run(mut child: Option<&mut ExitWatch>, pid: u32, options: &MeasureOptions) -> ProbeResult<Stopped> {
    let mut sampler = AsyncSampler::new(options.sampler_config(pid)).await?;
    let mut builder = RecordingBuilder::new(options.recording_config(pid));

    if let Some(child) = child.as_deref_mut()
        && child.exit_before(Instant::now() + options.warmup).await?.is_some()
    {
        return Ok(Stopped {
            builder,
            ending: Ending::ExitedDuringWarmup,
            interval: options.interval,
        });
    }
    if child.is_none() {
        tokio::time::sleep(options.warmup).await;
    }

    builder.push(sampler.sample().await?);
    let mut run_loop = MeasureLoop::start(options, child.is_none());
    let ending = loop {
        let wake = run_loop.next_wake();
        let exited = match child.as_deref_mut() {
            Some(child) => child.exit_before(wake.into()).await?.is_some(),
            None => {
                tokio::time::sleep_until(wake.into()).await;
                false
            }
        };
        builder.push(sampler.sample().await?);
        if let Some(ending) = run_loop.after_sample(wake, exited, &builder) {
            break ending;
        }
    };
    Ok(Stopped {
        builder,
        ending,
        interval: options.interval,
    })
}

/// `readiness` is declared first so its registration is dropped before `child` closes the pidfd.
struct ExitWatch {
    #[cfg(target_os = "linux")]
    readiness: Option<tokio::io::unix::AsyncFd<std::os::fd::RawFd>>,
    child: WatchedChild,
}

impl ExitWatch {
    fn new(child: WatchedChild) -> Self {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let readiness = child
                .pidfd()
                .and_then(|fd| tokio::io::unix::AsyncFd::with_interest(fd.as_raw_fd(), tokio::io::Interest::READABLE).ok());
            Self { readiness, child }
        }
        #[cfg(not(target_os = "linux"))]
        Self { child }
    }

    /// The exit, if it happens before `deadline`.
    async fn exit_before(&mut self, deadline: Instant) -> ProbeResult<Option<ChildExit>> {
        loop {
            if let Some(exit) = self.child.try_reap()? {
                return Ok(Some(exit));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            #[cfg(target_os = "linux")]
            if let Some(readiness) = &self.readiness {
                tokio::select! {
                    ready = readiness.readable() => {
                        let mut guard = ready?;
                        if let Some(exit) = self.child.try_reap()? {
                            return Ok(Some(exit));
                        }
                        guard.clear_ready();
                    }
                    () = tokio::time::sleep_until(deadline) => {}
                }
                continue;
            }
            tokio::time::sleep_until(deadline.min(Instant::now() + Duration::from_millis(20))).await;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::measure::RunLength;

    #[tokio::test]
    async fn notices_an_exit_immediately() {
        let options = MeasureOptions {
            run: RunLength::UntilExit {
                timeout: Some(Duration::from_secs(10)),
            },
            interval: Duration::from_secs(5),
            ..MeasureOptions::default()
        };
        let started = std::time::Instant::now();
        let measurement = measure_command(Command::new("sleep").arg("0.3"), &options).await.expect("runs");
        assert_eq!(measurement.ending, Ending::Exited);
        assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
        assert!(measurement.exit.is_some_and(|e| e.status.success()));
    }
}
