//! MeltAlarm's cable card in a terminal (SPEC §7.4): the full card, the compact card
//! (`floating-compact.png`) and the one-line form.
//!
//! Layout, state words and every status text are MeltAlarm (MIT, © 2026 Denis Domanskii, github.com/DeniDoman/meltalarm):
//! its flyout and floating-monitor designs, and the texts `meltalarm-core` produces.

use meltalarm_core::Level;
use meltalarm_model::WIRES;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use super::cutbar::{self, CutBar};
use super::theme::{Role, Theme};
use crate::monitor::Card;

pub const FULL_WIDTH: u16 = 57;
pub const COMPACT_WIDTH: u16 = 30;
pub const COMPACT_HEIGHT: u16 = 11;
pub const BAR_ROWS: u16 = 11;
pub const MIN_BAR_ROWS: u16 = 7;

/// Height of a full card with `bar_rows` bar rows.
pub fn full_height(bar_rows: u16, peaks: bool) -> u16 {
    bar_rows + 9 + u16::from(peaks)
}

pub struct Opts<'a> {
    pub bar_rows: u16,
    /// Zoom view: per-wire peaks since start or `c`.
    pub peaks: Option<&'a [Option<f32>; WIRES]>,
    /// One frame of reverse video on the pill after a state change.
    pub flash: bool,
}

fn amps1(a: Option<f32>) -> String {
    a.map_or("—".into(), |a| format!("{a:.1}"))
}

fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, right: u16) {
    if x < right {
        buf.set_stringn(x, y, text, (right - x) as usize, style);
    }
}

