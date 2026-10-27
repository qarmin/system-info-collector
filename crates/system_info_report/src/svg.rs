//! Static SVG line charts: no script, small, and readable embedded in Markdown or an issue.
//!
//! Colors are the validated eight-slot categorical palette in fixed order, with its own dark
//! steps; text never takes a series color, identity comes from the legend swatch beside it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use system_info_probe::recording::Recording;

use crate::series::{Series, SeriesKind, SeriesOptions, Unit, series, times};
use crate::table::{compact_number, shorten};

const LIGHT: [&str; 8] = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"];
const DARK: [&str; 8] = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"];
/// surface, primary ink, secondary ink, muted, gridline, baseline
const LIGHT_CHROME: [&str; 6] = ["#fcfcfb", "#0b0b0b", "#52514e", "#898781", "#e1e0d9", "#c3c2b7"];
const DARK_CHROME: [&str; 6] = ["#1a1a19", "#f0efec", "#c3c2b7", "#898781", "#2c2c2a", "#383835"];
const MAX_LINES: usize = 8;
const DIRECT_LABEL_MAX_LINES: usize = 4;
const CHAR_WIDTH: f64 = 6.6;
const LEFT: f64 = 52.0;
const RIGHT_PAD: f64 = 12.0;
const LEGEND_ROW: f64 = 18.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    /// Follows the viewer's light or dark preference.
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvgOptions {
    pub width: u32,
    /// Height of the plot area alone; title, legend and axis labels come on top.
    pub plot_height: u32,
    pub theme: Theme,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            width: 760,
            plot_height: 180,
            theme: Theme::Auto,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub label: String,
    pub values: Vec<Option<f64>>,
}

/// Series sharing one unit on one axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub title: String,
    pub unit: Unit,
    /// Seconds from the start, one per value.
    pub times: Vec<f64>,
    pub lines: Vec<Line>,
}

/// One chart per kind of series, in a fixed order.  A kind with more than eight series keeps the
/// seven largest by total and sums the rest into `other`, as an extra hue would not be told apart.
pub fn charts(recording: &Recording, options: SeriesOptions) -> Vec<Chart> {
    let ticks = times(recording);
    let mut by_kind: BTreeMap<SeriesKind, Vec<Series>> = BTreeMap::new();
    for s in series(recording, options) {
        by_kind.entry(s.kind).or_default().push(s);
    }
    // Both process charts show the same processes in the same slots, picked by CPU, so a
    // process keeps its color from one chart to the next.
    let process_choice = by_kind.get(&SeriesKind::ProcessCpu).map(|cpu| chosen_labels(cpu));
    by_kind
        .into_iter()
        .map(|(kind, series)| {
            let choice = match kind {
                SeriesKind::ProcessCpu | SeriesKind::ProcessMemory => process_choice.clone(),
                _ => None,
            };
            let mut chart = Chart {
                title: kind.title().to_string(),
                unit: series[0].unit,
                times: ticks.clone(),
                lines: fold_to_other(series, choice.as_deref()),
            };
            chart.rescale();
            chart
        })
        .collect()
}

/// Labels kept when a kind has more series than hues: the largest by total.
fn chosen_labels(series: &[Series]) -> Vec<String> {
    let mut ranked: Vec<&Series> = series.iter().collect();
    if ranked.len() > MAX_LINES {
        ranked.sort_by(|a, b| total(b).total_cmp(&total(a)));
        ranked.truncate(MAX_LINES - 1);
    }
    ranked.into_iter().map(|s| s.label.clone()).collect()
}

