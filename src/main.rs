#![allow(clippy::collapsible_else_if)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::too_many_arguments)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::print_stderr)]
#![warn(clippy::print_stdout)]
#![warn(clippy::dbg_macro)]
#![warn(clippy::string_slice)]

use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use crossbeam_channel::Sender;
use handsome_logger::{ColorChoice, ConfigBuilder, TermLogger, TerminalMode};
use log::{error, info};
use sysinfo::{ProcessesToUpdate, System};

use crate::cli::{parse_cli, Commands};
use crate::collecting::collector::collect_data;
use crate::converting::ploty_creator::load_results_and_save_plot;
use crate::settings::{CollectSettings, ConvertSettings};

mod cli;
mod collecting;
mod converting;
mod enums;
mod model;
mod settings;

#[tokio::main]
async fn main() {
    let _ = TermLogger::init(ConfigBuilder::default().build(), TerminalMode::Mixed, ColorChoice::Auto);

    let args = parse_cli();

    match args.command {
        Commands::Collect(collect_args) => {
            let settings: CollectSettings = collect_args.into();

            let creating_start_time = Instant::now();
            let mut sys = System::new_all();
            let creating_duration = creating_start_time.elapsed();
            let refresh_start_time = Instant::now();
            sys.refresh_memory();
            sys.refresh_cpu_all();
            if settings.need_to_refresh_processes {
                sys.refresh_processes(ProcessesToUpdate::All, true);
            }
            info!(
                "Initial refresh took {:?} (creating sys struct took {:?})",
                refresh_start_time.elapsed(),
                creating_duration
            );

            if let Err(e) = collect_data(&mut sys, &settings).await {
                error!("{e}");
                process::exit(1);
            };

            // Convert after collecting if enabled
            if settings.convert_after {
                if let Err(e) = load_results_and_save_plot(&settings.convert) {
                    error!("{e}");
                    process::exit(1);
                }
            }
        }
        Commands::Convert(convert_args) => {
            let settings: ConvertSettings = convert_args.into();

            // Only convert
            if let Err(e) = load_results_and_save_plot(&settings) {
                error!("{e}");
                process::exit(1);
            }
        }
    }
    info!("Closing app successfully");
}

pub fn set_ctrl_c_handler(ctx: Sender<()>) {
    let current_ctrl_c = AtomicU32::new(1);
    ctrlc::set_handler(move || {
        ctx.send(()).expect("Could not send signal on channel.");
        if current_ctrl_c.fetch_sub(1, Ordering::SeqCst) == 0 {
            info!("Closing app due clicking Ctrl-C multiple times");
            process::exit(1);
        } else {
            info!("Trying to close app cleanly, if you don't want to wait, click Ctrl-C again");
        }
    })
    .expect("Error when setting Ctrl-C handler");
}
