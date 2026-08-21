#![allow(clippy::collapsible_else_if)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::too_many_arguments)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::print_stderr)]
#![warn(clippy::print_stdout)]
#![warn(clippy::dbg_macro)]
#![warn(clippy::string_slice)]

use std::sync::atomic::Ordering;
use std::thread::available_parallelism;
use std::{env, process};

use handsome_logger::{ColorChoice, ConfigBuilder, TermLogger, TerminalMode};
use log::{error, info, warn};
use sysinfo::System;
use system_info_collector_core::discovery::{DiscoveredDisk, DiscoveredInterface, list_real_disks, list_real_interfaces};
use system_info_collector_core::engine::CollectorEngine;
use system_info_collector_core::enums::{DataType, SimpleDataCollectionMode, network_rate_columns};
use system_info_collector_core::settings::MAX_BUFFER_SAMPLES;
use system_info_collector_core::workers::file_writer::top_n_path;
use system_info_collector_core::workers::sysinfo_worker::bytes_to_mb;

use crate::cli::{Commands, parse_cli};
use crate::converting::ploty_creator::load_results_and_save_plot;
use crate::serving::data_buffer::{DataBuffer, DataPoint, SystemInfo, SystemMetadata};
use crate::serving::server::ExportPaths;
use crate::settings::{build_collect_settings, build_convert_settings};

mod cli;
mod converting;
mod serving;
mod settings;

