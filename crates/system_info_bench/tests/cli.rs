//! The binary end to end, on real programs.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_system_info_bench"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bench-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("utf8")
}

fn column(csv: &str, row: usize, header: &str) -> String {
    let headers: Vec<&str> = csv.lines().next().expect("header").split(',').collect();
    let index = headers
        .iter()
        .position(|h| *h == header)
        .unwrap_or_else(|| panic!("no {header} in {headers:?}"));
    csv.lines().nth(row).expect("row").split(',').nth(index).expect("cell").to_string()
}

#[test]
fn run_measures_a_command_and_reads_a_metric() {
    let output = bench(&[
        "run",
        "--duration",
        "1s",
        "--interval",
        "100ms",
        "--format",
        "csv",
        "--processes",
        "0",
        "--metric",
        r"passes=passes=(\d+)",
        "--",
        "sh",
        "-c",
        "timeout 3 sh -c 'while :; do :; done' & echo passes=7; wait",
    ]);
    let csv = stdout(&output);
    let cpu: f64 = column(&csv, 1, "cpu [%]").parse().expect("number");
    assert!((80.0..=110.0).contains(&cpu), "{csv}");
    assert_eq!(column(&csv, 1, "passes"), "7");
    assert_eq!(column(&csv, 1, "variant"), "sh");
}

#[test]
fn matrix_expands_runs_services_and_writes_files() {
    let dir = temp_dir("matrix");
    let marker = dir.join("services");
    std::fs::create_dir_all(&marker).expect("dir");
    let config = format!(
        r#"
repeats = 1
warmup = "200ms"
duration = "1s"
interval = "100ms"

[matrix]
load = ["1", "2"]

[[variant]]
name = "busy"
program = "sh"
args = ["-c", "for i in $(seq {{load}}); do timeout 2 sh -c 'while :; do :; done' & done; echo frames=10 t=1; echo frames=30 t=2; wait"]

[[metric]]
name = "fps"
pattern = 'frames=(\d+) t=([\d.]+)'
extract = "rate"

[[service]]
command = "echo up > {marker}/{{load}}; exec sleep 37.25"
settle = "50ms"
"#,
        marker = marker.display()
    );
    let file = dir.join("bench.toml");
    std::fs::write(&file, config).expect("write config");
    let svg = dir.join("svg");
    let csv = dir.join("csv");
    let output = bench(&[
        "matrix",
        "--format",
        "csv",
        "--svg",
        svg.to_str().expect("utf8"),
        "--csv",
        csv.to_str().expect("utf8"),
        file.to_str().expect("utf8"),
    ]);
    let table = stdout(&output);
    assert_eq!(table.lines().count(), 3, "{table}");
    assert!(table.lines().next().expect("header").starts_with("variant,load,cpu [%]"));
    let two: f64 = column(&table, 2, "cpu [%]").parse().expect("number");
    assert!((170.0..=210.0).contains(&two), "{table}");
    assert_eq!(column(&table, 1, "fps"), "20");

    for name in ["busy-1", "busy-2"] {
        assert!(svg.join(format!("{name}.svg")).exists(), "{name}.svg");
        assert!(csv.join(format!("{name}.csv")).exists(), "{name}.csv");
    }
    assert!(
        marker.join("1").exists() && marker.join("2").exists(),
        "services ran with the run's placeholders"
    );
    assert!(!sleep_alive("37.25"), "services are killed after their run");
    let _ = std::fs::remove_dir_all(&dir);
}

fn sleep_alive(argument: &str) -> bool {
    std::fs::read_dir("/proc").expect("proc").flatten().any(|entry| {
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        String::from_utf8_lossy(&cmdline).contains(argument)
    })
}

#[test]
fn attach_follows_a_running_process() {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("timeout 3 sh -c 'while :; do :; done'; true")
        .spawn()
        .expect("spawns");
    let output = bench(&[
        "attach",
        "--duration",
        "600ms",
        "--interval",
        "100ms",
        "--format",
        "csv",
        &child.id().to_string(),
    ]);
    let _ = child.kill();
    let _ = child.wait();
    let csv = stdout(&output);
    assert_eq!(column(&csv, 1, "group"), "target");
    let cpu: f64 = column(&csv, 1, "cpu avg [%]").parse().expect("number");
    assert!(cpu > 50.0, "{csv}");
}

#[test]
fn bad_files_are_reported_with_exit_code_two() {
    let dir = temp_dir("bad");
    let file = dir.join("bad.toml");
    std::fs::write(
        &file,
        "duration = \"1s\"\n[[variant]]\nname = \"x\"\nprogram = \"x\"\nargs = [\"{nope}\"]\n",
    )
    .expect("write");
    let output = bench(&["matrix", file.to_str().expect("utf8")]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown placeholder {nope}"));
    let _ = std::fs::remove_dir_all(Path::new(&dir));
}
