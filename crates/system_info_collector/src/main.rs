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
use system_info_collector_core::engine::CollectorEngine;
use system_info_collector_core::workers::sysinfo_worker::bytes_to_mb;

use crate::cli::{parse_cli, Commands};
use crate::converting::ploty_creator::load_results_and_save_plot;
use crate::serving::data_buffer::{DataBuffer, DataPoint, SystemInfo, SystemMetadata};
use crate::settings::{build_collect_settings, build_convert_settings};

mod cli;
mod converting;
mod serving;
mod settings;

#[tokio::main]
async fn main() {
    let _ = TermLogger::init(ConfigBuilder::default().build(), TerminalMode::Mixed, ColorChoice::Auto);

    print_version_mode();

    let args = parse_cli();

    match args.command {
        Commands::Collect(collect_args) => {
            let settings = build_collect_settings(collect_args);
            let convert_settings = settings.convert.clone();
            let convert_after = settings.convert_after;

            // Build the HTTP data buffer before starting the engine so we can
            // pass a clone to the on_row callback.
            let data_buffer: Option<DataBuffer> = if settings.serve {
                let buffer = DataBuffer::new(settings.max_results);

                // Populate metadata with a quick System query.
                let mut meta_sys = System::new();
                meta_sys.refresh_memory();
                meta_sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());

                let mut column_headers = vec!["Timestamp".to_string()];
                for mode in &settings.collection_mode {
                    column_headers.push(mode.to_string());
                }
                for p in &settings.process_cmd_to_search {
                    column_headers.push(format!("{} CPU", p.graph_name));
                    column_headers.push(format!("{} Memory", p.graph_name));
                }

                let metadata = SystemMetadata {
                    system_info: SystemInfo {
                        total_memory_mb: bytes_to_mb(meta_sys.total_memory()),
                        total_swap_mb: bytes_to_mb(meta_sys.total_swap()),
                        cpu_cores: meta_sys.cpus().len(),
                        start_time: settings.start_time,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    column_headers,
                    max_buffer_size: settings.max_results,
                };
                buffer.set_metadata(metadata);

                // Start the HTTP server in its own OS thread with an independent
                // Tokio runtime so it never blocks data collection.
                let server_buffer = buffer.clone();
                let port = settings.port;
                std::thread::spawn(move || {
                    info!("Starting HTTP server thread on port {port}");
                    let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime for server");
                    runtime.block_on(async move {
                        if let Err(e) = crate::serving::server::start_server(port, server_buffer).await {
                            error!("Server error: {e}");
                        }
                    });
                });

                Some(buffer)
            } else {
                None
            };

            let engine = CollectorEngine::new(settings);
            let shutdown = engine.shutdown_handle();

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
                .run(env!("CARGO_PKG_VERSION"), move |row| {
                    if let Some(ref buf) = data_buffer {
                        buf.add_data_point(DataPoint::from_row(&row));
                    }
                })
                .await
            {
                error!("{e}");
                process::exit(1);
            }

            if convert_after {
                if let Err(e) = load_results_and_save_plot(&convert_settings) {
                    error!("{e}");
                    process::exit(1);
                }
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
