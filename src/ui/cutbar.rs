//! The cut bar (SPEC §7.4) on `meltalarm_core::bar_share`.
//!
//! The cut-bar design (top = alarm limit, cut at the rating, knee scale, colour = state) is
//! MeltAlarm (MIT, © 2026 Denis Domanskii, github.com/DeniDoman/meltalarm), `docs/DESIGN.md` "Cut bars"; this is a terminal rendering of it.
//!
//! The top of the bar is the alarm limit; the bar is cut straight across at the rating; the
//! fill takes the wire's state colour and jumps the cut from the rating. With colour, a bar is a
//! flat-ended column measured in eighths of a row: the fill's top is a lower-eighth block
//! (`▁`…`▇`), and the cut is a thin gap of background at the bottom of the head's lowest cell.
//! Quadrant caps were tried first; terminals draw them as steps, not curves.
//! Without colour it is drawn per row: `░` track, `█` fill, `▓` caution, reverse video at the
//! alarm, and a blank row for the cut.

use meltalarm_core::{Level, WireView, bar_share};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::theme::{Role, Theme};

pub const WIDTH: u16 = 3;
/// The cut, in eighths of a row.
pub const GAP: usize = 2;
const LOWER: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// A bar in units of an eighth of a row: `body` units below the cut, `head` units above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub rows: usize,
    pub body_rows: usize,
    pub body: usize,
    pub head: usize,
}

impl Geometry {
    /// The cut lands on a row boundary as close to the rating's knee-scale share as rows allow.
    pub fn new(rows: u16, rating: f32, limit: f32) -> Self {
        let rows = (rows as usize).max(2);
        let units = 8 * rows - GAP;
        let body_rows = ((units as f32 * bar_share(rating, limit) / 8.0).round() as usize).clamp(1, rows - 1);
        Geometry { rows, body_rows, body: 8 * body_rows, head: 8 * (rows - body_rows) - GAP }
    }

    pub fn total(&self) -> usize {
        self.body + self.head
    }

    /// Filled units for `amps`: the knee scale stretched so the cut sits exactly at the rating.
    /// Below the rating the fill never passes the cut; from the rating it always does; the bar
    /// is full only at the alarm limit.
    pub fn fill(&self, amps: Option<f32>, rating: f32, limit: f32) -> usize {
        let Some(a) = amps else { return 0 };
        if a >= limit {
            return self.total();
        }
        let sr = bar_share(rating, limit);
        if a < rating {
            let f = (bar_share(a, limit) / sr * self.body as f32).round() as usize;
            return if a > 0.0 { f.clamp(1, self.body) } else { 0 };
        }
        let h = ((bar_share(a, limit) - sr) / (1.0 - sr) * self.head as f32).round() as usize;
        self.body + h.clamp(1, self.head - 1)
    }

    /// The row (from the bottom) holding the top of a fill of `units`.
    pub fn row_of(&self, units: usize) -> usize {
        let u = units.max(1);
        if u <= self.body {
            return (u - 1) / 8;
        }
        let h = u - self.body;
        self.body_rows + if h <= 8 - GAP { 0 } else { 1 + (h - (8 - GAP) - 1) / 8 }
    }
}

pub struct CutBar<'a> {
    pub wire: WireView,
    pub rating: f32,
    pub limit: f32,
    pub theme: &'a Theme,
}

impl CutBar<'_> {
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width < WIDTH || area.height < 3 {
            return;
        }
        let g = Geometry::new(area.height, self.rating, self.limit);
        let fill = g.fill(self.wire.amps, self.rating, self.limit);
        if self.theme.mono() {
            return self.render_mono(area, buf, g, fill);
        }
        let level = if fill == g.total() { Level::Warning } else { self.wire.level };
        let fill_c = self.theme.color(self.theme.level_gfx(level));
        let track_c = self.theme.color(Role::Track);
        let card_c = self.theme.color(Role::CardBg);
        // A cell `filled` eighths from its bottom: fill below, track above.
        let cell = |filled: usize| -> (char, Style) {
            match filled {
                0 => ('█', Style::new().fg(track_c).bg(track_c)),
                8 => ('█', Style::new().fg(fill_c).bg(fill_c)),
                n => (LOWER[n], Style::new().fg(fill_c).bg(track_c)),
            }
        };
        let head_fill = fill.saturating_sub(g.body);
        for k in 0..g.rows {
            let (ch, style) = if k < g.body_rows {
                cell(fill.saturating_sub(8 * k).min(8))
            } else if k == g.body_rows {
                // The head's lowest cell: the cut at its bottom, then head (filled whole once the
                // fill jumps the cut, so the cell never needs three colours).
                let above = if head_fill > 0 { fill_c } else { track_c };
                if card_c == Color::Reset {
                    ('▂', Style::new().fg(above).add_modifier(Modifier::REVERSED))
                } else {
                    ('▂', Style::new().fg(card_c).bg(above))
                }
            } else {
                let start = (8 - GAP) + 8 * (k - g.body_rows - 1);
                cell(head_fill.saturating_sub(start).min(8))
            };
            let y = area.y + (g.rows - 1 - k) as u16;
            for x in 0..WIDTH {
                buf[(area.x + x, y)].set_char(ch).set_style(style);
            }
        }
    }

    /// One state per row: the cut is a blank row; caution `▓`, alarm in reverse video.
    fn render_mono(&self, area: Rect, buf: &mut Buffer, g: Geometry, fill: usize) {
        let head_rows = g.rows - g.body_rows - 1;
        let filled = if fill <= g.body { fill.div_ceil(8) } else { g.body_rows + 1 + ((fill - g.body) * head_rows).div_ceil(g.head) };
        let alarm = fill == g.total() || self.wire.level == Level::Warning;
        for k in 0..g.rows {
            let (ch, style) = if k == g.body_rows {
                (' ', Style::new())
            } else {
                match (k < filled, self.wire.level) {
                    (false, _) => ('░', Style::new()),
                    (true, _) if alarm => ('█', Style::new().add_modifier(Modifier::REVERSED)),
                    (true, Level::Caution) => ('▓', Style::new()),
                    (true, _) => ('█', Style::new()),
                }
            };
            let y = area.y + (g.rows - 1 - k) as u16;
            for x in 0..WIDTH {
                buf[(area.x + x, y)].set_char(ch).set_style(style);
            }
        }
    }
}