// The collector runs a handful of periodic tasks that spend nearly all their
// time asleep, so the default "one worker per core" runtime is pure waste -
// on a 80-core machine it would spawn 80 threads to do the work of two.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let _ = TermLogger::init(ConfigBuilder::default().build(), TerminalMode::Mixed, ColorChoice::Auto);

    print_version_mode();

    let args = parse_cli();

    match args.command {
        Commands::Collect(collect_args) => {
            // Handle --list-* flags before doing anything else.
            if collect_args.list_disks {
                list_real_disks(&collect_args.disk_exclude);
                return;
            }
            if collect_args.list_networks {
                list_real_interfaces(&collect_args.network_exclude);
                return;
            }

            let settings = std::sync::Arc::new(build_collect_settings(collect_args));
            let convert_settings = settings.convert.clone();
            let convert_after = settings.convert_after;

            // Always collect and log system hardware info.
            let mut meta_sys = System::new();
            meta_sys.refresh_memory();
            meta_sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());

            let cpu_logical_cores = meta_sys.cpus().len();
            let cpu_physical_cores = sysinfo::System::physical_core_count().unwrap_or(0);
            let cpu_model = meta_sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default();
            let total_memory_mb = bytes_to_mb(meta_sys.total_memory());
            let total_swap_mb = bytes_to_mb(meta_sys.total_swap());

            info!("CPU: {cpu_model}, {cpu_physical_cores} physical cores / {cpu_logical_cores} threads");
            info!("Memory: {total_memory_mb:.0} MB total RAM, {total_swap_mb:.0} MB swap");

            // Create the engine — this runs hardware discovery exactly once.
            let engine = CollectorEngine::new(std::sync::Arc::clone(&settings));

            let gpu_names: Vec<String> = engine.discovery().gpus.iter().map(|g| g.display_name().to_string()).collect();
            let gpu_vram_mb: Vec<u64> = engine.discovery().gpus.iter().map(|g| g.vram_total_mb).collect();
            if gpu_names.is_empty() {
                info!("GPU: none detected");
            } else {
                info!("GPU: {}", gpu_names.join(", "));
            }

            let shutdown = engine.shutdown_handle();

            // Build the HTTP data buffer before starting the engine so we can
            // pass a clone to the on_row callback.
            let data_buffer: Option<DataBuffer> = if settings.serve {
                let buffer_capacity = settings.buffer_capacity();
                let covered_hours = buffer_capacity as f32 * settings.check_interval / 3600.0;
                info!(
                    "Live view buffer: {buffer_capacity} samples (~{covered_hours:.1} h at {}s interval)",
                    settings.check_interval
                );
                if (buffer_capacity as f32 * settings.check_interval) < settings.buffer_seconds - 1.0 {
                    warn!(
                        "Requested {}s of live history needs more than {MAX_BUFFER_SAMPLES} samples - capped to ~{covered_hours:.1} h",
                        settings.buffer_seconds
                    );
                }
                let buffer = DataBuffer::new(buffer_capacity);

                let interfaces = engine.discovery().interfaces.clone();
                let disks = engine.discovery().disks.clone();

                // Mirror the expanded per-GPU / per-interface column layout produced by
                // file_writer, so the chart detector in the web UI finds the columns.
                let mut data_types = vec![DataType::SECONDS_SINCE_START];
                let has_rx = settings.collection_mode.contains(&SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC);
                let has_tx = settings.collection_mode.contains(&SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC);
                let mut network_rate_emitted = false;
                for mode in &settings.collection_mode {
                    match mode {
                        SimpleDataCollectionMode::GPU_UTILIZATION => {
                            for (idx, name) in gpu_names.iter().enumerate() {
                                data_types.push(DataType::GPU_N_UTIL((idx, name.clone())));
                            }
                        }
                        SimpleDataCollectionMode::GPU_MEMORY_USED => {
                            for (idx, name) in gpu_names.iter().enumerate() {
                                data_types.push(DataType::GPU_N_VRAM_MB((idx, name.clone())));
                            }
                        }
                        SimpleDataCollectionMode::GPU_TEMPERATURE => {
                            for (idx, name) in gpu_names.iter().enumerate() {
                                data_types.push(DataType::GPU_N_TEMP_C((idx, name.clone())));
                            }
                        }
                        SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC | SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC => {
                            if !network_rate_emitted {
                                network_rate_emitted = true;
                                let ifaces = interfaces.iter().map(|iface| (iface.iface_index, iface.display_label()));
                                data_types.extend(network_rate_columns(has_rx, has_tx, ifaces));
                            }
                        }
                        SimpleDataCollectionMode::NETWORK_TOTAL => {
                            for iface in &interfaces {
                                data_types.push(DataType::NET_N_RX_TOTAL_MB((iface.iface_index, iface.display_label())));
                                data_types.push(DataType::NET_N_TX_TOTAL_MB((iface.iface_index, iface.display_label())));
                            }
                        }
                        SimpleDataCollectionMode::DISK_USED => {
                            for disk in &disks {
                                data_types.push(DataType::DISK_N_USED_GB((disk.disk_index, disk.display_label())));
                            }
                        }
                        SimpleDataCollectionMode::DISK_AVAILABLE => {
                            for disk in &disks {
                                data_types.push(DataType::DISK_N_AVAIL_GB((disk.disk_index, disk.display_label())));
                            }
                        }
                        SimpleDataCollectionMode::DISK_BUSY => {
                            for disk in &disks {
                                data_types.push(DataType::DISK_N_BUSY_PCT((disk.disk_index, disk.display_label())));
                            }
                        }
                        SimpleDataCollectionMode::DISK_READ => {
                            for disk in &disks {
                                data_types.push(DataType::DISK_N_READ_MBPS((disk.disk_index, disk.display_label())));
                            }
                        }
                        SimpleDataCollectionMode::DISK_WRITE => {
                            for disk in &disks {
                                data_types.push(DataType::DISK_N_WRITE_MBPS((disk.disk_index, disk.display_label())));
                            }
                        }
                        SimpleDataCollectionMode::CPU_USAGE_TOTAL => data_types.push(DataType::CPU_USAGE_TOTAL),
                        SimpleDataCollectionMode::CPU_USAGE_PER_CORE => data_types.push(DataType::CPU_USAGE_PER_CORE),
                        SimpleDataCollectionMode::SWAP_FREE => data_types.push(DataType::SWAP_FREE),
                        SimpleDataCollectionMode::SWAP_USED => data_types.push(DataType::SWAP_USED),
                        SimpleDataCollectionMode::MEMORY_USED => data_types.push(DataType::MEMORY_USED),
                        SimpleDataCollectionMode::MEMORY_FREE => data_types.push(DataType::MEMORY_FREE),
                        SimpleDataCollectionMode::MEMORY_AVAILABLE => data_types.push(DataType::MEMORY_AVAILABLE),
                    }
                }
                for (idx, p) in settings.process_cmd_to_search.iter().enumerate() {
                    data_types.push(DataType::CUSTOM_CPU((idx, p.graph_name.clone())));
                    data_types.push(DataType::CUSTOM_MEMORY((idx, p.graph_name.clone())));
                }

                // The web UI groups columns into charts by their canonical CSV name, and
                // shows the readable label next to the data.
                let mut column_headers = vec!["Timestamp".to_string()];
                column_headers.extend(data_types.iter().skip(1).map(DataType::column_name));

                let mut column_labels = vec!["Timestamp".to_string()];
                column_labels.extend(data_types.iter().skip(1).map(DataType::pretty_print));

                let metadata = SystemMetadata {
                    system_info: SystemInfo {
                        total_memory_mb,
                        total_swap_mb,
                        cpu_cores: cpu_logical_cores,
                        cpu_physical_cores,
                        cpu_model: cpu_model.clone(),
                        gpu_names: gpu_names.clone(),
                        gpu_vram_mb: gpu_vram_mb.clone(),
                        disk_labels: disks.iter().map(DiscoveredDisk::display_label).collect(),
                        net_labels: interfaces.iter().map(DiscoveredInterface::display_label).collect(),
                        start_time: settings.start_time,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    column_headers,
                    column_labels,
                    max_buffer_size: buffer_capacity,
                    check_interval: settings.check_interval,
                };
                buffer.set_metadata(metadata);

                // Reports are rendered from the CSV on disk, so the server needs to know
                // where this run writes it.
                let export_paths = ExportPaths::new(
                    settings.convert.data_path.clone(),
                    if settings.top_n_processes > 0 {
                        vec![
                            top_n_path(&settings.convert.data_path, "cpu"),
                            top_n_path(&settings.convert.data_path, "ram"),
                        ]
                    } else {
                        vec![]
                    },
                );

                // Start the HTTP server in its own OS thread with an independent
                // Tokio runtime so it never blocks data collection.
                let server_buffer = buffer.clone();
                let port = settings.port;
                std::thread::spawn(move || {
                    info!("Starting HTTP server thread on port {port}");
                    let runtime = crate::serving::server::build_runtime().expect("Failed to create Tokio runtime for server");
                    runtime.block_on(async move {
                        if let Err(e) = crate::serving::server::start_server(port, server_buffer, export_paths).await {
                            error!("Server error: {e}");
                        }
                    });
                });

                Some(buffer)
            } else {
                None
            };
            // Build top-N live callback (only when serve is enabled).
            let on_top_row: Option<std::sync::Arc<dyn Fn(f64, Vec<(String, f32)>, Vec<(String, f64)>) + Send + Sync>> =
                if let Some(ref buf) = data_buffer {
                    let top_buf = buf.clone();
                    Some(std::sync::Arc::new(move |ts, cpu, ram| {
                        top_buf.add_top_point(ts, cpu, ram);
                    }))
                } else {
                    None
                };

            let _ = (cpu_model, gpu_names, gpu_vram_mb); // suppress unused warnings when !serve

            // Register Ctrl-C handler: first press → graceful stop, second → immediate exit.
            let shutdown_for_ctrlc = shutdown.clone();
            let mut ctrlc_count = 0u32;
            ctrlc::set_handler(move || {
                ctrlc_count += 1;
                if ctrlc_count == 1 {
                    info!("Trying to close app cleanly, press Ctrl-C again to force quit");
                    shutdown_for_ctrlc.store(true, Ordering::Relaxed);
                } else {
                    info!("Forcing quit");
                    process::exit(1);
                }
            })
            .expect("Error setting Ctrl-C handler");

            if let Err(e) = engine
                .run(
                    env!("CARGO_PKG_VERSION"),
                    move |row| {
                        if let Some(ref buf) = data_buffer {
                            buf.add_data_point(DataPoint::from_row(row));
                        }
                    },
                    on_top_row,
                )
                .await
            {
                error!("{e}");
                process::exit(1);
            }

            if convert_after && let Err(e) = load_results_and_save_plot(&convert_settings) {
                error!("{e}");
                process::exit(1);
            }
        }

        Commands::Convert(convert_args) => {
            let settings = build_convert_settings(convert_args);
            if let Err(e) = load_results_and_save_plot(&settings) {
                error!("{e}");
                process::exit(1);
            }
        }
    }

    info!("Closing app successfully");
}

