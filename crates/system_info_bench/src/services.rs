use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use system_info_report::benchmark::{RunHooks, Variant};

use crate::config::{ServiceSpec, substitute};

/// Starts the services before every run and kills them, with everything they started, after it.
pub struct Services {
    specs: Vec<ServiceSpec>,
    running: Mutex<Vec<Child>>,
}

impl Services {
    pub fn new(specs: Vec<ServiceSpec>) -> Self {
        Self {
            specs,
            running: Mutex::new(Vec::new()),
        }
    }
}

fn values(variant: &Variant, repeat: usize) -> Vec<(String, String)> {
    let mut values = vec![
        ("name".to_string(), variant.name.clone()),
        ("repeat".to_string(), (repeat + 1).to_string()),
    ];
    values.extend(variant.dimensions.iter().cloned());
    values
}

impl RunHooks for Services {
    fn before(&self, variant: &Variant, repeat: usize) -> Result<(), String> {
        let values = values(variant, repeat);
        for spec in &self.specs {
            let command = substitute(&spec.command, &values)?;
            let mut builder = Command::new("sh");
            builder
                .arg("-c")
                .arg(&command)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut builder, 0);
            let child = builder.spawn().map_err(|e| format!("cannot start service {command:?}: {e}"))?;
            self.running.lock().map_err(|_| "service list poisoned".to_string())?.push(child);
            if let Some(settle) = spec.settle {
                std::thread::sleep(settle.0);
            }
        }
        Ok(())
    }

    fn after(&self, _variant: &Variant, _repeat: usize) {
        let Ok(mut running) = self.running.lock() else { return };
        for mut child in running.drain(..) {
            #[cfg(unix)]
            // SAFETY: plain signal delivery; the negative pid addresses the service's own process group.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
