use regex_lite::Regex;
use system_info_report::benchmark::{Metric, RunOutput};

/// How the matches of a metric's pattern in a run's log become one number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Extract {
    /// Capture group 1 of the last match.
    Last,
    First,
    Min,
    Max,
    Mean,
    Sum,
    /// How many times the pattern matched.
    Count,
    /// Change of group 1 over change of group 2 between the first and the last match, e.g.
    /// frames over seconds from `frames=(\d+) t=([\d.]+)`.
    Rate,
}

pub struct LogMetric {
    name: String,
    pattern: Regex,
    extract: Extract,
    decimals: u8,
}

impl LogMetric {
    pub fn new(name: &str, pattern: &str, extract: Extract, decimals: u8) -> Result<Self, String> {
        let pattern = Regex::new(pattern).map_err(|e| format!("metric {name:?}: {e}"))?;
        let groups_needed = match extract {
            Extract::Count => 0,
            Extract::Rate => 2,
            _ => 1,
        };
        if pattern.captures_len() - 1 < groups_needed {
            return Err(format!(
                "metric {name:?}: {extract:?} needs {groups_needed} capture group(s) in the pattern"
            ));
        }
        Ok(Self {
            name: name.to_string(),
            pattern,
            extract,
            decimals,
        })
    }

    /// `NAME=PATTERN`, with `:extract` after the name to pick something other than the last match,
    /// e.g. `fps:mean=fps=([\d.]+)`.
    pub fn parse_argument(argument: &str) -> Result<Self, String> {
        let (head, pattern) = argument.split_once('=').ok_or_else(|| format!("{argument:?} is not NAME=PATTERN"))?;
        let (name, extract) = match head.split_once(':') {
            Some((name, extract)) => (name, <Extract as clap::ValueEnum>::from_str(extract, true)?),
            None => (head, Extract::Last),
        };
        Self::new(name, pattern, extract, 1)
    }

    pub fn value(&self, log: &str) -> Option<f64> {
        let group = |index: usize| -> Vec<f64> {
            self.pattern
                .captures_iter(log)
                .filter_map(|c| c.get(index)?.as_str().parse::<f64>().ok())
                .collect()
        };
        match self.extract {
            Extract::Count => Some(self.pattern.find_iter(log).count() as f64),
            Extract::Last => group(1).last().copied(),
            Extract::First => group(1).first().copied(),
            Extract::Min => group(1).into_iter().reduce(f64::min),
            Extract::Max => group(1).into_iter().reduce(f64::max),
            Extract::Sum => {
                let values = group(1);
                (!values.is_empty()).then(|| values.iter().sum())
            }
            Extract::Mean => {
                let values = group(1);
                (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
            }
            Extract::Rate => {
                let pairs: Vec<(f64, f64)> = self
                    .pattern
                    .captures_iter(log)
                    .filter_map(|c| Some((c.get(1)?.as_str().parse().ok()?, c.get(2)?.as_str().parse().ok()?)))
                    .collect();
                let (first, last) = (pairs.first()?, pairs.last()?);
                let span = last.1 - first.1;
                (span > 0.0).then(|| (last.0 - first.0) / span)
            }
        }
    }
}

impl Metric for LogMetric {
    fn name(&self) -> &str {
        &self.name
    }

    fn decimals(&self) -> u8 {
        self.decimals
    }

    fn extract(&self, run: &RunOutput<'_>) -> Option<f64> {
        self.value(run.log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "start\npasses=10 seconds=1.0\nnoise\npasses=50 seconds=3.0\npasses=90 seconds=5.0\n";

    fn metric(extract: Extract) -> LogMetric {
        LogMetric::new("passes", r"passes=(\d+) seconds=([\d.]+)", extract, 1).expect("valid")
    }

    #[test]
    fn extracts_from_matches() {
        assert_eq!(metric(Extract::Last).value(LOG), Some(90.0));
        assert_eq!(metric(Extract::First).value(LOG), Some(10.0));
        assert_eq!(metric(Extract::Max).value(LOG), Some(90.0));
        assert_eq!(metric(Extract::Mean).value(LOG), Some(50.0));
        assert_eq!(metric(Extract::Count).value(LOG), Some(3.0));
        assert_eq!(metric(Extract::Rate).value(LOG), Some(20.0));
        assert_eq!(metric(Extract::Last).value("nothing here"), None);
        assert_eq!(metric(Extract::Count).value("nothing here"), Some(0.0));
    }

    #[test]
    fn parses_command_line_form() {
        let m = LogMetric::parse_argument(r"fps:mean=fps=([\d.]+)").expect("valid");
        assert_eq!(m.name, "fps");
        assert_eq!(m.value("fps=10\nfps=20"), Some(15.0));
        assert!(LogMetric::parse_argument("no-equals").is_err_and(|e| e.contains("NAME=PATTERN")));
        assert!(LogMetric::parse_argument("x:rate=only (\\d+)").is_err_and(|e| e.contains("2 capture group")));
    }
}
