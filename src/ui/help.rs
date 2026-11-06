//! The help overlay (`F1`/`?`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::theme::Theme;

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn frame<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    Block::new().borders(Borders::ALL).border_set(border::ROUNDED).border_style(theme.dim()).title(title)
}

pub fn help(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let key = |k: &str, what: &str| {
        Line::from(vec![Span::styled(format!(" {k:<14}"), Style::new().add_modifier(Modifier::BOLD)), Span::raw(what.to_owned())])
    };
    let lines = vec![
        key("F1  ?", "this help"),
        key("F3  r", "raw frames instead of the chart"),
        key("F5  p", "pause the display (an alarm resumes it)"),
        key("F6  ← →", "chart window: 1 / 5 / 15 / 60 min"),
        key("F9  l", "CSV log on/off (~/.local/state/psutop/)"),
        key("1  2", "zoom one cable; again to go back"),
        key("c", "clear peak markers (zoom view)"),
        key("F10  q  Esc", "quit"),
        Line::raw(""),
        Line::raw(" States: OK · Caution · ALARM · No data · Not connected"),
        Line::raw(" Bars: the top is the alarm limit, the cut is the rating."),
        Line::raw(" Only the PSU cuts power; psutop is a monitor."),
        Line::styled(" Cable labels (GPU 0 / GPU 1) are set in ~/.config/psutop/config.toml.", theme.dim()),
        Line::raw(""),
        Line::styled(" Built on MeltAlarm (MIT) © 2026 Denis Domanskii", theme.dim()),
        Line::styled(" github.com/DeniDoman/meltalarm", theme.dim()),
    ];
    let r = centered(area, 64, lines.len() as u16 + 2);
    Clear.render(r, buf);
    Paragraph::new(lines).block(frame(" Help ", theme)).render(r, buf);
}
