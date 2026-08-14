use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use log::warn;
use serde::Serialize;
use system_info_collector_core::workers::file_writer::is_rotated_filename;

use crate::converting::csv_file_loader::scan_timestamps;

pub const CURRENT: &str = "current";
pub const PREVIOUS_RUN: &str = "previous run";
pub const ROTATED: &str = "rotated";

/// A CSV file that can be exported: the one the running collector writes, plus
/// the files left behind by earlier runs (`system_data__1.csv`) and by
/// size-based rotation (`system_data_2026-08-12_09-00-00.csv`).
#[derive(Clone, Serialize)]
pub struct DataSource {
    pub name: String,
    pub kind: &'static str,
    pub current: bool,
    pub points: usize,
    pub first_timestamp: Option<f64>,
    pub last_timestamp: Option<f64>,
    pub days: Vec<String>,
    pub weeks: Vec<String>,
    pub size_bytes: u64,
}

#[derive(Clone)]
struct Summary {
    points: usize,
    first_timestamp: Option<f64>,
    last_timestamp: Option<f64>,
    days: Vec<String>,
    weeks: Vec<String>,
}

struct CacheEntry {
    size_bytes: u64,
    modified: Option<SystemTime>,
    summary: Summary,
}

/// Summarizing a file means reading its whole timestamp column, so the result is
/// kept until the file changes on disk.  Only the file of the running collector
/// grows, so every older file is scanned once no matter how often the export
/// dialog is opened.
#[derive(Default)]
pub struct SourceCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl SourceCache {
    fn summary(&self, path: &str, size_bytes: u64, modified: Option<SystemTime>) -> Option<Summary> {
        let mut entries = self.entries.lock().expect("source cache lock poisoned");

        if let Some(entry) = entries.get(path)
            && entry.size_bytes == size_bytes
            && entry.modified == modified
        {
            return Some(entry.summary.clone());
        }

        let summary = match scan_timestamps(path) {
            Ok((start_time, timestamps)) => summarize(start_time, &timestamps),
            Err(e) => {
                warn!("Not offering {path} for export: {e}");
                return None;
            }
        };
        entries.insert(
            path.to_string(),
            CacheEntry {
                size_bytes,
                modified,
                summary: summary.clone(),
            },
        );
        Some(summary)
    }
}

/// Every data file belonging to `data_path`, with the period each one covers.
///
/// Sorted with the file of the running collector first, then the rest
/// newest-first.  Files without a single data row are left out.
pub fn list_sources(data_path: &str, cache: &SourceCache) -> Vec<DataSource> {
    let mut sources: Vec<DataSource> = discover(data_path)
        .into_iter()
        .filter_map(|(path, kind)| {
            let metadata = fs::metadata(&path).ok()?;
            let summary = cache.summary(&path, metadata.len(), metadata.modified().ok())?;
            Some(DataSource {
                name: file_name_of(&path)?,
                kind,
                current: kind == CURRENT,
                points: summary.points,
                first_timestamp: summary.first_timestamp,
                last_timestamp: summary.last_timestamp,
                days: summary.days,
                weeks: summary.weeks,
                size_bytes: metadata.len(),
            })
        })
        .filter(|source| source.points > 0)
        .collect();

    sources.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then(b.last_timestamp.partial_cmp(&a.last_timestamp).unwrap_or(std::cmp::Ordering::Equal))
    });
    sources
}

/// Path and kind of the data file called `name`, or `None` when no such file
/// belongs to `data_path`.  Names arrive from the browser, so they are matched
/// against the discovered files instead of being turned into a path directly.
pub fn resolve(data_path: &str, name: &str) -> Option<(String, &'static str)> {
    discover(data_path)
        .into_iter()
        .find(|(path, _)| file_name_of(path).as_deref() == Some(name))
}

