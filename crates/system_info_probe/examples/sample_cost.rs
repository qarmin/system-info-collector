//! Time per sample of every process: this crate's sampler against a sysinfo refresh configured
//! configured with the same refresh kinds as the collector app uses for its process recordings.

use std::time::{Duration, Instant};

use system_info_probe::process::{DetailRefresh, ProcessDetail, ProcessSampler, ProcessSamplerConfig};

const ROUNDS: u32 = 40;

fn time(mut sample: impl FnMut()) -> Duration {
    sample();
    let start = Instant::now();
    for _ in 0..ROUNDS {
        sample();
    }
    start.elapsed() / ROUNDS
}

fn probe(detail: ProcessDetail) -> Duration {
    let mut sampler = ProcessSampler::new(ProcessSamplerConfig {
        detail,
        detail_scope: None,
        refresh: DetailRefresh::default(),
    })
    .expect("sampler");
    time(|| {
        sampler.sample().expect("sample");
    })
}

fn main() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let refresh = ProcessRefreshKind::nothing()
        .without_tasks()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet);
    let mut sys = System::new();
    let sysinfo = time(|| {
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
    });
    let processes = sys.processes().len();
    let basic = probe(ProcessDetail::Basic);
    let full = probe(ProcessDetail::Full);
    let report = format!("{processes} processes, per sample: sysinfo {sysinfo:?}, probe basic {basic:?}, probe io+status (idle skipped) {full:?}");
    std::fs::write(std::env::temp_dir().join("probe_sample_cost.txt"), report).expect("write report");
}
