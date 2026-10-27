//! The benchmark file: variants, a matrix of dimensions they are expanded over, metrics read
//! from their output and services kept running around every run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use system_info_probe::measure::{MeasureOptions, MeasureScope, RunLength};
use system_info_report::benchmark::Variant;

use crate::duration::DurationText;
use crate::metric::{Extract, LogMetric};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Process,
    #[default]
    Tree,
}

impl From<Scope> for MeasureScope {
    fn from(scope: Scope) -> Self {
        match scope {
            Scope::Process => Self::Process,
            Scope::Tree => Self::Tree,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    #[serde(default = "one")]
    pub repeats: usize,
    pub warmup: Option<DurationText>,
    /// Measure this long, then kill; exclusive with `until_exit`.
    pub duration: Option<DurationText>,
    /// Measure until the program exits, killing it after `timeout` if given.
    #[serde(default)]
    pub until_exit: bool,
    pub timeout: Option<DurationText>,
    pub interval: Option<DurationText>,
    #[serde(default)]
    pub scope: Scope,
    /// Keep every run's output here instead of in a temporary file.
    pub logs: Option<PathBuf>,
    #[serde(default)]
    pub matrix: toml::Table,
    #[serde(rename = "variant")]
    pub variants: Vec<VariantSpec>,
    #[serde(default, rename = "metric")]
    pub metrics: Vec<MetricSpec>,
    #[serde(default, rename = "service")]
    pub services: Vec<ServiceSpec>,
}

const fn one() -> usize {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantSpec {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    /// Matrix dimensions this variant is expanded over; all of them when absent.
    pub matrix: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSpec {
    pub name: String,
    pub pattern: String,
    #[serde(default = "last")]
    pub extract: Extract,
    #[serde(default = "one_decimal")]
    pub decimals: u8,
}

const fn last() -> Extract {
    Extract::Last
}

const fn one_decimal() -> u8 {
    1
}

/// A command started before every run and killed after it, e.g. a virtual display.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    /// Run through `sh -c`; may use the run's `{placeholders}`.
    pub command: String,
    /// Wait this long after starting it, for it to become ready.
    pub settle: Option<DurationText>,
}

pub fn load(path: &Path) -> Result<File, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

impl File {
    pub fn measure_options(&self) -> Result<MeasureOptions, String> {
        let run = match (self.duration, self.until_exit) {
            (Some(duration), false) => RunLength::Fixed(duration.0),
            (None, true) => RunLength::UntilExit {
                timeout: self.timeout.map(|t| t.0),
            },
            (Some(_), true) => return Err("`duration` and `until_exit` exclude each other".to_string()),
            (None, false) => return Err("set either `duration` or `until_exit = true`".to_string()),
        };
        Ok(MeasureOptions {
            warmup: self.warmup.map(|w| w.0).unwrap_or_default(),
            run,
            interval: self.interval.map_or(MeasureOptions::default().interval, |i| i.0),
            scope: self.scope.into(),
            ..MeasureOptions::default()
        })
    }

    pub fn metrics(&self) -> Result<Vec<LogMetric>, String> {
        self.metrics
            .iter()
            .map(|m| LogMetric::new(&m.name, &m.pattern, m.extract, m.decimals))
            .collect()
    }

    /// Every variant crossed with every combination of its matrix dimensions, in file order.
    pub fn expand(&self) -> Result<Vec<Variant>, String> {
        let mut dimensions: Vec<(String, Vec<String>)> = Vec::new();
        for (key, values) in &self.matrix {
            let values = values
                .as_array()
                .ok_or_else(|| format!("matrix.{key} must be a list"))?
                .iter()
                .map(|v| match v {
                    toml::Value::String(s) => Ok(s.clone()),
                    toml::Value::Integer(i) => Ok(i.to_string()),
                    toml::Value::Float(f) => Ok(f.to_string()),
                    toml::Value::Boolean(b) => Ok(b.to_string()),
                    _ => Err(format!("matrix.{key} may hold only strings, numbers and booleans")),
                })
                .collect::<Result<_, _>>()?;
            dimensions.push((key.clone(), values));
        }
        let mut out = Vec::new();
        for spec in &self.variants {
            let used: Vec<&(String, Vec<String>)> = match &spec.matrix {
                Some(keys) => keys
                    .iter()
                    .map(|k| {
                        dimensions
                            .iter()
                            .find(|(d, _)| d == k)
                            .ok_or_else(|| format!("variant {:?}: no matrix dimension {k:?}", spec.name))
                    })
                    .collect::<Result<_, _>>()?,
                None => dimensions.iter().collect(),
            };
            for combination in combinations(&used) {
                let mut values: Vec<(String, String)> = vec![("name".to_string(), spec.name.clone())];
                values.extend(combination.iter().cloned());
                let fill = |text: &str| substitute(text, &values).map_err(|e| format!("variant {:?}: {e}", spec.name));
                out.push(Variant {
                    name: spec.name.clone(),
                    dimensions: combination,
                    program: fill(&spec.program)?,
                    args: spec.args.iter().map(|a| fill(a)).collect::<Result<_, _>>()?,
                    env: spec.env.iter().map(|(k, v)| Ok((k.clone(), fill(v)?))).collect::<Result<_, String>>()?,
                    cwd: spec.cwd.clone(),
                });
            }
        }
        Ok(out)
    }
}

fn combinations(dimensions: &[&(String, Vec<String>)]) -> Vec<Vec<(String, String)>> {
    let mut out: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for (key, values) in dimensions {
        out = out
            .into_iter()
            .flat_map(|prefix| {
                values.iter().map(move |value| {
                    let mut next = prefix.clone();
                    next.push((key.clone(), value.clone()));
                    next
                })
            })
            .collect();
    }
    out
}

/// Replaces `{key}` with its value; `{{` and `}}` stand for literal braces.  An unknown key is an
/// error, since it is almost always a typo.
pub fn substitute(text: &str, values: &[(String, String)]) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut key = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => key.push(c),
                        None => return Err(format!("unclosed {{ in {text:?}")),
                    }
                }
                let value = values
                    .iter()
                    .find(|(k, _)| *k == key)
                    .ok_or_else(|| format!("unknown placeholder {{{key}}} in {text:?}"))?;
                out.push_str(&value.1);
            }
            '}' => return Err(format!("unmatched }} in {text:?}")),
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
repeats = 2
warmup = "1s"
duration = "5s"
interval = "100ms"