fn discover(data_path: &str) -> Vec<(String, &'static str)> {
    let path = Path::new(data_path);
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let Some(current_name) = file_name_of(data_path) else {
        return vec![];
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();

    let mut found = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = if name == current_name {
            CURRENT
        } else if is_backup_filename(&name, &stem, &extension) {
            PREVIOUS_RUN
        } else if is_rotated_filename(&name, &stem, &extension) {
            ROTATED
        } else {
            continue;
        };
        found.push((entry.path().to_string_lossy().into_owned(), kind));
    }
    found
}

/// Matches the `{base}__{N}{ext}` files written by the start-up backup rotation.
fn is_backup_filename(file_name: &str, base: &str, extension: &str) -> bool {
    let Some(rest) = file_name.strip_prefix(&format!("{base}__")) else {
        return false;
    };
    let Some(number) = rest.strip_suffix(extension) else {
        return false;
    };
    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
}

fn file_name_of(path: &str) -> Option<String> {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Timestamps inside one file are monotonic, so a date is only formatted when it
/// differs from the previous row - otherwise a million-row file would build a
/// million throwaway strings.
fn summarize(start_time: f64, timestamps: &[f64]) -> Summary {
    let mut days: BTreeSet<String> = BTreeSet::new();
    let mut weeks: BTreeSet<String> = BTreeSet::new();
    let mut previous: Option<NaiveDate> = None;

    for timestamp in timestamps {
        let Some(date) = abs_ts_to_naive_date(start_time + timestamp) else {
            continue;
        };
        if previous == Some(date) {
            continue;
        }
        previous = Some(date);
        days.insert(date.format("%Y-%m-%d").to_string());
        weeks.insert(week_key(date));
    }

    Summary {
        points: timestamps.len(),
        first_timestamp: timestamps.first().map(|t| start_time + t),
        last_timestamp: timestamps.last().map(|t| start_time + t),
        days: days.into_iter().collect(),
        weeks: weeks.into_iter().collect(),
    }
}

pub fn abs_ts_to_naive_date(abs_ts: f64) -> Option<NaiveDate> {
    DateTime::from_timestamp(abs_ts as i64, 0).map(|dt: DateTime<Utc>| dt.date_naive())
}

pub fn week_key(date: NaiveDate) -> String {
    format!("{:04}-W{:02}", date.iso_week().year(), date.iso_week().week())
}

/// Calendar key an absolute timestamp belongs to, for split exports.
pub fn period_key(abs_ts: f64, split: &str) -> Option<String> {
    let date = abs_ts_to_naive_date(abs_ts)?;
    Some(match split {
        "week" => week_key(date),
        _ => date.format("%Y-%m-%d").to_string(),
    })
}

pub fn parse_week_str(s: &str) -> Option<(i32, u32)> {
    let (year_part, week_part) = s.split_once('-')?;
    let year: i32 = year_part.parse().ok()?;
    let week: u32 = week_part.strip_prefix('W')?.parse().ok()?;
    Some((year, week))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_backup_files() {
        assert!(is_backup_filename("system_data__1.csv", "system_data", ".csv"));
        assert!(is_backup_filename("system_data__12.csv", "system_data", ".csv"));
        assert!(!is_backup_filename("system_data.csv", "system_data", ".csv"));
        assert!(!is_backup_filename("system_data_top_cpu.csv", "system_data", ".csv"));
        assert!(!is_backup_filename("system_data__x.csv", "system_data", ".csv"));
        assert!(!is_backup_filename("other__1.csv", "system_data", ".csv"));
    }

    #[test]
    fn summarizes_days_and_weeks() {
        // 2026-08-12 00:00:00 UTC
        let start_time = 1_786_492_800.0;
        let summary = summarize(start_time, &[0.0, 3600.0, 90_000.0]);
        assert_eq!(summary.points, 3);
        assert_eq!(summary.days, vec!["2026-08-12".to_string(), "2026-08-13".to_string()]);
        assert_eq!(summary.weeks, vec!["2026-W33".to_string()]);
        assert_eq!(summary.first_timestamp, Some(start_time));
        assert_eq!(summary.last_timestamp, Some(start_time + 90_000.0));
    }
}
