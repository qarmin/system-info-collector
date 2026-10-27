use std::process::Command;
use std::time::Instant;

use crate::child::{WatchedChild, own_process_group, program_name};
use crate::error::{ProbeError, ProbeResult};
use crate::measure::{ChildExit, Ending, MeasureLoop, MeasureOptions, MeasureScope, Measurement, Stopped};
use crate::recording::RecordingBuilder;
use crate::snapshot::Sampler;

/// Starts `command`, lets it warm up, samples it, then kills it - see [`MeasureOptions`].
/// Stdio is whatever `command` was configured with.
pub fn measure_command(command: &mut Command, options: &MeasureOptions) -> ProbeResult<Measurement> {
    options.validate()?;
    if options.scope == MeasureScope::Tree {
        own_process_group(command);
    }
    let child = command.spawn().map_err(|source| ProbeError::Spawn {
        program: program_name(command),
        source,
    })?;
    let mut child = WatchedChild::new(child);
    let pid = child.pid();
    let result = run(&mut Target::Child(&mut child), options);
    // Whatever happened, the child must not outlive the measurement.
    let killed = child.kill_and_reap(options.scope == MeasureScope::Tree);
    let stopped = result?;
    Ok(stopped.into_measurement(pid, Some(killed?)))
}

/// Samples an already running process for [`MeasureOptions::run`], without touching it.
pub fn measure_pid(pid: u32, options: &MeasureOptions) -> ProbeResult<Measurement> {
    options.validate()?;
    Ok(run(&mut Target::Existing(pid), options)?.into_measurement(pid, None))
}

enum Target<'a> {
    Child(&'a mut WatchedChild),
    Existing(u32),
}

impl Target<'_> {
    fn pid(&self) -> u32 {
        match self {
            Self::Child(child) => child.pid(),
            Self::Existing(pid) => *pid,
        }
    }

    /// Waits until `deadline`, returning early when a spawned child exits.
    fn wait_for_exit_until(&mut self, deadline: Instant) -> ProbeResult<Option<ChildExit>> {
        match self {
            Self::Child(child) => wait_for_exit_until(child, deadline),
            Self::Existing(_) => {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                Ok(None)
            }
        }
    }
}

fn run(target: &mut Target<'_>, options: &MeasureOptions) -> ProbeResult<Stopped> {
    let pid = target.pid();
    let mut sampler = Sampler::new(options.sampler_config(pid))?;
    let mut builder = RecordingBuilder::new(options.recording_config(pid));

    if target.wait_for_exit_until(Instant::now() + options.warmup)?.is_some() {
        return Ok(Stopped {
            builder,
            ending: Ending::ExitedDuringWarmup,
        });
    }

    builder.push(sampler.sample()?);
    let mut run_loop = MeasureLoop::start(options, matches!(target, Target::Existing(_)));
    let ending = loop {
        let wake = run_loop.next_wake();
        let exit = target.wait_for_exit_until(wake)?;
        builder.push(sampler.sample()?);
        if let Some(ending) = run_loop.after_sample(wake, exit.is_some(), &builder) {
            break ending;
        }
    };
    Ok(Stopped { builder, ending })
}

#[cfg(target_os = "linux")]
fn wait_for_exit_until(child: &mut WatchedChild, deadline: Instant) -> ProbeResult<Option<ChildExit>> {
    use std::os::fd::AsRawFd;
    loop {
        if let Some(exit) = child.try_reap()? {
            return Ok(Some(exit));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let Some(pidfd) = child.pidfd() else {
            std::thread::sleep(remaining);
            return Ok(child.try_reap()?);
        };
        let mut poll = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32 + i32::from(!remaining.subsec_nanos().is_multiple_of(1_000_000));
        // SAFETY: one valid pollfd for the duration of the call.
        unsafe { libc::poll(&raw mut poll, 1, timeout_ms) };
    }
}

#[cfg(not(target_os = "linux"))]
fn wait_for_exit_until(child: &mut WatchedChild, deadline: Instant) -> ProbeResult<Option<ChildExit>> {
    const SLICE: std::time::Duration = std::time::Duration::from_millis(20);
    loop {
        if let Some(exit) = child.try_reap()? {
            return Ok(Some(exit));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        std::thread::sleep(remaining.min(SLICE));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::measure::RunLength;

    #[test]
    fn exit_during_warmup_is_reported_without_samples() {
        let options = MeasureOptions {
            warmup: Duration::from_secs(5),
            ..MeasureOptions::default()
        };
        let started = Instant::now();
        let measurement = measure_command(Command::new("true").stdout(std::process::Stdio::null()), &options).expect("runs");
        assert_eq!(measurement.ending, Ending::ExitedDuringWarmup);
        assert!(measurement.recording.ticks.is_empty());
        assert!(started.elapsed() < Duration::from_secs(4), "exit should end the warm-up early");
        assert!(measurement.exit.is_some_and(|e| e.status.success()));
    }

    #[test]
    fn rejects_a_zero_interval() {
        let options = MeasureOptions {
            interval: Duration::ZERO,
            run: RunLength::Fixed(Duration::from_millis(10)),
            ..MeasureOptions::default()
        };
        measure_command(&mut Command::new("true"), &options).expect_err("zero interval is rejected");
    }
}