/// Kept series stay in their original order - slots follow the entity, never its rank - and the
/// rest are summed into one `other` line.
fn fold_to_other(series: Vec<Series>, choice: Option<&[String]>) -> Vec<Line> {
    let chosen = choice.map_or_else(|| chosen_labels(&series), <[String]>::to_vec);
    let (kept, rest): (Vec<Series>, Vec<Series>) = series.into_iter().partition(|s| chosen.contains(&s.label));
    let mut lines: Vec<Line> = kept
        .into_iter()
        .take(MAX_LINES)
        .map(|s| Line {
            label: s.label,
            values: s.values,
        })
        .collect();
    if !rest.is_empty() {
        let length = rest[0].values.len();
        let values = (0..length)
            .map(|i| {
                let present: Vec<f64> = rest.iter().filter_map(|s| s.values[i]).collect();
                (!present.is_empty()).then(|| present.iter().sum())
            })
            .collect();
        lines.push(Line {
            label: format!("other ({})", rest.len()),
            values,
        });
    }
    lines
}

impl Chart {
    /// Gigabytes of memory read better as GiB than as thousands of MiB.
    fn rescale(&mut self) {
        let max = self.lines.iter().flat_map(|l| l.values.iter().flatten()).copied().fold(0.0f64, f64::max);
        if self.unit == Unit::Mib && max >= 4096.0 {
            self.unit = Unit::Gib;
            for value in self.lines.iter_mut().flat_map(|l| l.values.iter_mut().flatten()) {
                *value /= 1024.0;
            }
        }
    }
}

fn total(series: &Series) -> f64 {
    series.values.iter().flatten().sum()
}

