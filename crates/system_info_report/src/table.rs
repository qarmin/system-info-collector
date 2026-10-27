use std::fmt::Write as _;
use std::time::Duration;

/// A value in a table.  Terminal and Markdown show it for people (`12.3 MiB`), TSV and CSV
/// write the bare number with the unit moved into the column header.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Text(String),
    Integer(i64),
    Number {
        value: f64,
        decimals: u8,
    },
    Bytes(u64),
    Duration(Duration),
    /// 100 meaning one core, or the whole of something.
    Percent(f64),
}

impl Cell {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn number(value: f64, decimals: u8) -> Self {
        Self::Number { value, decimals }
    }

    fn human(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(text) => text.clone(),
            Self::Integer(value) => value.to_string(),
            Self::Number { value, decimals } => format!("{value:.*}", usize::from(*decimals)),
            Self::Bytes(bytes) => human_bytes(*bytes),
            Self::Duration(duration) => human_duration(*duration),
            Self::Percent(value) => format!("{value:.1} %"),
        }
    }

    fn raw(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(text) => text.clone(),
            Self::Integer(value) => value.to_string(),
            Self::Number { value, decimals } => compact_number(*value, *decimals),
            Self::Bytes(bytes) => bytes.to_string(),
            Self::Duration(duration) => compact_number(duration.as_secs_f64(), 3),
            Self::Percent(value) => compact_number(*value, 2),
        }
    }

    fn unit(&self) -> Option<&'static str> {
        match self {
            Self::Bytes(_) => Some("B"),
            Self::Duration(_) => Some("s"),
            Self::Percent(_) => Some("%"),
            _ => None,
        }
    }

    fn right_aligned(&self) -> bool {
        !matches!(self, Self::Text(_) | Self::Empty)
    }
}

/// `value` with at most `decimals` decimals and no trailing zeros - `12.50` becomes `12.5`, `3.000`
/// becomes `3` - which keeps exported files small.
pub fn compact_number(value: f64, decimals: u8) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let mut text = format!("{value:.*}", usize::from(decimals));
    if text.contains('.') {
        let trimmed = text.trim_end_matches('0').trim_end_matches('.').len();
        text.truncate(trimmed);
    }
    if text == "-0" { "0".to_string() } else { text }
}

