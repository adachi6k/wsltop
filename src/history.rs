//! Fixed-clock host history. Waiting slots hold the last observation; unavailable
//! readings remain explicit failures. Redraws do not move individual samples.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

// Display retention only; collection cadence and accounting are independent.
pub const HISTORY_LEN: usize = 23;

#[derive(Debug, Clone, PartialEq)]
pub struct HostHistory {
    pub cpu: MetricHistory,
    pub memory: MetricHistory,
}

impl Default for HostHistory {
    fn default() -> Self {
        Self::new(Instant::now(), Duration::from_secs(3))
    }
}

impl HostHistory {
    pub fn new(origin: Instant, interval: Duration) -> Self {
        Self {
            cpu: MetricHistory::new(origin, interval),
            memory: MetricHistory::new(origin, interval),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricHistory {
    origin: Instant,
    interval: Duration,
    points: VecDeque<(u128, Option<f64>)>,
}

impl MetricHistory {
    fn new(origin: Instant, interval: Duration) -> Self {
        Self {
            origin,
            interval,
            points: VecDeque::new(),
        }
    }

    fn slot(&self, at: Instant) -> Option<u128> {
        Some(at.checked_duration_since(self.origin)?.as_nanos() / self.interval.as_nanos().max(1))
    }

    pub fn record(&mut self, at: Instant, percent: Option<f64>) {
        let Some(slot) = self.slot(at) else { return };
        let value = percent.filter(|value| value.is_finite() && (0.0..=100.0).contains(value));
        if let Some((last_slot, last_value)) = self.points.back_mut() {
            if slot < *last_slot {
                return;
            }
            if slot == *last_slot {
                *last_value = value;
                return;
            }
        }
        if self.points.len() == HISTORY_LEN {
            self.points.pop_front();
        }
        self.points.push_back((slot, value));
    }

    /// Both metrics share a clock, so all columns shift together. A predecessor
    /// observation supplies the held value for slots without a new result.
    pub fn sparkline(&self, columns: usize, now: Instant, ascii: bool) -> String {
        let columns = columns.min(HISTORY_LEN);
        let Some(current) = self.slot(now) else {
            return " ".repeat(columns);
        };
        let levels = if ascii {
            ['_', '.', ':', '-', '=', '+', '*', '#']
        } else {
            ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']
        };
        (0..columns)
            .map(|column| {
                let Some(slot) = current.checked_sub((columns - 1 - column) as u128) else {
                    return ' ';
                };
                match self.points.iter().rev().find(|(at, _)| *at <= slot) {
                    None => ' ',            // No observation yet, not a measured zero.
                    Some((_, None)) => '!', // Unavailable, not a healthy held value.
                    Some((_, Some(value))) => levels[((value * 8.0 / 100.0) as usize).min(7)],
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_values_on_fixed_clock_and_scrolls_the_entire_graph_left() {
        let start = Instant::now();
        let mut history = MetricHistory::new(start, Duration::from_secs(3));
        history.record(start + Duration::from_millis(800), Some(0.0));
        history.record(start + Duration::from_millis(3400), Some(50.0));
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(6), false),
            " ▁▅▅"
        );
        // Observation-relative ages cross boundaries here, but clock slots do not.
        assert_eq!(
            history.sparkline(4, start + Duration::from_millis(8900), false),
            " ▁▅▅"
        );
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(9), false),
            "▁▅▅▅"
        );
        history.record(start + Duration::from_millis(9300), Some(100.0));
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(10), false),
            "▁▅▅█"
        );
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(10), true),
            "_==#"
        );
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(12), false),
            "▅▅██"
        );
    }

    #[test]
    fn errors_differ_from_waiting_and_zero_until_recovery() {
        let start = Instant::now();
        let mut history = MetricHistory::new(start, Duration::from_secs(3));
        assert_eq!(history.sparkline(4, start, false), "    ");
        history.record(start, Some(0.0));
        history.record(start + Duration::from_secs(3), None);
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(9), false),
            "▁!!!"
        );
        history.record(start + Duration::from_secs(9), Some(50.0));
        assert_eq!(
            history.sparkline(4, start + Duration::from_secs(9), false),
            "▁!!▅"
        );
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 101.0] {
            history.record(start + Duration::from_secs(9), Some(invalid));
            assert_eq!(
                history.sparkline(1, start + Duration::from_secs(9), true),
                "!"
            );
        }
    }

    #[test]
    fn coalesces_fast_results_without_losing_the_visible_predecessor() {
        let start = Instant::now();
        let mut history = MetricHistory::new(start, Duration::from_secs(1));
        history.record(start, Some(0.0));
        for index in 0..100 {
            history.record(start + Duration::from_millis(11000 + index), Some(100.0));
        }
        assert_eq!(history.points.len(), 2);
        assert_eq!(
            history.sparkline(12, start + Duration::from_secs(12), false),
            "▁▁▁▁▁▁▁▁▁▁██"
        );
        for index in 12..100 {
            history.record(start + Duration::from_secs(index), Some(100.0));
        }
        assert_eq!(history.points.len(), HISTORY_LEN);
        assert_eq!(history.sparkline(12, start, false), "            ");
        assert_eq!(
            history.sparkline(12, start + Duration::from_secs(99), false),
            "████████████"
        );
    }

    #[test]
    fn cpu_and_memory_share_boundaries_despite_different_arrival_times() {
        let start = Instant::now();
        let mut history = HostHistory::new(start, Duration::from_secs(3));
        history
            .memory
            .record(start + Duration::from_millis(500), Some(50.0));
        history
            .cpu
            .record(start + Duration::from_millis(1500), Some(50.0));
        for millis in [1500, 3000, 3500, 4500, 5999, 6000] {
            let now = start + Duration::from_millis(millis);
            assert_eq!(
                history.cpu.sparkline(6, now, false),
                history.memory.sparkline(6, now, false)
            );
        }
    }
}