/// `▁▂▃▄▅▆▇█` on the knee scale, for the one-line layout and `watch` (SPEC §7.6, §7.7).
pub fn mini(amps: Option<f32>, limit: f32) -> char {
    match amps {
        None => '·',
        Some(a) => LOWER[((bar_share(a, limit) * 8.0).round() as usize).clamp(1, 8)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Depth;

    const R: f32 = 9.5;
    const L: f32 = 10.5;

    #[test]
    fn geometry_follows_the_knee_scale() {
        let g = Geometry::new(11, R, L);
        assert_eq!(g, Geometry { rows: 11, body_rows: 9, body: 72, head: 14 });
        assert_eq!(g.fill(Some(8.1), R, L), 50);
        assert_eq!(g.fill(Some(9.49), R, L), 72, "just below the rating the fill reaches the cut, not past it");
        assert_eq!(g.fill(Some(9.5), R, L), 73, "from the rating the fill jumps the cut");
        assert_eq!(g.fill(Some(10.4), R, L), 85, "full only at the alarm limit");
        assert_eq!(g.fill(Some(12.4), R, L), 86);
        assert_eq!(g.fill(Some(0.125), R, L), 1, "any current shows");
        assert_eq!(g.fill(Some(0.0), R, L), 0);
        assert_eq!(g.fill(None, R, L), 0);
        assert_eq!((g.row_of(50), g.row_of(72), g.row_of(73), g.row_of(78), g.row_of(79), g.row_of(86)), (6, 8, 9, 9, 10, 10));
    }

    fn draw(amps: Option<f32>, level: Level, rows: u16, depth: Depth) -> (Vec<String>, Buffer) {
        let theme = Theme::new(depth, false);
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, rows));
        CutBar { wire: WireView { amps, level, flagged: false }, rating: R, limit: L, theme: &theme }.render(buf.area, &mut buf);
        ((0..rows).map(|y| (0..3).map(|x| buf[(x, y)].symbol().to_owned()).collect()).collect(), buf)
    }

    #[test]
    fn flat_ends_eighth_block_fill_and_a_thin_cut() {
        let theme = Theme::new(Depth::True, false);
        let (rows, buf) = draw(Some(8.1), Level::Normal, 11, Depth::True);
        assert_eq!(rows[0], "███", "flat top");
        assert_eq!(rows[1], "▂▂▂", "the cut at the bottom of the head");
        // Cards have no fill, so the gap is the terminal's background: drawn in reverse video.
        assert!(buf[(0, 1)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buf[(0, 1)].fg, theme.color(Role::Track));
        // 50 units: six full rows, then two eighths.
        assert_eq!(rows[4], "▂▂▂");
        assert_eq!((buf[(1, 4)].fg, buf[(1, 4)].bg), (theme.color(Role::Neutral), theme.color(Role::Track)));
        assert_eq!(buf[(1, 5)].fg, theme.color(Role::Neutral));
        assert_eq!(rows[10], "███", "flat bottom");
    }

    #[test]
    fn caution_fills_the_head_cell_and_a_full_bar_is_red() {
        let theme = Theme::new(Depth::True, false);
        let (_, buf) = draw(Some(9.9), Level::Caution, 11, Depth::True);
        // The cut cell is drawn reversed: the colour above the gap is its foreground.
        assert_eq!(buf[(1, 1)].fg, theme.color(Role::Caution), "the fill jumped the cut");
        let (_, buf) = draw(Some(12.4), Level::Warning, 11, Depth::True);
        assert!((0..11).all(|y| y == 1 || buf[(1, y)].fg == theme.color(Role::WarnGfx)));
    }

    #[test]
    fn sixteen_colours_draw_the_cut_in_reverse_video() {
        let (rows, buf) = draw(Some(8.1), Level::Normal, 11, Depth::C16);
        assert_eq!(rows[1], "▂▂▂");
        assert!(buf[(0, 1)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn mono_rows() {
        let (rows, _) = draw(Some(9.9), Level::Caution, 11, Depth::Mono);
        assert_eq!(rows[0], "▓▓▓");
        assert_eq!(rows[1], "   ");
        assert_eq!(rows[2], "▓▓▓");
        let (rows, _) = draw(Some(8.1), Level::Normal, 11, Depth::Mono);
        assert_eq!((rows[0].as_str(), rows[2].as_str(), rows[4].as_str()), ("░░░", "░░░", "███"));
    }

    #[test]
    fn mini_steps() {
        assert_eq!(mini(Some(0.1875), L), '▁');
        assert_eq!(mini(Some(8.1), L), '▅');
        assert_eq!(mini(Some(10.5), L), '█');
        assert_eq!(mini(None, L), '·');
    }
}
