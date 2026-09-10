//! Guards against a file being embedded with `include_str!` but never committed.
//!
//! That combination compiles perfectly on the machine that wrote the file and
//! fails on every other one, which is how `session_viewer.html` shipped broken:
//! a blanket `*.html` in `.gitignore` kept it out of the repository and nothing
//! noticed, because the build only needs the file to exist locally.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every `include_str!`/`include_bytes!` target in a crate's sources, resolved to
/// a repository-relative path.
fn embedded_paths(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut found = Vec::new();
    let mut directories = vec![root.join("crates")];

    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                directories.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let Ok(source) = std::fs::read_to_string(&path) else { continue };
                for target in embedded_in(&source) {
                    // `include_str!` resolves relative to the file doing the including.
                    let resolved = path.parent().map(|dir| dir.join(&target)).unwrap_or(target);
                    found.push((path.clone(), resolved));
                }
            }
        }
    }
    found
}

/// Literal paths passed to `include_str!` or `include_bytes!`.
///
/// Deliberately naive - it only understands a plain string literal, which is all
/// this repository uses. A macro with a computed path would be missed, so if one
/// ever appears this needs revisiting rather than silently covering less.
fn embedded_in(source: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for macro_name in ["include_str!(\"", "include_bytes!(\""] {
        let mut rest = source;
        while let Some(start) = rest.find(macro_name) {
            rest = &rest[start + macro_name.len()..];
            let Some(end) = rest.find('"') else { break };
            out.push(PathBuf::from(&rest[..end]));
            rest = &rest[end..];
        }
    }
    out
}

fn is_tracked(root: &Path, path: &Path) -> bool {
    // A staged-but-uncommitted file counts: it will reach the next clone.
    Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path)
        .current_dir(root)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[test]
fn every_embedded_file_is_in_the_repository() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root is two levels above the crate")
        .to_path_buf();

    // Building from a published tarball or an exported archive has no git to ask;
    // there is nothing to verify there, and failing would be a false alarm.
    let inside_git = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&root)
        .output()
        .is_ok_and(|out| out.status.success());
    if !inside_git {
        eprintln!("not a git work tree, skipping");
        return;
    }

    let embedded = embedded_paths(&root);
    assert!(
        !embedded.is_empty(),
        "found no include_str! targets at all - the scan is broken, not the repository"
    );

    let missing: Vec<String> = embedded
        .iter()
        .filter(|(_, target)| !is_tracked(&root, target))
        .map(|(source, target)| {
            let ignored = Command::new("git")
                .args(["check-ignore", "-v", "--"])
                .arg(target)
                .current_dir(&root)
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_else(|| "not ignored, just never added".to_string());
            format!(
                "{} embeds {} which git does not track\n      reason: {ignored}",
                source.display(),
                target.display()
            )
        })
        .collect();

    assert!(
        missing.is_empty(),
        "{} embedded file(s) would be absent from a fresh clone, so the build only works here:\n  - {}",
        missing.len(),
        missing.join("\n  - ")
    );
}
