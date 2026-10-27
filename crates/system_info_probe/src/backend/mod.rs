#[cfg(target_os = "linux")]
pub mod procfs;

#[cfg(target_os = "linux")]
use std::io::Read as _;
#[cfg(target_os = "linux")]
use std::path::Path;

/// Reads a whole file into a buffer kept by the caller.  `std::fs::read_to_string` would allocate a
/// fresh string per call, which on a few hundred `/proc` files per sample is most of the work.
#[cfg(target_os = "linux")]
pub(crate) fn read_text_into(path: impl AsRef<Path>, buffer: &mut String) -> std::io::Result<()> {
    buffer.clear();
    std::fs::File::open(path)?.read_to_string(buffer).map(drop)
}

#[cfg(target_os = "linux")]
pub(crate) fn read_bytes_into(path: impl AsRef<Path>, buffer: &mut Vec<u8>) -> std::io::Result<()> {
    buffer.clear();
    std::fs::File::open(path)?.read_to_end(buffer).map(drop)
}