/// The `( ● OK )` pill as spans.
pub fn pill<'a>(card: &Card, theme: &Theme, flash: bool) -> Vec<Span<'a>> {
    let mut style = theme.pill(card.status);
    if flash {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let (l, r) = theme.pill_caps();
    let cap = Style::new().fg(style.bg.unwrap_or_default());
    let text = theme.pill_text(card.status, &card.status_text);
    if theme.mono() {
        return vec![Span::styled(format!("({text})"), style)];
    }
    let mut spans = vec![Span::styled(l, cap)];
    match theme.pill_dot(card.status) {
        Some(dot) => {
            let dot = if flash { dot.add_modifier(Modifier::REVERSED) } else { dot };
            spans.push(Span::styled(" ", style));
            spans.push(Span::styled("●", dot));
            spans.push(Span::styled(format!(" {} ", card.status_text), style));
        }
        None => spans.push(Span::styled(text, style)),
    }
    spans.push(Span::styled(r, cap));
    spans
}

fn block<'a>(card: &Card, theme: &Theme, title: &str, flash: bool, right: Option<&str>) -> Block<'a> {
    let mut left = vec![
        Span::styled(" ", theme.border(card.in_alarm())),
        Span::styled(title.to_owned(), theme.on_card(Role::Text).add_modifier(Modifier::BOLD)),
        Span::styled("  ", theme.card()),
    ];
    left.extend(pill(card, theme, flash));
    left.push(Span::styled(" ", theme.border(card.in_alarm())));
    let mut b = Block::new()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(theme.border(card.in_alarm()))
        .style(theme.card())
        .title(Line::from(left));
    if let Some(gpu) = right {
        b = b.title_top(Line::from(vec![Span::styled(format!(" {gpu} "), theme.on_card(Role::Text2))]).right_aligned());
    }
    b
}

/// x of each bar, spread over `width` like the flyout.
fn bar_xs(x: u16, width: u16, max_gap: u16) -> [u16; WIRES] {
    let n = WIRES as u16;
    let gap = (width.saturating_sub(n * cutbar::WIDTH + 4) / (n - 1)).clamp(1, max_gap);
    let total = n * cutbar::WIDTH + (n - 1) * gap;
    let left = x + width.saturating_sub(total) / 2;
    std::array::from_fn(|i| left + i as u16 * (cutbar::WIDTH + gap))
}

fn bars(card: &Card, xs: &[u16; WIRES], y: u16, rows: u16, buf: &mut Buffer, theme: &Theme) {
    for (i, &x) in xs.iter().enumerate() {
        CutBar { wire: card.wires[i], rating: card.rating, limit: card.limit, theme }.render(Rect::new(x, y, cutbar::WIDTH, rows), buf);
    }
}

fn values(card: &Card, xs: &[u16; WIRES], y: u16, buf: &mut Buffer, theme: &Theme, right: u16) {
    for (i, &x) in xs.iter().enumerate() {
        let s = amps1(card.wires[i].amps);
        let len = s.chars().count() as u16;
        let style = if card.wires[i].amps.is_none() { theme.on_card(Role::Text2) } else { theme.value(card.wires[i].level) };
        put(buf, (x + 1).saturating_sub(len / 2), y, &s, style, right);
    }
}

fn line_style(theme: &Theme, level: Level) -> Style {
    match level {
        Level::Normal => theme.on_card(Role::Text2),
        l => theme.on_card(theme.level_text(l)),
    }
}

pub fn full(card: &Card, area: Rect, buf: &mut Buffer, theme: &Theme, opts: &Opts) {
    let title = if area.width >= 34 { card.label.as_str() } else { card.short.as_str() };
    let block = block(card, theme, title, opts.flash, card.gpu.as_deref().filter(|g| area.width as usize > g.len() + 36));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height < opts.bar_rows + 5 || inner.width < 26 {
        return;
    }
    let right = inner.right();
    let spare = inner.height.saturating_sub(opts.bar_rows + 6 + u16::from(opts.peaks.is_some()));
    let mut y = inner.y;
    put(buf, inner.x + 1, y, "Current per wire, A", theme.on_card(Role::Text2), right);
    y += 1 + u16::from(spare >= 1);
    let xs = bar_xs(inner.x, inner.width, 8);
    bars(card, &xs, y, opts.bar_rows, buf, theme);
    if let Some(peaks) = opts.peaks {
        let g = cutbar::Geometry::new(opts.bar_rows, card.rating, card.limit);
        for (i, &x) in xs.iter().enumerate() {
            let f = g.fill(peaks[i], card.rating, card.limit);
            if f > 0 && x + 3 < right {
                let row = (opts.bar_rows as usize - 1).saturating_sub(g.row_of(f));
                buf[(x + 3, y + row as u16)].set_char('◂').set_style(theme.on_card(Role::Text2));
            }
        }
    }
    y += opts.bar_rows;
    values(card, &xs, y, buf, theme, right);
    y += 1;
    if let Some(peaks) = opts.peaks {
        for (i, &x) in xs.iter().enumerate() {
            put(buf, x.saturating_sub(1), y, &format!("{:>4}", amps1(peaks[i])), theme.on_card(Role::Text2), right);
        }
        put(buf, inner.x + 1, y, "peak", theme.on_card(Role::Text2), right);
        y += 1;
    }
    if let Some((text, level)) = &card.reason {
        put(buf, inner.x + 1, y, text, line_style(theme, *level), right);
    }
    y += 1;
    let divider: String = "─".repeat(inner.width.saturating_sub(2) as usize);
    put(buf, inner.x + 1, y, &divider, theme.on_card(Role::Border), right);
    y += 1;
    let col2 = inner.x + inner.width / 2;
    put(buf, inner.x + 1, y, "Imbalance", theme.on_card(Role::Text2), right);
    put(buf, col2, y, "PSU status", theme.on_card(Role::Text2), right);
    y += 1;
    let imbalance = card.imbalance.map_or("—".into(), |i| format!("{i:.1} A"));
    put(buf, inner.x + 1, y, &imbalance, theme.value(card.imbalance_level), col2.saturating_sub(1));
    put(buf, col2, y, &card.psu_status, theme.value(card.psu_level), right);
}

/// `floating-compact.png`: title and pill, bars at 6 rows plus the cut, values, one line.
pub fn compact(card: &Card, area: Rect, buf: &mut Buffer, theme: &Theme, flash: bool) {
    let long = card.label.len() + card.status_text.len() + 10 <= area.width as usize;
    let title = if long { card.label.as_str() } else { card.short.as_str() };
    let block = block(card, theme, title, flash, None);
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height < 5 || inner.width < 24 {
        return;
    }
    let rows = inner.height.saturating_sub(2).min(BAR_ROWS);
    let xs = bar_xs(inner.x, inner.width, 4);
    bars(card, &xs, inner.y, rows, buf, theme);
    values(card, &xs, inner.y + rows, buf, theme, inner.right());
    let (text, level) = &card.summary;
    put(buf, inner.x + 1, inner.y + rows + 1, text, line_style(theme, *level), inner.right());
}

/// `GPU power cable 1 ( ● OK )  ▆▅▅▅▆▆  8.1 7.8 7.7 7.8 8.0 8.1  Imbalance 0.4 A`, shortened to fit.
pub fn one_line<'a>(card: &Card, theme: &Theme, width: u16, flash: bool) -> Line<'a> {
    let plain = |s: String| Span::raw(s);
    let minibars: Vec<Span> = card
        .wires
        .iter()
        .map(|w| {
            let ch = cutbar::mini(w.amps, card.limit).to_string();
            let style = if theme.mono() {
                if w.level == Level::Warning { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() }
            } else {
                theme.fg(theme.level_gfx(w.level))
            };
            Span::styled(ch, style)
        })
        .collect();
    let vals = card.wires.iter().map(|w| amps1(w.amps)).collect::<Vec<_>>().join(" ");
    let (summary, level) = card.summary.clone();
    let summary_style = match level {
        Level::Normal => theme.dim(),
        l => theme.fg(theme.level_text(l)),
    };
    let pill = pill(card, theme, flash);
    let pill_w: usize = pill.iter().map(|s| s.width()).sum();
    let budget = width as usize;
    let label = if 18 + pill_w + 12 + 9 + 2 + summary.len() <= budget { card.label.clone() } else { card.short.clone() };
    let label = format!("{label:<w$}", w = if label.len() > 8 { 17 } else { 7 });
    let mut used = label.len() + 1 + pill_w;
    let mut spans = vec![Span::styled(label, Style::new().add_modifier(Modifier::BOLD)), plain(" ".into())];
    spans.extend(pill);
    let pad = 11usize.saturating_sub(pill_w);
    if used + pad + 7 <= budget {
        spans.push(plain(" ".repeat(pad + 1)));
        spans.extend(minibars);
        used += pad + 7;
    }
    if used + 2 + summary.len() <= budget {
        let with_values = used + 2 + vals.len() + 2 + summary.len() <= budget;
        if with_values {
            spans.push(plain(format!("  {vals}")));
        }
        spans.push(plain("  ".into()));
        spans.push(Span::styled(summary, summary_style));
    } else if used + 2 + vals.len() <= budget {
        spans.push(plain(format!("  {vals}")));
    }
    Line::from(spans)
}