/// A round step giving about `target` intervals over `span`.
fn nice_step(span: f64, target: f64) -> f64 {
    if span <= 0.0 {
        return 1.0;
    }
    let raw = span / target;
    let magnitude = 10f64.powf(raw.log10().floor());
    let fraction = raw / magnitude;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

fn time_step(span: f64) -> f64 {
    const STEPS: [f64; 14] = [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0];
    STEPS.iter().copied().find(|step| span / step <= 7.0).unwrap_or(14_400.0)
}

fn time_label(seconds: f64, span: f64) -> String {
    let total = seconds.round() as u64;
    if span < 60.0 {
        format!("{total}s")
    } else if span < 3600.0 {
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    }
}

/// `950`, `1.5K`, `12K`, `3.2M` - axis ticks stay short.
fn compact_tick(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude >= 1e6 {
        format!("{}M", compact_number(value / 1e6, 1))
    } else if magnitude >= 1e3 {
        format!("{}K", compact_number(value / 1e3, 1))
    } else {
        compact_number(value, 2)
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const FONT: &str = "system-ui,-apple-system,'Segoe UI',Roboto,'DejaVu Sans',Arial,sans-serif";

/// Concrete colors per class: CSS variables would be shorter, but most renderers outside browsers
/// (image viewers, converters, office suites) ignore them.  A renderer ignoring the media query
/// falls back to the light colors.
fn colors(palette: &[&str; 8], chrome: &[&str; 6]) -> String {
    let [surface, ink, ink2, muted, grid, base] = chrome;
    let mut css = format!(".bg{{fill:{surface}}}.t{{fill:{ink}}}.l{{fill:{ink2}}}.a{{fill:{muted}}}.g{{stroke:{grid}}}.b{{stroke:{base}}}");
    for (slot, color) in palette.iter().enumerate() {
        let _ = write!(css, ".c{}{{stroke:{color}}}", slot + 1);
    }
    css
}

fn style(theme: Theme) -> String {
    let mut css = format!(
        ".t{{font:600 13px {FONT}}}.l{{font:12px {FONT}}}.a{{font:11px {FONT}}}.g,.b{{stroke-width:1}}\
         path{{fill:none;stroke-width:2;stroke-linejoin:round;stroke-linecap:round}}"
    );
    match theme {
        Theme::Light => css.push_str(&colors(&LIGHT, &LIGHT_CHROME)),
        Theme::Dark => css.push_str(&colors(&DARK, &DARK_CHROME)),
        Theme::Auto => {
            css.push_str(&colors(&LIGHT, &LIGHT_CHROME));
            let _ = write!(css, "@media (prefers-color-scheme:dark){{{}}}", colors(&DARK, &DARK_CHROME));
        }
    }
    css
}

impl Chart {
    fn legend_rows(&self, width: f64) -> Vec<Vec<(usize, String, f64)>> {
        if self.lines.len() < 2 {
            return Vec::new();
        }
        let mut rows: Vec<Vec<(usize, String, f64)>> = vec![Vec::new()];
        let mut x = LEFT;
        for (index, line) in self.lines.iter().enumerate() {
            let label = shorten(&line.label, 40);
            let entry_width = 22.0 + label.chars().count() as f64 * CHAR_WIDTH + 14.0;
            if x + entry_width > width - RIGHT_PAD && x > LEFT {
                rows.push(Vec::new());
                x = LEFT;
            }
            rows.last_mut().expect("never empty").push((index, label, x));
            x += entry_width;
        }
        rows
    }

    /// Height the chart takes at `options`.
    pub fn height(&self, options: &SvgOptions) -> f64 {
        28.0 + self.legend_rows(f64::from(options.width)).len() as f64 * LEGEND_ROW + 8.0 + f64::from(options.plot_height) + 26.0
    }

    /// The chart as a group placed at `top`, for stacking several in one document.
    fn render_into(&self, out: &mut String, top: f64, options: &SvgOptions) {
        let width = f64::from(options.width);
        let legend = self.legend_rows(width);
        let plot_top = top + 28.0 + legend.len() as f64 * LEGEND_ROW + 8.0;
        let plot_height = f64::from(options.plot_height);
        let plot_bottom = plot_top + plot_height;
        let direct_labels = (2..=DIRECT_LABEL_MAX_LINES).contains(&self.lines.len());
        let label_room = if direct_labels {
            self.lines.iter().map(|l| shorten(&l.label, 14).chars().count()).max().unwrap_or(0) as f64 * CHAR_WIDTH + 12.0
        } else {
            0.0
        };
        let plot_right = width - RIGHT_PAD - label_room;
        let plot_width = plot_right - LEFT;

        let max_value = self.lines.iter().flat_map(|l| l.values.iter().flatten()).copied().fold(0.0f64, f64::max);
        let y_step = nice_step(if max_value > 0.0 { max_value } else { 1.0 }, 4.0);
        let y_max = (max_value / y_step).ceil().max(1.0) * y_step;
        let span = self.times.last().copied().unwrap_or(0.0).max(1e-9);
        let x_of = |t: f64| LEFT + t / span * plot_width;
        let y_of = |v: f64| plot_bottom - v / y_max * plot_height;

        let _ = write!(
            out,
            r#"<text class="t" x="{LEFT}" y="{:.1}">{} ({})</text>"#,
            top + 18.0,
            escape(&self.title),
            escape(self.unit.label())
        );
        for (row, entries) in legend.iter().enumerate() {
            let y = top + 28.0 + row as f64 * LEGEND_ROW + 10.0;
            for (index, label, x) in entries {
                let _ = write!(
                    out,
                    r#"<path class="c{}" d="M{x:.1} {:.1}h14"/><text class="l" x="{:.1}" y="{:.1}">{}</text>"#,
                    index + 1,
                    y - 4.0,
                    x + 20.0,
                    y,
                    escape(label)
                );
            }
        }

        let mut tick = 0.0;
        while tick <= y_max + y_step / 2.0 {
            let y = y_of(tick);
            let class = if tick == 0.0 { "b" } else { "g" };
            let _ = write!(
                out,
                r#"<line class="{class}" x1="{LEFT}" x2="{plot_right:.1}" y1="{y:.1}" y2="{y:.1}"/><text class="a" x="{:.1}" y="{:.1}" text-anchor="end">{}</text>"#,
                LEFT - 6.0,
                y + 4.0,
                compact_tick(tick)
            );
            tick += y_step;
        }
        let x_step = time_step(span);
        let mut t = 0.0;
        while t <= span + 1e-9 {
            let _ = write!(
                out,
                r#"<text class="a" x="{:.1}" y="{:.1}" text-anchor="middle">{}</text>"#,
                x_of(t),
                plot_bottom + 16.0,
                time_label(t, span)
            );
            t += x_step;
        }

        let mut end_labels: Vec<(f64, usize)> = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let d = path(&self.times, &line.values, plot_width, &x_of, &y_of);
            let last = line.values.iter().rev().flatten().next().copied();
            let _ = write!(
                out,
                r#"<path class="c{}" d="{d}"><title>{}{}</title></path>"#,
                index + 1,
                escape(&line.label),
                last.map_or_else(String::new, |v| format!(
                    ": {} {}",
                    compact_number(v, self.unit.decimals()),
                    self.unit.label()
                ))
            );
            if direct_labels && let Some(value) = line.values.last().copied().flatten() {
                end_labels.push((y_of(value), index));
            }
        }
        // Labels that would overlap are dropped rather than nudged off their lines; the legend
        // still names every series.
        end_labels.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut last_y = f64::NEG_INFINITY;
        for (y, index) in end_labels {
            if y - last_y < 13.0 {
                continue;
            }
            last_y = y;
            let _ = write!(
                out,
                r#"<text class="l" x="{:.1}" y="{:.1}">{}</text>"#,
                plot_right + 6.0,
                y + 4.0,
                escape(&shorten(&self.lines[index].label, 14))
            );
        }
    }

    pub fn to_svg(&self, options: &SvgOptions) -> String {
        document(std::slice::from_ref(self), options)
    }
}

/// Every chart stacked in one SVG document.
pub fn document(charts: &[Chart], options: &SvgOptions) -> String {
    let height: f64 = charts.iter().map(|c| c.height(options)).sum();
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{height:.0}" viewBox="0 0 {w} {height:.0}"><style>{}</style><rect class="bg" width="100%" height="100%"/>"#,
        style(options.theme),
        w = options.width,
    );
    let mut top = 0.0;
    for chart in charts {
        chart.render_into(&mut out, top, options);
        top += chart.height(options);
    }
    out.push_str("</svg>");
    out
}

/// Path data with a break wherever a value is missing.  Longer series than the plot has pixels
/// keep only the lowest and highest value per pixel column, so peaks survive and the file stays
/// proportional to the chart width rather than to the recording length.
fn path(times: &[f64], values: &[Option<f64>], plot_width: f64, x_of: &dyn Fn(f64) -> f64, y_of: &dyn Fn(f64) -> f64) -> String {
    let columns = plot_width.max(1.0) as usize;
    let mut points: Vec<Option<(f64, f64)>> = Vec::with_capacity(values.len().min(columns * 2));
    if values.len() > columns * 2 {
        let span = times.last().copied().unwrap_or(0.0).max(1e-9);
        let mut bucket: Option<usize> = None;
        let mut low: Option<(f64, f64)> = None;
        let mut high: Option<(f64, f64)> = None;
        fn flush(low: &mut Option<(f64, f64)>, high: &mut Option<(f64, f64)>, points: &mut Vec<Option<(f64, f64)>>) {
            match (low.take(), high.take()) {
                (Some(a), Some(b)) if a == b => points.push(Some(a)),
                (Some(a), Some(b)) if a.0 <= b.0 => points.extend([Some(a), Some(b)]),
                (Some(a), Some(b)) => points.extend([Some(b), Some(a)]),
                _ => {}
            }
        }
        for (t, value) in times.iter().zip(values) {
            let column = ((t / span) * (columns - 1) as f64) as usize;
            if bucket != Some(column) {
                flush(&mut low, &mut high, &mut points);
                bucket = Some(column);
            }
            match value {
                Some(v) => {
                    if low.is_none_or(|(_, l)| *v < l) {
                        low = Some((*t, *v));
                    }
                    if high.is_none_or(|(_, h)| *v > h) {
                        high = Some((*t, *v));
                    }
                }
                None => {
                    flush(&mut low, &mut high, &mut points);
                    points.push(None);
                }
            }
        }
        flush(&mut low, &mut high, &mut points);
    } else {
        points.extend(times.iter().zip(values).map(|(t, v)| v.map(|v| (*t, v))));
    }

    let mut d = String::new();
    let mut pen_down = false;
    for point in points {
        match point {
            Some((t, v)) => {
                let _ = write!(d, "{}{:.1} {:.1}", if pen_down { "L" } else { "M" }, x_of(t), y_of(v));
                pen_down = true;
            }
            None => pen_down = false,
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(lines: Vec<Line>) -> Chart {
        Chart {
            title: "CPU".to_string(),
            unit: Unit::Percent,
            times: (0..lines[0].values.len()).map(|i| i as f64).collect(),
            lines,
        }
    }

    #[test]
    fn missing_values_break_the_line() {
        let d = path(
            &[0.0, 1.0, 2.0, 3.0],
            &[Some(1.0), None, Some(2.0), Some(3.0)],
            100.0,
            &|t| t * 10.0,
            &|v| v,
        );
        assert_eq!(d, "M0.0 1.0M20.0 2.0L30.0 3.0");
    }

    #[test]
    fn long_series_keep_extremes_per_pixel() {
        let times: Vec<f64> = (0..10_000).map(f64::from).collect();
        let mut values: Vec<Option<f64>> = vec![Some(1.0); 10_000];
        values[5_000] = Some(99.0);
        let d = path(&times, &values, 100.0, &|t| t / 100.0, &|v| v);
        assert!(d.matches(['M', 'L']).count() <= 200, "{} points", d.matches(['M', 'L']).count());
        assert!(d.contains(" 99.0"), "the spike survives");
    }

    #[test]
    fn nice_axis_steps() {
        assert!((nice_step(87.0, 4.0) - 50.0).abs() < 1e-9);
        assert!((nice_step(7.3, 4.0) - 2.0).abs() < 1e-9);
        assert!((time_step(600.0) - 120.0).abs() < 1e-9);
        assert_eq!(time_label(125.0, 600.0), "2:05");
        assert_eq!(compact_tick(1500.0), "1.5K");
    }

    #[test]
    fn single_series_has_no_legend_and_many_fold_into_other() {
        let one = chart(vec![Line {
            label: "a".to_string(),
            values: vec![Some(1.0), Some(2.0)],
        }]);
        assert!(one.legend_rows(760.0).is_empty());

        let series: Vec<Series> = (0..12)
            .map(|i| Series {
                name: format!("s{i}"),
                label: format!("s{i}"),
                kind: SeriesKind::ProcessCpu,
                unit: Unit::Percent,
                values: vec![Some(f64::from(i)), None],
            })
            .collect();
        let lines = fold_to_other(series, None);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0].label, "s5", "kept series stay in entity order");
        assert_eq!(lines[6].label, "s11");
        assert_eq!(lines[7].label, "other (5)");
        assert_eq!(lines[7].values, vec![Some(10.0), None]);
    }

    #[test]
    fn renders_a_well_formed_document() {
        let svg = chart(vec![
            Line {
                label: "a <b>".to_string(),
                values: vec![Some(10.0), Some(40.0), None, Some(20.0)],
            },
            Line {
                label: "c".to_string(),
                values: vec![Some(5.0), Some(5.0), Some(6.0), Some(7.0)],
            },
        ])
        .to_svg(&SvgOptions::default());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("a &lt;b&gt;"));
        assert!(svg.contains("prefers-color-scheme:dark"));
        assert_eq!(svg.matches("<path class=\"c1\"").count(), 2, "legend swatch and line");
    }
}