[matrix]
size = ["1024x768", "3840x2160"]
mode = ["idle", "full"]
count = [100, 2000]

[[variant]]
name = "a"
program = "./bin/{name}"
args = ["{size}", "{mode}", "{{literal}}"]
env = { LEVEL = "{mode}" }
matrix = ["size", "mode"]

[[variant]]
name = "b"
program = "b"
matrix = ["count"]

[[metric]]
name = "fps"
pattern = 'frames=(\d+) t=([\d.]+)'
extract = "rate"
"#;

    #[test]
    fn expands_variants_over_their_dimensions_in_order() {
        let file: File = toml::from_str(FILE).expect("valid");
        let variants = file.expand().expect("expands");
        assert_eq!(variants.len(), 4 + 2);
        assert_eq!(variants[0].program, "./bin/a");
        assert_eq!(variants[0].args, vec!["1024x768", "idle", "{literal}"]);
        assert_eq!(variants[1].args[1], "full");
        assert_eq!(variants[2].args[0], "3840x2160");
        assert_eq!(variants[0].env, vec![("LEVEL".to_string(), "idle".to_string())]);
        assert_eq!(variants[4].dimensions, vec![("count".to_string(), "100".to_string())]);
        assert_eq!(file.metrics().expect("valid").len(), 1);
        let options = file.measure_options().expect("valid");
        assert_eq!(options.run, RunLength::Fixed(std::time::Duration::from_secs(5)));
    }

    #[test]
    fn rejects_typos_and_contradictions() {
        substitute("{sise}", &[("size".to_string(), "1".to_string())]).expect_err("typo");
        substitute("{open", &[]).expect_err("unclosed");
        assert_eq!(substitute("a{{b}}{x}", &[("x".to_string(), "1".to_string())]), Ok("a{b}1".to_string()));
        let both: File = toml::from_str("duration = \"1s\"\nuntil_exit = true\n[[variant]]\nname = \"x\"\nprogram = \"x\"\n").expect("parses");
        both.measure_options().expect_err("contradiction");
        toml::from_str::<File>("bogus = 1\n[[variant]]\nname = \"x\"\nprogram = \"x\"\n").expect_err("unknown key");
    }
}
