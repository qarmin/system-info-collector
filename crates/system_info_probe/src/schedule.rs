use std::time::{Duration, Instant};

/// Ticks stay on a fixed grid from the start; ones missed because a sample ran long are skipped
/// rather than fired back to back.
pub(crate) fn next_deadline(previous: Instant, interval: Duration, now: Instant) -> Instant {
    let mut next = previous + interval;
    while next <= now {
        next += interval;
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_missed_ticks_but_keeps_the_grid() {
        let start = Instant::now();
        let interval = Duration::from_millis(100);
        assert_eq!(next_deadline(start, interval, start), start + interval);
        assert_eq!(
            next_deadline(start, interval, start + Duration::from_millis(250)),
            start + Duration::from_millis(300)
        );
    }
}