// This is unused depending on build features
#[allow(clippy::allow_attributes)]
#[allow(unused_mut)]
pub fn print_version_mode() {
    let debug_release = if cfg!(debug_assertions) { "debug" } else { "release" };
    let processors = available_parallelism().map(std::num::NonZero::get).unwrap_or(1);
    let info = os_info::get();

    let mut app_cpu_version = "Baseline";
    let mut os_cpu_version = "Baseline";
    if cfg!(target_feature = "sse2") {
        app_cpu_version = "x86-64-v1 (SSE2)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse2") {
        os_cpu_version = "x86-64-v1 (SSE2)";
    }
    if cfg!(target_feature = "popcnt") {
        app_cpu_version = "x86-64-v2 (SSE4.2 + POPCNT)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("popcnt") {
        os_cpu_version = "x86-64-v2 (SSE4.2 + POPCNT)";
    }
    if cfg!(target_feature = "avx2") {
        app_cpu_version = "x86-64-v3 (AVX2)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("avx2") {
        os_cpu_version = "x86-64-v3 (AVX2)";
    }
    if cfg!(target_feature = "avx512f") {
        app_cpu_version = "x86-64-v4 (AVX-512)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("avx512f") {
        os_cpu_version = "x86-64-v4 (AVX-512)";
    }

    info!(
        "System Info Collector, version: {}, {debug_release} mode, os {} {} ({} {}), {processors} cpu/threads, app cpu: {app_cpu_version}, os cpu: {os_cpu_version}",
        env!("CARGO_PKG_VERSION"),
        info.os_type(),
        info.version(),
        env::consts::ARCH,
        info.bitness(),
    );
    info!("Process ID is {}", process::id());

    if cfg!(debug_assertions) {
        warn!("You are running debug version of app which is a lot of slower than release version.");
    }
}
