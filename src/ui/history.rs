//! The history chart, drawn the way nvtop draws its plots (SPEC §7.5). nvtop is GPL-3.0; this
//! follows its approach and uses none of its code.
//!
//! - One slot per sample: each slot holds a whole number of samples (one, unless the window
//!   needs more), counted from the start, so a sample never moves to another slot and the chart
//!   scrolls one whole slot at a time. Nothing is averaged; a slot shows its highest value.
//! - The lines take turns by column: in each slot cable 1 has the left column and cable 2 the
//!   right. In a cable's column the other cable's line carries on at its level and meets a step
//!   with `┴ ┬ ┼`, so both lines stay whole and neither hides the other.
//! - A fixed scale: 0 to 20 A, ticks every 5 A, so the shape never rescales.
//! - Step lines in box-drawing characters, time ticks under the frame, the legend (each cable's
//!   name and its newest value) in colour at the top left of the plot.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders, Widget};

use super::theme::Theme;
use crate::history::History;

/// The top of the amps scale. A wire above it draws on the top row.
pub const TOP_A: f32 = 20.0;

pub fn window_name(w: Duration) -> String {
    format!("{} min", w.as_secs() / 60)
}

/// `-5m`, `-3m45s`, `-30s`, `0s`.
pub fn ago(secs: u64) -> String {
    match secs {
        0 => "0s".into(),
        s if s < 60 => format!("-{s}s"),
        s if s % 60 == 0 => format!("-{}m", s / 60),
        s => format!("-{}m{}s", s / 60, s % 60),
    }
}

/// Samples per slot: the fewest that fit `window` into `n` slots.
pub fn per_slot(window: Duration, interval: Duration, n: usize) -> u64 {
    let span = interval.as_secs_f64() * n.max(1) as f64;
    ((window.as_secs_f64() / span).ceil() as u64).max(1)
}

/// The last `n` slots, oldest first. Slot `s` holds the samples whose index is in
/// `s·per .. (s+1)·per`; its value is their highest, `None` when none of them has one.
pub fn slots(samples: &[(u64, Option<f32>)], per: u64, n: usize) -> Vec<Option<f32>> {
    let mut out = vec![None; n];
    let Some(&(last, _)) = samples.last() else { return out };
    let newest = last / per;
    for &(i, v) in samples.iter().rev() {
        let back = (newest - i / per) as usize;
        if back >= n {
            break;
        }
        if let Some(v) = v {
            let slot = &mut out[n - 1 - back];
            *slot = Some(slot.map_or(v, |s| s.max(v)));
        }
    }
    out
}

struct Plot<'a> {
    area: Rect,
    buf: &'a mut Buffer,
}

impl Plot<'_> {
    /// `row` counts from the bottom.
    fn put(&mut self, x: usize, row: usize, ch: char, style: Style) {
        let (w, h) = (self.area.width as usize, self.area.height as usize);
        if x < w && row < h {
            self.buf[(self.area.x + x as u16, self.area.y + (h - 1 - row) as u16)].set_char(ch).set_style(style);
        }
    }

    /// Step lines, one per entry of `rows` (each a row per slot, `None` for no value), taking turns
    /// by column and right-aligned in the plot. In line `k`'s column: a level is `─`; a rise is
    /// `┘│┌` and a fall `┐│└`. Every other line carries on at its level: `─` in its own colour, or
    /// a tee or cross where it meets the step (`┴` at its low end, `┬` at its high end, `┼` on it).
    // Slot `i` indexes every line's rows at once.
    #[allow(clippy::needless_range_loop)]
    fn lines(&mut self, rows: &[Vec<Option<usize>>], styles: &[Style]) {
        let lines = rows.len();
        let Some(n) = rows.first().map(Vec::len) else { return };
        let off = (self.area.width as usize).saturating_sub(n * lines);
        let mut level: Vec<Option<usize>> = rows.iter().map(|r| r[0]).collect();
        for i in 0..n {
            for k in 0..lines {
                let x = off + i * lines + k;
                let (before, now) = (level[k], rows[k][i]);
                let style = styles[k];
                let step = match (before, now) {
                    (Some(a), Some(b)) if a != b => Some((a, b)),
                    _ => None,
                };
                if let Some((a, b)) = step {
                    let up = b > a;
                    for y in a.min(b) + 1..a.max(b) {
                        self.put(x, y, '│', style);
                    }
                    self.put(x, a, if up { '┘' } else { '┐' }, style);
                    self.put(x, b, if up { '┌' } else { '└' }, style);
                } else if let Some(b) = now {
                    self.put(x, b, '─', style);
                }
                for j in (0..lines).filter(|&j| j != k) {
                    let Some(l) = level[j] else { continue };
                    match step.map(|(a, b)| (a.min(b), a.max(b))) {
                        Some((lo, _)) if l == lo => self.put(x, l, '┴', style),
                        Some((_, hi)) if l == hi => self.put(x, l, '┬', style),
                        Some((lo, hi)) if lo < l && l < hi => self.put(x, l, '┼', style),
                        _ if now == Some(l) => {}
                        _ => self.put(x, l, '─', styles[j]),
                    }
                }
                level[k] = now;
            }
        }
    }
}

