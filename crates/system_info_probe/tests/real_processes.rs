//! Measurements of real process trees, checked against the kernel's own totals from `wait4`.
#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::Duration;

use system_info_probe::measure::{Ending, MeasureOptions, Measurement, RunLength, TARGET_GROUP};

/// Two busy children for about a second each, then a burst of children too short to sample.
const WORKLOAD: &str = "\
    timeout 1 sh -c 'while :; do :; done' & \
    timeout 1 sh -c 'while :; do :; done' & \
    wait; \
    for i in $(seq 40); do sh -c 'i=0; while [ $i -lt 3000 ]; do i=$((i+1)); done'; done";

fn workload() -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(WORKLOAD).stdout(Stdio::null()).stderr(Stdio::null());
    command
}

fn options() -> MeasureOptions {
    MeasureOptions {
        run: RunLength::UntilExit {
            timeout: Some(Duration::from_secs(30)),
        },
        interval: Duration::from_millis(200),
        ..MeasureOptions::default()
    }
}

fn check(measurement: &Measurement) {
    assert_eq!(measurement.ending, Ending::Exited);
    let group = measurement.recording.group_index(TARGET_GROUP).expect("target group");
    let measured: Duration = measurement.recording.ticks.iter().map(|t| t.groups[group].total_cpu_time()).sum();
    let kernel = measurement.exit.and_then(|e| e.cpu_time).expect("wait4 totals");

    // What the root did after its last sample is credited from its wait4 totals, so the sum
    // matches the kernel; an overcount anywhere would push it above, as the credit never goes
    // negative.  The credit lands on the last tick and must stay a small share, or the sampled
    // part undercounted.
    assert!(kernel >= Duration::from_millis(1500), "workload too light to judge: {kernel:?}");
    let difference = measured.abs_diff(kernel).as_secs_f64() / kernel.as_secs_f64();
    assert!(difference < 0.01, "measured {measured:?} vs kernel {kernel:?}");
    let last_tick_reaped = measurement.recording.ticks.last().expect("ticks").groups[group].reaped_cpu_time;
    assert!(
        last_tick_reaped.as_secs_f64() < kernel.as_secs_f64() * 0.25,
        "tail credit {last_tick_reaped:?} of {kernel:?}"
    );

    let reaped: Duration = measurement.recording.ticks.iter().map(|t| t.groups[group].reaped_cpu_time).sum();
    assert!(reaped > Duration::ZERO, "the short-lived children must surface as reaped CPU");
    let seen = measurement.recording.processes.len();
    assert!(seen >= 5, "sh, both timeouts and both loops are in the tree, saw {seen}");
}

#[test]
fn blocking_tree_cpu_matches_the_kernel() {
    let measurement = system_info_probe::blocking::measure_command(&mut workload(), &options()).expect("measures");
    check(&measurement);
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "multi_thread")]
async fn tokio_tree_cpu_matches_the_kernel() {
    let measurement = system_info_probe::tokio::measure_command(&mut workload(), &options())
        .await
        .expect("measures");
    check(&measurement);
}

#[test]
fn a_fixed_run_kills_the_whole_tree() {
    let options = MeasureOptions {
        run: RunLength::Fixed(Duration::from_millis(600)),
        interval: Duration::from_millis(100),
        ..MeasureOptions::default()
    };
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 30 & sleep 30 & wait");
    let measurement = system_info_probe::blocking::measure_command(&mut command, &options).expect("measures");
    assert_eq!(measurement.ending, Ending::Completed);
    let last = measurement.recording.ticks.last().expect("ticks");
    assert_eq!(last.groups[0].processes, 3, "sh and both sleeps are members");
    let sleeps: Vec<u32> = measurement
        .recording
        .processes
        .values()
        .filter(|p| p.info.name == "sleep")
        .map(|p| p.info.key.pid)
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    for pid in sleeps {
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(state.is_empty() || state.contains(") Z "), "sleep {pid} survived the measurement");
    }
}
