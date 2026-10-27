//! Recording the live machine with every source the platform has.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use system_info_probe::blocking::{Recorder, RecorderOptions};
use system_info_probe::process::{Pattern, ProcessDetail, ProcessSamplerConfig, Selector};
use system_info_probe::recording::{ProcessSeries, Recording, RecordingConfig};
use system_info_probe::{Sampler, SamplerConfig};

fn sampler_config() -> SamplerConfig {
    SamplerConfig {
        processes: Some(ProcessSamplerConfig {
            detail: ProcessDetail::Full,
            ..ProcessSamplerConfig::default()
        }),
        system: true,
        #[cfg(feature = "disk")]
        disks: Some((
            system_info_probe::disk::select(
                &system_info_probe::disk::filesystems(),
                &system_info_probe::disk::DiskQuery::All { excluded: vec![] },
            )
            .0,
            system_info_probe::disk::DiskMetrics::SpaceAndIo,
        )),
        #[cfg(feature = "network")]
        interfaces: system_info_probe::network::select(&system_info_probe::network::InterfaceQuery::All { excluded: vec![] }).0,
        ..SamplerConfig::default()
    }
}

fn recording_config() -> RecordingConfig {
    RecordingConfig {
        groups: vec![
            ("me".to_string(), Selector::Pid(std::process::id())),
            ("cargo".to_string(), Selector::Pattern(Pattern::name_is("cargo"))),
        ],
        process_series: ProcessSeries::All,
    }
}

fn options() -> RecorderOptions {
    RecorderOptions {
        interval: Duration::from_millis(100),
        max_ticks: Some(5),
        ..RecorderOptions::default()
    }
}

fn check(recording: &Recording) {
    assert_eq!(recording.ticks.len(), 5);
    assert!(recording.logical_cores >= 1);
    assert_eq!(recording.groups, vec!["me", "cargo"]);
    for tick in &recording.ticks {
        let cpu = tick.cpu.as_ref().expect("system CPU recorded");
        assert!((0.0..=100.0).contains(&cpu.total_percent));
        assert_eq!(cpu.per_core_percent.len(), recording.logical_cores);
        assert!(tick.memory.is_some_and(|m| m.used_bytes > 0 && m.used_bytes <= m.total_bytes));
        assert_eq!(tick.groups[0].processes, 1, "this test process");
        assert!(tick.elapsed >= Duration::from_millis(50), "{:?}", tick.elapsed);
        assert!(!tick.processes.is_empty());
    }
    let me = recording
        .processes
        .values()
        .find(|p| p.info.key.pid == std::process::id())
        .expect("own process recorded");
    assert!(me.alive_at_end);
    assert!(recording.ticks.windows(2).all(|w| w[1].offset > w[0].offset));
}

#[test]
fn blocking_recorder_records_the_machine() {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let handle = Recorder::spawn(
        Sampler::new(sampler_config()).expect("sampler"),
        recording_config(),
        options(),
        Some(Box::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        })),
    )
    .expect("starts");
    check(&handle.join().expect("records"));
    assert_eq!(seen.load(Ordering::Relaxed), 5);
}

#[test]
fn blocking_recorder_stops_promptly() {
    let handle = Recorder::spawn(
        Sampler::new(sampler_config()).expect("sampler"),
        recording_config(),
        RecorderOptions {
            interval: Duration::from_secs(60),
            ..RecorderOptions::default()
        },
        None,
    )
    .expect("starts");
    let started = std::time::Instant::now();
    let recording = handle.stop_and_join().expect("records");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(recording.ticks.is_empty());
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "multi_thread")]
async fn tokio_recorder_records_the_machine_and_streams_ticks() {
    use system_info_probe::tokio::{AsyncRecorder, AsyncSampler};
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let handle = AsyncRecorder::spawn(
        AsyncSampler::new(sampler_config()).await.expect("sampler"),
        recording_config(),
        options(),
        Some(sender),
    )
    .expect("starts");
    let mut streamed = 0;
    while receiver.recv().await.is_some() {
        streamed += 1;
    }
    let recording = handle.join().await.expect("records");
    assert_eq!(streamed, 5);
    check(&recording);
}

#[cfg(all(target_os = "linux", feature = "disk", feature = "network"))]
#[test]
fn disks_and_interfaces_are_recorded_when_present() {
    let config = sampler_config();
    let disks = config.disks.as_ref().map_or(0, |(d, _)| d.len());
    let interfaces = config.interfaces.len();
    let handle = Recorder::spawn(Sampler::new(config).expect("sampler"), RecordingConfig::default(), options(), None).expect("starts");
    let recording = handle.join().expect("records");
    let last = recording.ticks.last().expect("ticks");
    assert_eq!(last.disks.len(), disks);
    assert_eq!(last.networks.len(), interfaces);
    assert!(last.disks.iter().all(|d| d.space.is_some_and(|s| s.total_bytes > 0)));
}
