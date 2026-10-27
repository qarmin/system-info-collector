//! Interactive HTML reports: one plotly figure with a row per chart, sharing the time axis.

use plotly::common::{Line, Title};
use plotly::layout::themes::PLOTLY_DARK;
use plotly::layout::{Axis, AxisRange, GridPattern, Layout, LayoutGrid};
use plotly::{Plot, Scatter};
use system_info_probe::recording::Recording;

use crate::series::SeriesOptions;

/// The vendored plotly has axes up to 14; charts past that are left out.
pub const MAX_CHART_ROWS: usize = 14;
const ROW_HEIGHT: u32 = 380;

#[derive(Clone, Debug, PartialEq)]
pub struct HtmlTrace {
    pub name: String,
    /// Any CSS color; `None` leaves the choice to plotly's palette.
    pub color: Option<String>,
    /// Aligned with [`HtmlReport::times`]; `None` is drawn as a gap.
    pub values: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HtmlChart {
    pub y_title: String,
    pub y_range: Option<(i64, i64)>,
    pub traces: Vec<HtmlTrace>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtmlTheme {
    #[default]
    Dark,
    White,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HtmlReport {
    /// Timestamps of the samples as ISO 8601 text, e.g. `2026-10-27T16:18:30.070Z`.
    pub times: Vec<String>,
    pub charts: Vec<HtmlChart>,
    /// Lines shown under the charts, e.g. the CPU model.  HTML is not escaped.
    pub notes: Vec<String>,
    pub width: u32,
    /// The figure grows past this with the number of charts.
    pub min_height: u32,
    pub theme: HtmlTheme,
}

/// A self-contained page; plotly.js itself is loaded from its CDN.
pub fn render(report: &HtmlReport) -> String {
    let rows = report.charts.len().min(MAX_CHART_ROWS);
    let mut layout = Layout::new()
        .width(report.width as usize)
        .height(report.min_height.max(rows as u32 * ROW_HEIGHT) as usize)
        .grid(LayoutGrid::new().rows(rows).columns(1).pattern(GridPattern::Independent));
    if report.theme == HtmlTheme::Dark {
        layout = layout.template(&*PLOTLY_DARK);
    }
    let x_axis = Axis::new().title(Title::with_text("Time"));
    let mut plot = Plot::new();
    for (index, chart) in report.charts.iter().take(MAX_CHART_ROWS).enumerate() {
        let row = index as u32 + 1;
        let mut y_axis = Axis::new().title(Title::with_text(&chart.y_title));
        if let Some((low, high)) = chart.y_range {
            y_axis = match (usize::try_from(low), usize::try_from(high)) {
                (Ok(low), Ok(high)) => y_axis.range(AxisRange::new(low, high)),
                _ => y_axis.range(vec![low, high]),
            };
        }
        layout = set_axes(row, layout, x_axis.clone(), y_axis);
        for trace in &chart.traces {
            let mut scatter = Scatter::new(report.times.clone(), trace.values.clone())
                .name(&trace.name)
                .y_axis(format!("y{row}"))
                .x_axis(format!("x{row}"));
            if let Some(color) = &trace.color {
                scatter = scatter.line(Line::new().color(color.clone()));
            }
            plot.add_trace(scatter);
        }
    }
    plot.set_layout(layout);

    let mut html = plot.to_html();
    if report.theme == HtmlTheme::Dark {
        html = html.replace("<head>", "<head><style>body {background-color: #111111;color: white;}</style>");
    }
    let notes: String = report
        .notes
        .iter()
        .map(|note| format!("<div style=\"text-align: center;\">{note}</div>"))
        .collect();
    html = html.replace("</body>", &format!("{notes}\n{}\n</body>", per_chart_legends_script()));
    minify(&html)
}

/// Drops the indentation plotly-rs writes after every line break.
fn minify(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut lines = html.split('\n');
    if let Some(first) = lines.next() {
        out.push_str(first);
    }
    for line in lines {
        match line.strip_prefix(' ') {
            Some(indented) => out.push_str(indented.trim_start_matches(' ')),
            None => {
                out.push('\n');
                out.push_str(line);
            }
        }
    }
    out
}

/// One plotly legend per chart, placed beside it.  Plotly.js 2.16+ supports several legends,
/// plotly-rs 0.14 cannot express them, so the figure is restyled once it is drawn.
fn per_chart_legends_script() -> &'static str {
    r#"<script type="module">
    (async () => {
        const plotDiv = document.getElementById('plotly-html-element');
        while (!plotDiv._fullLayout) await new Promise(r => setTimeout(r, 50));
        const fl = plotDiv._fullLayout;
        const data = plotDiv.data;
        const yaxisOrder = [];
        const yaxisSeen = new Set();
        data.forEach(t => {
            const y = t.yaxis || 'y';
            if (!yaxisSeen.has(y)) { yaxisSeen.add(y); yaxisOrder.push(y); }
        });
        if (yaxisOrder.length <= 1) return;
        const legendMap = {};
        yaxisOrder.forEach((y, i) => { legendMap[y] = i === 0 ? 'legend' : 'legend' + (i + 1); });
        const layoutUpdate = {};
        yaxisOrder.forEach(y => {
            const axisKey = y === 'y' ? 'yaxis' : 'yaxis' + y.slice(1);
            const domain = fl[axisKey]?.domain ?? [0, 1];
            const yMid = (domain[0] + domain[1]) / 2;
            layoutUpdate[legendMap[y]] = { y: yMid, yanchor: 'middle', x: 1.02, xanchor: 'left', tracegroupgap: 0 };
        });
        const legendRefs = data.map(t => legendMap[t.yaxis || 'y']);
        await Plotly.relayout(plotDiv, layoutUpdate);
        await Plotly.restyle(plotDiv, { legend: legendRefs });
    })();
</script>"#
}

fn set_axes(row: u32, layout: Layout, x: Axis, y: Axis) -> Layout {
    match row {
        1 => layout.x_axis(x).y_axis(y),
        2 => layout.x_axis2(x).y_axis2(y),
        3 => layout.x_axis3(x).y_axis3(y),
        4 => layout.x_axis4(x).y_axis4(y),
        5 => layout.x_axis5(x).y_axis5(y),
        6 => layout.x_axis6(x).y_axis6(y),
        7 => layout.x_axis7(x).y_axis7(y),
        8 => layout.x_axis8(x).y_axis8(y),
        9 => layout.x_axis9(x).y_axis9(y),
        10 => layout.x_axis10(x).y_axis10(y),
        11 => layout.x_axis11(x).y_axis11(y),
        12 => layout.x_axis12(x).y_axis12(y),
        13 => layout.x_axis13(x).y_axis13(y),
        14 => layout.x_axis14(x).y_axis14(y),
        _ => layout,
    }
}

/// `2026-10-27T16:18:30.070Z` for a unix timestamp, shifted by `offset_secs` so local time can
/// be shown without plotly knowing the zone.
pub fn iso_time(unix_secs: f64, offset_secs: i64) -> String {
    let millis_total = ((unix_secs + offset_secs as f64) * 1000.0).round() as i64;
    let (days, millis) = (millis_total.div_euclid(86_400_000), millis_total.rem_euclid(86_400_000));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (hours, minutes, seconds, ms) = (millis / 3_600_000, millis / 60_000 % 60, millis / 1000 % 60, millis % 1000);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{ms:03}Z")
}

/// An interactive report of any recording: the same charts as the SVG export.
pub fn from_recording(recording: &Recording, options: SeriesOptions, offset_secs: i64, theme: HtmlTheme) -> HtmlReport {
    let charts = crate::svg::charts(recording, options);
    HtmlReport {
        times: recording
            .ticks
            .iter()
            .map(|t| iso_time(recording.start_unix + t.offset.as_secs_f64(), offset_secs))
            .collect(),
        charts: charts
            .into_iter()
            .map(|chart| HtmlChart {
                y_title: format!("{} [{}]", chart.title, chart.unit.label()),
                y_range: None,
                traces: chart
                    .lines
                    .into_iter()
                    .map(|line| HtmlTrace {
                        name: line.label,
                        color: None,
                        values: line.values,
                    })
                    .collect(),
            })
            .collect(),
        notes: Vec::new(),
        width: 1700,
        min_height: 800,
        theme,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_time_as_iso() {
        assert_eq!(iso_time(0.0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_time(1_793_114_310.07, 0), "2026-10-27T15:18:30.070Z");
        assert_eq!(iso_time(951_782_400.0, 3600), "2000-02-29T01:00:00.000Z");
    }

    #[test]
    fn renders_gaps_ranges_and_notes() {
        let html = render(&HtmlReport {
            times: vec![iso_time(0.0, 0), iso_time(1.0, 0)],
            charts: vec![HtmlChart {
                y_title: "CPU [%]".to_string(),
                y_range: Some((-1, 100)),
                traces: vec![HtmlTrace {
                    name: "cpu".to_string(),
                    color: Some("hsl(1, 2%, 3%)".to_string()),
                    values: vec![Some(5.0), None],
                }],
            }],
            notes: vec!["CPU: test".to_string()],
            width: 800,
            min_height: 400,
            theme: HtmlTheme::White,
        });
        assert!(html.contains(r#""y":[5.0,null]"#), "{html}");
        assert!(html.contains(r#""range":[-1,100]"#));
        assert!(html.contains("hsl(1, 2%, 3%)"));
        assert!(html.contains("<div style=\"text-align: center;\">CPU: test</div>"));
        assert!(!html.contains("background-color: #111111"));
    }

    #[test]
    fn minify_joins_indented_lines() {
        assert_eq!(minify("a\n    b\nc\n  d"), "ab\ncd");
    }
}
