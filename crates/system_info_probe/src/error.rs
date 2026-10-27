use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("cannot read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("cannot parse {path}: {detail}")]
    Parse { path: PathBuf, detail: String },
    #[error("cannot start {program}: {source}")]
    Spawn { program: String, source: std::io::Error },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("the sampling thread panicked")]
    WorkerPanicked,
    #[cfg(feature = "gpu-nvidia")]
    #[error("NVML: {0}")]
    Nvml(#[from] nvml_wrapper::error::NvmlError),
}

pub type ProbeResult<T> = Result<T, ProbeError>;

impl ProbeError {
    #[cfg(target_os = "linux")]
    pub(crate) fn read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Read { path: path.into(), source }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn parse(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self::Parse {
            path: path.into(),
            detail: detail.into(),
        }
    }
}
