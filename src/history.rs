//! The last hour of samples for the chart, and per-wire peaks for the zoom view.

use std::collections::VecDeque;
use std::time::Duration;

use meltalarm_model::WIRES;

pub const SPAN: Duration = Duration::from_secs(3600);
/// The chart windows `F6` and `←`/`→` step through.
pub const WINDOWS: [Duration; 4] = [Duration::from_secs(60), Duration::from_secs(300), Duration::from_secs(900), Duration::from_secs(3600)];

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// Seconds since the monitor started.
    pub t: f64,
    /// Sum of the wires × 12 V.
    pub cable_w: [Option<f32>; 2],
    pub max_wire: [Option<f32>; 2],
}

pub struct History {
    points: VecDeque<Point>,
    cap: usize,
    /// The poll interval, seconds: one sample per interval, as nvtop keeps one per update.
    interval: f64,
    /// Samples ever stored, gap fillers included: the index the next one gets.
    next: u64,
    peaks: [[Option<f32>; WIRES]; 2],
}

impl History {
    pub fn new(interval: Duration) -> Self {
        let cap = (SPAN.as_secs_f64() / interval.as_secs_f64().max(0.1)).ceil() as usize + 1;
        History {
            points: VecDeque::with_capacity(cap.min(8192)),
            cap,
            interval: interval.as_secs_f64().max(0.1),
            next: 0,
            peaks: [[None; WIRES]; 2],
        }
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs_f64(self.interval)
    }

    /// Stores `p`. Samples missed while the PSU was away (two intervals or more since the last
    /// one) become empty samples first, so every sample stands for one interval and the chart's
    /// time axis stays true.
    pub fn push(&mut self, p: Point) {
        if let Some(last) = self.latest_t() {
            let gap = p.t - last;
            if gap >= 2.0 * self.interval {
                let missed = ((gap / self.interval).round() as usize).saturating_sub(1);
                for i in missed.saturating_sub(self.cap) + 1..=missed {
                    self.store(Point { t: last + i as f64 * self.interval, cable_w: [None; 2], max_wire: [None; 2] });
                }
            }
        }
        self.store(p);
    }

    fn store(&mut self, p: Point) {
        if self.points.len() == self.cap {
            self.points.pop_front();
        }
        self.points.push_back(p);
        self.next += 1;
    }

    pub fn note_wires(&mut self, cable: usize, wires: &[Option<f32>; WIRES]) {
        for (peak, w) in self.peaks[cable].iter_mut().zip(wires) {
            if let Some(a) = *w {
                *peak = Some(peak.map_or(a, |p: f32| p.max(a)));
            }
        }
    }

    pub fn peaks(&self) -> &[[Option<f32>; WIRES]; 2] {
        &self.peaks
    }

    pub fn clear_peaks(&mut self) {
        self.peaks = [[None; WIRES]; 2];
    }

    pub fn latest_t(&self) -> Option<f64> {
        self.points.back().map(|p| p.t)
    }

    /// Samples up to `end` (a paused display's last moment), oldest first, each with its index
    /// since the start (gap fillers count).
    pub fn indexed_until(&self, end: f64) -> impl DoubleEndedIterator<Item = (u64, &Point)> {
        let first = self.next - self.points.len() as u64;
        self.points.iter().enumerate().map(move |(i, p)| (first + i as u64, p)).filter(move |(_, p)| p.t <= end)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: f64) -> Point {
        Point { t, cable_w: [Some(t as f32), None], max_wire: [None, None] }
    }

    #[test]
    fn keeps_one_hour_and_indexes_from_the_start() {
        let mut h = History::new(Duration::from_secs(1));
        for t in 0..4000 {
            h.push(p(t as f64));
        }
        assert_eq!(h.len(), 3601);
        let all: Vec<(u64, f64)> = h.indexed_until(f64::INFINITY).map(|(i, p)| (i, p.t)).collect();
        assert_eq!((all[0], all[3600]), ((399, 399.0), (3999, 3999.0)));
        assert_eq!(h.indexed_until(3500.0).next_back().map(|(i, _)| i), Some(3500));
    }

    #[test]
    fn missed_samples_become_empty_ones_but_jitter_does_not() {
        let mut h = History::new(Duration::from_secs(1));
        for t in [0.0, 1.0, 2.6, 3.4, 7.5] {
            h.push(p(t));
        }
        let got: Vec<(u64, Option<f32>)> = h.indexed_until(f64::INFINITY).map(|(i, p)| (i, p.cable_w[0])).collect();
        // 2.6 and 3.4 are late and early, not missing; 4–6 never came.
        assert_eq!(got.len(), 8);
        assert_eq!(got[3..].iter().map(|(_, v)| v.is_some()).collect::<Vec<_>>(), [true, false, false, false, true]);
        assert_eq!(got.last().unwrap().0, 7);
    }

    #[test]
    fn peaks_track_the_maximum_until_cleared() {
        let mut h = History::new(Duration::from_secs(1));
        h.note_wires(1, &[Some(8.0), None, Some(1.0), None, None, None]);
        h.note_wires(1, &[Some(7.0), Some(2.0), Some(9.9), None, None, None]);
        assert_eq!(h.peaks()[1][..3], [Some(8.0), Some(2.0), Some(9.9)]);
        h.clear_peaks();
        assert_eq!(h.peaks()[1][0], None);
    }
}
