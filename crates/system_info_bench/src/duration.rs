use std::time::Duration;

/// `250ms`, `4s`, `1.5s`, `2m`, `1h`; a bare number is seconds.
pub fn parse(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number.parse().map_err(|_| format!("{text:?} is not a duration like 500ms, 4s or 2m"))?;
    let seconds = match unit.trim() {
        "ms" => value / 1000.0,
        "" | "s" => value,
        "m" | "min" => value * 60.0,
        "h" => value * 3600.0,
        other => return Err(format!("unknown unit {other:?} in {text:?}, use ms, s, m or h")),
    };
    Duration::try_from_secs_f64(seconds).map_err(|e| format!("{text:?}: {e}"))
}

/// A duration written as text in the configuration file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurationText(pub Duration);

impl<'de> serde::Deserialize<'de> for DurationText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text).map(Self).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse("250ms"), Ok(Duration::from_millis(250)));
        assert_eq!(parse("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse("4"), Ok(Duration::from_secs(4)));
        assert_eq!(parse("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse(" 1h "), Ok(Duration::from_secs(3600)));
        parse("5 days").expect_err("unknown unit");
        parse("ms").expect_err("no number");
        parse("-1s").expect_err("negative");
    }
}
