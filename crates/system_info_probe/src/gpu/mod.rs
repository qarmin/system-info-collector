//! GPU utilization, memory and temperature.  Each vendor sits behind its own feature.

#[cfg(all(target_os = "linux", feature = "gpu-amd"))]
mod amd;
#[cfg(all(target_os = "linux", feature = "gpu-intel"))]
mod intel;
#[cfg(feature = "gpu-nvidia")]
mod nvidia;
#[cfg(all(target_os = "linux", feature = "gpu-vivante"))]
pub mod vivante;

use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GpuKind {
    Nvidia {
        nvml_index: u32,
    },
    /// `/sys/class/drm/cardN/device`.
    Amd {
        device_path: PathBuf,
    },
    Intel {
        device_path: PathBuf,
    },
    /// NXP i.MX `galcore` driver, read through the 3D core's idle register.
    Vivante {
        register_base: u64,
    },
}

impl GpuKind {
    pub fn vendor(&self) -> &'static str {
        match self {
            Self::Nvidia { .. } => "NVIDIA",
            Self::Amd { .. } => "AMD (Linux sysfs)",
            Self::Intel { .. } => "Intel (Linux sysfs)",
            Self::Vivante { .. } => "Vivante (galcore)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Gpu {
    pub name: String,
    pub kind: GpuKind,
    pub vram_total_bytes: Option<u64>,
    /// Driver and its version, e.g. `galcore 6.4.3.p2.336687` or `amdgpu (kernel 6.8.0)`.
    pub driver_version: Option<String>,
}

/// Every field is `None` where the vendor or driver does not expose it - Intel integrated GPUs
/// have no dedicated memory and usually no temperature sensor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GpuReading {
    pub utilization_percent: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub temperature_celsius: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuDiscovery {
    /// Reading the Vivante idle register through `/dev/mem` while `galcore` has the GPU
    /// power-gated hangs the whole SoC, so it stays off unless the board is known to be safe.
    pub allow_vivante_register_reads: bool,
}

/// NVIDIA first (NVML), then AMD and Intel from sysfs, then Vivante; only vendors whose feature
/// is enabled are looked for.
pub fn discover(options: GpuDiscovery) -> Vec<Gpu> {
    #[cfg_attr(
        not(any(
            feature = "gpu-nvidia",
            all(target_os = "linux", any(feature = "gpu-amd", feature = "gpu-intel", feature = "gpu-vivante"))
        )),
        expect(unused_mut)
    )]
    let mut gpus = Vec::new();
    #[cfg(feature = "gpu-nvidia")]
    nvidia::discover(&mut gpus);
    #[cfg(all(target_os = "linux", any(feature = "gpu-amd", feature = "gpu-intel")))]
    discover_drm(&mut gpus);
    #[cfg(all(target_os = "linux", feature = "gpu-vivante"))]
    if options.allow_vivante_register_reads {
        vivante::discover(&mut gpus);
    }
    let _ = options;
    gpus
}

#[cfg(all(target_os = "linux", any(feature = "gpu-amd", feature = "gpu-intel")))]
fn discover_drm(gpus: &mut Vec<Gpu>) {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else { return };
    // Only "cardN" - not "cardN-HDMI-A-1" connectors or "renderDN".
    let mut cards: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path())
        .collect();
    cards.sort();
    for card in cards {
        let device_path = card.join("device");
        let Ok(vendor) = std::fs::read_to_string(device_path.join("vendor")) else {
            continue;
        };
        let card_name = card.file_name().unwrap_or_default().to_string_lossy().into_owned();
        match vendor.trim().to_lowercase().as_str() {
            #[cfg(feature = "gpu-amd")]
            "0x1002" => amd::discover(gpus, device_path, &card_name),
            #[cfg(feature = "gpu-intel")]
            "0x8086" => gpus.push(Gpu {
                name: crate::pci::device_name(&device_path).unwrap_or_else(|| format!("Intel GPU ({card_name})")),
                driver_version: kernel_driver_version(&device_path),
                kind: GpuKind::Intel { device_path },
                vram_total_bytes: None,
            }),
            _ => {}
        }
    }
}

/// In-tree drivers (amdgpu, i915, xe) carry no version of their own - they ship with the kernel.
#[cfg(all(target_os = "linux", any(feature = "gpu-amd", feature = "gpu-intel")))]
fn kernel_driver_version(device_path: &std::path::Path) -> Option<String> {
    let driver = crate::pci::driver_name(device_path)?;
    let module_version = std::fs::read_to_string(format!("/sys/module/{driver}/version")).ok();
    Some(match (module_version, sysinfo::System::kernel_version()) {
        (Some(version), _) => format!("{driver} {}", version.trim()),
        (None, Some(kernel)) => format!("{driver} (kernel {kernel})"),
        (None, None) => driver,
    })
}