/// `names`: each line's legend name, the GPU its cable feeds ("GPU 0", or the config's label).
/// Each cable's line is its hottest wire, in the cable's colour.
/// `end`: the moment the chart ends, the newest sample unless the display is paused.
pub fn render(hist: &History, window: Duration, end: Option<f64>, names: &[String; 2], area: Rect, buf: &mut Buffer, theme: &Theme) {
    if area.height < 4 || area.width < 30 {
        return;
    }
    let colors = [theme.series(0), theme.series(1)];
    let dim = theme.dim();
    let (lw, rw) = (8u16, 2u16);
    let x_labels = area.height >= 7;
    let frame = Rect::new(area.x + lw, area.y, area.width - lw - rw, area.height - u16::from(x_labels));
    let title = if area.width >= 60 { format!(" History · {} ", window_name(window)) } else { format!(" {} ", window_name(window)) };
    let block = Block::new().borders(Borders::ALL).border_set(border::ROUNDED).border_style(dim).title(title);
    let plot = block.inner(frame);
    block.render(frame, buf);
    let (pw, ph) = (plot.width as usize, plot.height as usize);
    if pw < 4 || ph < 2 {
        return;
    }

    // Two columns per slot, one per cable.
    let n = pw / 2;
    let interval = hist.interval();
    let per = per_slot(window, interval, n);

    // Ticks: quarters when there is room, else halves, else the ends.
    let fracs: &[f32] = if ph >= 9 {
        &[0.0, 0.25, 0.5, 0.75, 1.0]
    } else if ph >= 5 {
        &[0.0, 0.5, 1.0]
    } else {
        &[0.0, 1.0]
    };
    for &f in fracs {
        let y = plot.y + ((1.0 - f) * (ph - 1) as f32).round() as u16;
        buf.set_stringn(area.x, y, format!("{:>7}", format!("{:.0} A", TOP_A * f)), lw as usize - 1, dim);
    }
    if x_labels {
        // Where a moment `secs` ago sits: two columns per slot back from the newest.
        let slot_secs = per as f64 * interval.as_secs_f64();
        let col = |secs: f64| (pw as f64 - 1.0 - 2.0 * (secs / slot_secs).floor()).max(0.0) as u16;
        let ticks = if pw >= 60 { 4 } else { 2 };
        for i in 0..=ticks {
            let secs = window.as_secs() * (ticks - i) as u64 / ticks as u64;
            let x = plot.x + col(secs as f64);
            buf[(x, frame.bottom() - 1)].set_char('┬').set_style(dim);
            let text = ago(secs);
            let len = text.chars().count() as u16;
            let lx = if i == 0 {
                x
            } else if i == ticks {
                (x + 1).saturating_sub(len)
            } else {
                x.saturating_sub(len / 2)
            };
            buf.set_stringn(lx, frame.bottom(), text, len as usize, dim);
        }
    }

    let samples: Vec<_> = hist.indexed_until(end.unwrap_or(f64::INFINITY)).collect();
    if samples.is_empty() {
        buf.set_stringn(plot.x + 1, plot.y + plot.height / 2, "waiting for samples…", pw - 1, dim);
        return;
    }
    let row = |v: Option<f32>| v.map(|v| ((v / TOP_A).clamp(0.0, 1.0) * (ph - 1) as f32).round() as usize);
    let rows: Vec<Vec<Option<usize>>> = (0..2)
        .map(|c| {
            let amps: Vec<(u64, Option<f32>)> = samples.iter().map(|(i, p)| (*i, p.max_wire[c])).collect();
            slots(&amps, per, n).into_iter().map(row).collect()
        })
        .collect();
    let mut p = Plot { area: plot, buf };
    p.lines(&rows, &colors);
    // The legend goes on top of the lines, as in nvtop: one line per cable, in its colour, with
    // the line's newest value (the cable's highest wire right now, as on its card).
    let newest = samples.last().map(|(_, p)| p.max_wire).unwrap_or_default();
    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    for (i, name) in names.iter().enumerate().take(ph) {
        let value = newest[i].map_or("—".to_owned(), |a| format!("{a:.1} A"));
        p.buf.set_stringn(plot.x + 1, plot.y + i as u16, format!("{name:<width$}  {value:>6}"), pw.saturating_sub(2), colors[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Point;

    fn amps(h: &History) -> Vec<(u64, Option<f32>)> {
        h.indexed_until(f64::INFINITY).map(|(i, p)| (i, p.max_wire[0])).collect()
    }

    fn pt(t: f64, a: Option<f32>) -> Point {
        Point { t, cable_w: [None, None], max_wire: [a, None] }
    }

    #[test]
    fn labels_and_slot_sizes() {
        assert_eq!((ago(300), ago(225), ago(45), ago(0)), ("-5m".into(), "-3m45s".into(), "-45s".into(), "0s".into()));
        let s = Duration::from_secs;
        // 50 slots at 1 Hz: 1 sample each for 1 min (50 s shown), 6 for 5 min, 72 for 60 min.
        assert_eq!((per_slot(s(60), s(1), 50), per_slot(s(300), s(1), 50), per_slot(s(3600), s(1), 50)), (2, 6, 72));
        assert_eq!(per_slot(s(30), s(1), 50), 1);
    }

    #[test]
    fn slots_take_the_highest_and_keep_gaps() {
        let s = [(0, Some(1.0)), (1, Some(3.0)), (2, None), (3, None), (4, Some(2.0)), (5, Some(5.0)), (6, Some(4.0))];
        // Two per slot, by index: {0,1} {2,3} {4,5} {6}.
        assert_eq!(slots(&s, 2, 4), vec![Some(3.0), None, Some(5.0), Some(4.0)]);
        assert_eq!(slots(&s, 2, 6), vec![None, None, Some(3.0), None, Some(5.0), Some(4.0)]);
        assert_eq!(slots(&s, 1, 2), vec![Some(5.0), Some(4.0)]);
    }

    /// As in nvtop, a new sample either fills the newest slot or starts a new one and moves every
    /// other slot one place left, unchanged.
    #[test]
    fn the_chart_scrolls_one_whole_slot_at_a_time() {
        let mut h = History::new(Duration::from_secs(1));
        let mut before: Option<Vec<Option<f32>>> = None;
        for t in 0..200 {
            // Late and early samples, as a real poll loop has them.
            let jitter = [0.0, 0.3, -0.2, 0.45][t % 4];
            h.push(pt(t as f64 + jitter, Some((t * 37 % 11) as f32)));
            let now = slots(&amps(&h), 3, 8);
            if let Some(b) = before {
                assert!(b[..7] == now[..7] || b[1..] == now[..7], "t={t}: {b:?} → {now:?}");
            }
            before = Some(now);
        }
    }

    fn draw(rows: &[Vec<Option<usize>>], w: u16, h: u16) -> Vec<String> {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        Plot { area, buf: &mut buf }.lines(rows, &[Style::new(), Style::new()]);
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect()).collect()
    }

    #[test]
    fn lines_take_turns_by_column_and_meet_with_tees() {
        // Cable 1 rises from 0 to 2 and stays; cable 2 stays at 1, so it crosses cable 1's step.
        let rows = vec![vec![Some(0), Some(2), Some(2)], vec![Some(1), Some(1), Some(1)]];
        assert_eq!(draw(&rows, 6, 3), ["  ┌───", "──┼───", "──┘   "]);
        // Both at the same level: one line, drawn in turns, never broken.
        let rows = vec![vec![Some(0), Some(0)], vec![Some(0), Some(0)]];
        assert_eq!(draw(&rows, 4, 2), ["    ", "────"]);
        // Cable 2 falls onto cable 1's level: a tee where it lands.
        let rows = vec![vec![Some(0), Some(0)], vec![Some(2), Some(0)]];
        assert_eq!(draw(&rows, 4, 3), ["───┐", "   │", "───┴"]);
        // A gap breaks only its own line.
        let rows = vec![vec![Some(0), None, Some(0)], vec![Some(1), Some(1), Some(1)]];
        assert_eq!(draw(&rows, 6, 2), ["──────", "──  ──"]);
    }
}
