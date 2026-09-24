//! On-disk home for recorded sessions, shared by the `session` command and the
//! HTTP server so both produce and read the same files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use chrono::Local;
use log::info;
use serde::Serialize;
use system_info_collector_core::session_recorder::{PREFIX_READ_BYTES, SessionData, SessionMeta, parse_meta_prefix};

pub const DEFAULT_SESSION_DIR: &str = "sessions";

const FILE_PREFIX: &str = "session_";
const FILE_SUFFIX: &str = ".json";

#[derive(Serialize)]
pub struct SessionListEntry {
    pub name: String,
    pub size_bytes: u64,
    /// `None` when the file is unreadable or was written by an incompatible
    /// version - the entry is still listed so it is visible rather than silently
    /// missing.
    pub meta: Option<SessionMeta>,
}

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, data: &SessionData) -> Result<PathBuf, Error> {
        std::fs::create_dir_all(&self.dir).context(format!("Failed to create session directory {}", self.dir.display()))?;
        let path = self
            .dir
            .join(format!("{FILE_PREFIX}{}{FILE_SUFFIX}", Local::now().format("%Y-%m-%d_%H-%M-%S")));
        write_session(&path, data)?;
        Ok(path)
    }

    /// Newest first, so the UI lists the recording you just made at the top.
    pub fn list(&self) -> Vec<SessionListEntry> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<SessionListEntry> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !is_session_file_name(&name) {
                    return None;
                }
                Some(SessionListEntry {
                    size_bytes: entry.metadata().map_or(0, |m| m.len()),
                    meta: read_meta(&entry.path()),
                    name,
                })
            })
            .collect();
        out.sort_unstable_by(|a, b| b.name.cmp(&a.name));
        out
    }

    /// Resolves a name coming from an HTTP request.  Only bare file names
    /// matching the pattern this store writes are accepted, so a request cannot
    /// reach outside the session directory.
    pub fn path_for(&self, name: &str) -> Result<PathBuf, Error> {
        if !is_session_file_name(name) {
            return Err(Error::msg(format!("Not a session file name: {name:?}")));
        }
        Ok(self.dir.join(name))
    }

    pub fn read_raw(&self, name: &str) -> Result<Vec<u8>, Error> {
        let path = self.path_for(name)?;
        std::fs::read(&path).context(format!("Failed to read {}", path.display()))
    }
}

pub fn write_session(path: &Path, data: &SessionData) -> Result<(), Error> {
    let file = std::fs::File::create(path).context(format!("Failed to create {}", path.display()))?;
    serde_json::to_writer(std::io::BufWriter::new(file), data).context(format!("Failed to write {}", path.display()))?;
    let size = std::fs::metadata(path).map_or(0, |m| m.len());
    info!(
        "Session written to {} ({})",
        path.display(),
        humansize::format_size(size, humansize::BINARY)
    );
    Ok(())
}

/// A name is trusted only if it has no path components at all and looks exactly
/// like what [`SessionStore::save`] produces.
fn is_session_file_name(name: &str) -> bool {
    if Path::new(name).components().count() != 1 {
        return false;
    }
    let Some(stem) = name.strip_prefix(FILE_PREFIX).and_then(|rest| rest.strip_suffix(FILE_SUFFIX)) else {
        return false;
    };
    !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Reads just enough of the file to recover `meta`, falling back to a full parse
/// if the layout ever changes.
fn read_meta(path: &Path) -> Option<SessionMeta> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; PREFIX_READ_BYTES];
    // A single read can come up short, so fill the buffer before giving up.
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    buffer.truncate(filled);

    if let Some(meta) = parse_meta_prefix(&buffer) {
        return Some(meta);
    }

    #[derive(serde::Deserialize)]
    struct MetaOnly {
        meta: SessionMeta,
    }
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<MetaOnly>(&content).ok().map(|wrapper| wrapper.meta)
}

/// The viewer with the recording and Chart.js baked in: one file that opens from
/// a double click, with no server and no network.
pub fn build_standalone_html(session_json: &str) -> String {
    let mut html = include_str!("serving/session_viewer.html").to_string();
    html = html.replace(
        r#"<script src="/static/chart.min.js"></script>"#,
        &format!("<script>{}</script>", include_str!("serving/chart.min.js")),
    );
    html.replace("</head>", &format!("<script>window.SESSION_DATA={session_json};</script></head>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_its_own_file_names() {
        assert!(is_session_file_name("session_2026-09-10_08-14-02.json"));
        assert!(!is_session_file_name("session_.json"));
        assert!(!is_session_file_name("other.json"));
        assert!(!is_session_file_name("session_x.txt"));
    }

    #[test]
    fn rejects_paths_that_escape_the_directory() {
        for name in [
            "../session_x.json",
            "sub/session_x.json",
            "/etc/session_x.json",
            "session_../../etc/passwd.json",
            "..",
        ] {
            assert!(!is_session_file_name(name), "{name} must be rejected");
        }

        let store = SessionStore::new("/tmp/sessions");
        store.path_for("../../etc/passwd").expect_err("traversal must be refused");
        let resolved = store.path_for("session_2026-01-01_00-00-00.json").expect("plain name is accepted");
        assert_eq!(resolved, Path::new("/tmp/sessions/session_2026-01-01_00-00-00.json"));
    }
}