/// Readings of the given GPUs, aligned with [`GpuSampler::gpus`].
#[derive(Clone, Debug)]
pub struct GpuSample {
    pub taken_at: Instant,
    pub readings: Vec<Option<GpuReading>>,
}

/// Utilization from Intel RC6 residency and the Vivante idle register is a share of the time
/// since the previous sample, so the first sample reports it only once a window exists.
pub struct GpuSampler {
    gpus: Vec<Gpu>,
    readers: Vec<Reader>,
    #[cfg(feature = "gpu-nvidia")]
    nvml: Option<nvml_wrapper::Nvml>,
}

enum Reader {
    #[cfg(feature = "gpu-nvidia")]
    Nvidia(u32),
    #[cfg(all(target_os = "linux", feature = "gpu-amd"))]
    Amd(Box<amdgpu_sysfs::gpu_handle::GpuHandle>),
    #[cfg(all(target_os = "linux", feature = "gpu-intel"))]
    Intel(intel::IntelReader),
    #[cfg(all(target_os = "linux", feature = "gpu-vivante"))]
    Vivante(vivante::VivanteReader),
    Unavailable,
}

impl GpuSampler {
    pub fn new(gpus: Vec<Gpu>) -> Self {
        #[cfg(feature = "gpu-nvidia")]
        let nvml = gpus
            .iter()
            .any(|g| matches!(g.kind, GpuKind::Nvidia { .. }))
            .then(|| {
                nvml_wrapper::Nvml::init()
                    .inspect_err(|e| log::warn!("NVML init failed ({e}), NVIDIA readings disabled"))
                    .ok()
            })
            .flatten();
        let readers = gpus.iter().map(Reader::open).collect();
        Self {
            gpus,
            readers,
            #[cfg(feature = "gpu-nvidia")]
            nvml,
        }
    }

    pub fn gpus(&self) -> &[Gpu] {
        &self.gpus
    }

    pub fn sample(&mut self) -> GpuSample {
        let taken_at = Instant::now();
        let readings = self
            .readers
            .iter_mut()
            .map(|reader| match reader {
                #[cfg(feature = "gpu-nvidia")]
                Reader::Nvidia(index) => nvidia::read(self.nvml.as_ref()?, *index),
                #[cfg(all(target_os = "linux", feature = "gpu-amd"))]
                Reader::Amd(handle) => Some(amd::read(handle)),
                #[cfg(all(target_os = "linux", feature = "gpu-intel"))]
                Reader::Intel(reader) => Some(reader.read(taken_at)),
                #[cfg(all(target_os = "linux", feature = "gpu-vivante"))]
                Reader::Vivante(reader) => Some(reader.read()),
                Reader::Unavailable => None,
            })
            .collect();
        GpuSample { taken_at, readings }
    }
}

impl Reader {
    fn open(gpu: &Gpu) -> Self {
        match &gpu.kind {
            #[cfg(feature = "gpu-nvidia")]
            GpuKind::Nvidia { nvml_index } => Self::Nvidia(*nvml_index),
            #[cfg(all(target_os = "linux", feature = "gpu-amd"))]
            GpuKind::Amd { device_path } => {
                amdgpu_sysfs::gpu_handle::GpuHandle::new_from_path(device_path.clone()).map_or(Self::Unavailable, |h| Self::Amd(Box::new(h)))
            }
            #[cfg(all(target_os = "linux", feature = "gpu-intel"))]
            GpuKind::Intel { device_path } => Self::Intel(intel::IntelReader::new(device_path)),
            #[cfg(all(target_os = "linux", feature = "gpu-vivante"))]
            GpuKind::Vivante { register_base } => vivante::VivanteReader::start(*register_base)
                .inspect_err(|e| log::warn!("Vivante idle register at {register_base:#x} unavailable ({e}), utilization disabled"))
                .map_or(Self::Unavailable, Self::Vivante),
            #[cfg_attr(
                all(
                    feature = "gpu-nvidia",
                    target_os = "linux",
                    feature = "gpu-amd",
                    feature = "gpu-intel",
                    feature = "gpu-vivante"
                ),
                expect(unreachable_patterns)
            )]
            _ => Self::Unavailable,
        }
    }
}
