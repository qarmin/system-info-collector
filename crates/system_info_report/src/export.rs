//! Recordings as CSV and JSON.  Missing values are empty fields - never a sentinel such as `-1`
//! that would be read as a real number.

use std::io::Write;

use system_info_probe::recording::Recording;

use crate::series::{Series, SeriesOptions, series, times};
use crate::table::compact_number;

/// One row per tick, one column per series: `t` (seconds from the start), then e.g.
/// `cpu.pct,mem_used.mib,app.pct,...`.  The smallest layout, and what spreadsheets expect.
pub fn write_wide_csv(recording: &Recording, options: SeriesOptions, out: &mut impl Write) -> std::io::Result<()> {
    let all = series(recording, options);
    let mut line = String::from("t");
    for s in &all {
        line.push(',');
        line.push_str(&s.name);
    }
    writeln!(out, "{line}")?;
    for (tick, t) in times(recording).into_iter().enumerate() {
        line.clear();
        line.push_str(&compact_number(t, 2));
        for s in &all {
            line.push(',');
            if let Some(value) = s.values[tick] {
                line.push_str(&compact_number(value, s.unit.decimals()));
            }
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// One row per measured value: `t,series,value`.  Larger, but each row stands on its own, which
/// is what pandas, R and SQL want; ticks without a value simply have no row.
pub fn write_long_csv(recording: &Recording, options: SeriesOptions, out: &mut impl Write) -> std::io::Result<()> {
    let all: Vec<Series> = series(recording, options);
    writeln!(out, "t,series,value")?;
    for (tick, t) in times(recording).into_iter().enumerate() {
        let t = compact_number(t, 2);
        for s in &all {
            if let Some(value) = s.values[tick] {
                writeln!(out, "{t},{},{}", s.name, compact_number(value, s.unit.decimals()))?;
            }
        }
    }
    Ok(())
}

/// The whole recording, every field, unindented.
#[cfg(feature = "json")]
pub fn write_json(recording: &Recording, out: &mut impl Write) -> serde_json::Result<()> {
    serde_json::to_writer(out, recording)
}

#[cfg(feature = "json")]
pub fn read_json(input: impl std::io::Read) -> serde_json::Result<Recording> {
    serde_json::from_reader(input)
}