/// At most `max` characters, ending in `...` when cut.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max.saturating_sub(3)).collect::<String>())
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn human_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else if secs < 120.0 {
        format!("{secs:.2} s")
    } else {
        let total = duration.as_secs();
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableFormat {
    /// Aligned columns for a terminal or a log.
    Terminal,
    Markdown,
    Tsv,
    Csv,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub title: Option<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            title: None,
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    pub fn render(&self, format: TableFormat) -> String {
        match format {
            TableFormat::Terminal => self.terminal(),
            TableFormat::Markdown => self.markdown(),
            TableFormat::Tsv => self.delimited('\t'),
            TableFormat::Csv => self.delimited(','),
        }
    }

    fn cell(&self, row: usize, column: usize) -> &Cell {
        self.rows[row].get(column).unwrap_or(&Cell::Empty)
    }

    fn right_aligned(&self, column: usize) -> bool {
        (0..self.rows.len())
            .map(|row| self.cell(row, column))
            .find(|c| **c != Cell::Empty)
            .is_some_and(Cell::right_aligned)
    }

    fn human_grid(&self) -> Vec<Vec<String>> {
        (0..self.rows.len())
            .map(|row| (0..self.headers.len()).map(|column| self.cell(row, column).human()).collect())
            .collect()
    }

    fn terminal(&self) -> String {
        let grid = self.human_grid();
        let widths: Vec<usize> = (0..self.headers.len())
            .map(|column| {
                grid.iter()
                    .map(|row| row[column].chars().count())
                    .chain([self.headers[column].chars().count()])
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let line = |cells: &[String], out: &mut String| {
            let parts: Vec<String> = cells
                .iter()
                .enumerate()
                .map(|(column, text)| {
                    let width = widths[column];
                    if self.right_aligned(column) {
                        format!("{text:>width$}")
                    } else {
                        format!("{text:<width$}")
                    }
                })
                .collect();
            out.push_str(parts.join("  ").trim_end());
            out.push('\n');
        };
        let mut out = String::new();
        if let Some(title) = &self.title {
            out.push_str(title);
            out.push('\n');
        }
        line(&self.headers, &mut out);
        let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        out.push_str(&rule.join("  "));
        out.push('\n');
        for row in &grid {
            line(row, &mut out);
        }
        out
    }

    fn markdown(&self) -> String {
        let escape = |text: &str| text.replace('|', "\\|");
        let mut out = String::new();
        if let Some(title) = &self.title {
            let _ = writeln!(out, "**{}**\n", escape(title));
        }
        let headers: Vec<String> = self.headers.iter().map(|h| escape(h)).collect();
        let _ = writeln!(out, "| {} |", headers.join(" | "));
        let rule: Vec<&str> = (0..self.headers.len())
            .map(|c| if self.right_aligned(c) { "---:" } else { "---" })
            .collect();
        let _ = writeln!(out, "| {} |", rule.join(" | "));
        for row in self.human_grid() {
            let cells: Vec<String> = row.iter().map(|c| escape(c)).collect();
            let _ = writeln!(out, "| {} |", cells.join(" | "));
        }
        out
    }

    fn delimited(&self, separator: char) -> String {
        let field = |text: &str| -> String {
            let needs_quotes = text.contains(separator) || text.contains('"') || text.contains('\n') || text.contains('\r');
            match (separator, needs_quotes) {
                (',', true) => format!("\"{}\"", text.replace('"', "\"\"")),
                (_, true) => text.replace(['\t', '\n', '\r'], " "),
                _ => text.to_string(),
            }
        };
        let headers: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                let unit = (0..self.rows.len()).find_map(|row| self.cell(row, column).unit());
                field(&unit.map_or_else(|| header.clone(), |unit| format!("{header} [{unit}]")))
            })
            .collect();
        let mut out = headers.join(&separator.to_string());
        out.push('\n');
        for row in 0..self.rows.len() {
            let cells: Vec<String> = (0..self.headers.len()).map(|column| field(&self.cell(row, column).raw())).collect();
            out.push_str(&cells.join(&separator.to_string()));
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Table {
        let mut table = Table::new(["name", "cpu", "rss", "time"]).with_title("Runs");
        table.push(vec![
            Cell::text("egui"),
            Cell::Percent(12.345),
            Cell::Bytes(5 << 20),
            Cell::Duration(Duration::from_millis(1500)),
        ]);
        table.push(vec![
            Cell::text("a, \"quoted\" | name"),
            Cell::Percent(3.0),
            Cell::Empty,
            Cell::Duration(Duration::from_millis(250)),
        ]);
        table
    }

    #[test]
    fn terminal_aligns_numbers_right_and_text_left() {
        let rendered = sample().render(TableFormat::Terminal);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[0], "Runs");
        assert!(lines[1].starts_with("name "));
        assert!(lines[3].contains("12.3 %") && lines[3].contains("5.0 MiB") && lines[3].ends_with("1.50 s"));
        assert!(lines[4].contains("  3.0 %"), "{rendered}");
        assert!(lines[4].ends_with("250 ms"));
    }

    #[test]
    fn markdown_escapes_pipes_and_marks_numeric_columns() {
        let rendered = sample().render(TableFormat::Markdown);
        assert!(rendered.contains("| --- | ---: | ---: | ---: |"));
        assert!(rendered.contains("a, \"quoted\" \\| name"));
    }

    #[test]
    fn csv_writes_bare_numbers_with_units_in_the_header() {
        let rendered = sample().render(TableFormat::Csv);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[0], "name,cpu [%],rss [B],time [s]");
        assert_eq!(lines[1], "egui,12.35,5242880,1.5");
        assert_eq!(lines[2], "\"a, \"\"quoted\"\" | name\",3,,0.25");
        assert_eq!(sample().render(TableFormat::Tsv).lines().nth(1), Some("egui\t12.35\t5242880\t1.5"));
    }

    #[test]
    fn compact_numbers_drop_trailing_zeros() {
        assert_eq!(compact_number(12.50, 2), "12.5");
        assert_eq!(compact_number(3.0, 3), "3");
        assert_eq!(compact_number(-0.0001, 2), "0");
        assert_eq!(compact_number(1234.0, 0), "1234");
        assert_eq!(compact_number(f64::NAN, 2), "");
    }

    #[test]
    fn human_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_duration(Duration::from_secs(3725)), "1:02:05");
    }
}
