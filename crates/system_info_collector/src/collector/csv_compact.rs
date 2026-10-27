//! Compact CSV cells: a value equal to the one in the row above is left empty, and inside a
//! `;`-separated cell (CPU per core) so is every element equal to the one above it.

const LIST_SEPARATOR: char = ';';

/// What to write for `value` given the full value of the row above.
pub fn compact(value: &str, previous: &str) -> String {
    if value == previous {
        return String::new();
    }
    if value.contains(LIST_SEPARATOR) && previous.contains(LIST_SEPARATOR) {
        let mut before = previous.split(LIST_SEPARATOR);
        let elements: Vec<&str> = value
            .split(LIST_SEPARATOR)
            .map(|v| if before.next() == Some(v) { "" } else { v })
            .collect();
        if previous.split(LIST_SEPARATOR).count() == elements.len() {
            return elements.join(";");
        }
    }
    value.to_string()
}

/// The full value of a cell read as `raw`, given the full value of the row above.
pub fn expand(raw: &str, previous: &str) -> String {
    if raw.is_empty() {
        return previous.to_string();
    }
    if raw.contains(LIST_SEPARATOR) && previous.contains(LIST_SEPARATOR) {
        let mut before = previous.split(LIST_SEPARATOR);
        return raw
            .split(LIST_SEPARATOR)
            .map(|v| {
                let above = before.next().unwrap_or_default();
                if v.is_empty() { above } else { v }
            })
            .collect::<Vec<_>>()
            .join(";");
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_repeated_values_and_list_elements_empty() {
        assert_eq!(compact("12.5", "12.5"), "");
        assert_eq!(compact("13", "12.5"), "13");
        assert_eq!(compact("4;0;0;7", "4;1;0;2"), ";0;;7");
        assert_eq!(compact("4;0", "-1"), "4;0", "nothing to compare a list against");
        assert_eq!(compact("4;0;1", "4;0"), "4;0;1", "core count changed");
    }

    #[test]
    fn round_trips() {
        let rows = ["-1", "4;1;0;2", "4;0;0;7", "4;0;0;7", "5;5;5;5", "-1", "1;2;3;4"];
        let mut previous_full = String::new();
        let mut previous_read = String::new();
        for row in rows {
            let written = compact(row, &previous_full);
            let read = expand(&written, &previous_read);
            assert_eq!(read, row, "written as {written:?}");
            previous_full = row.to_string();
            previous_read = read;
        }
    }
}
