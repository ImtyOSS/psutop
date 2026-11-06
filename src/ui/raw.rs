//! The raw pane (`F3`): the last `E0`/`C1`/`E1`/`C0` replies in hex with their words decoded.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use super::theme::Theme;
use crate::frame::{self, RawFrames};
use crate::monitor::Snapshot;

pub fn lines<'a>(raw: &RawFrames) -> Vec<Line<'a>> {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    let mut frame_line = |name: &str, f: &Option<Vec<u8>>| match f {
        Some(f) => out.push(Line::from(vec![Span::styled(format!("{name} "), bold), Span::raw(frame::hex(f))])),
        None => out.push(Line::from(vec![Span::styled(format!("{name} "), bold), Span::raw("—")])),
    };
    frame_line("E0", &raw.e0);
    frame_line("C1", &raw.c1);
    frame_line("E1", &raw.e1);
    frame_line("C0", &raw.c0);
    out.push(Line::raw(""));
    if let Some(e0) = &raw.e0 {
        out.extend(frame::describe_e0(e0).into_iter().map(|l| Line::raw(format!("E0 {l}"))));
    }
    if let Some(c1) = &raw.c1 {
        out.push(Line::raw(format!("C1 {}", frame::describe_c1(c1))));
    }
    if let Some(e1) = &raw.e1 {
        out.push(Line::raw(format!("E1 {}", frame::describe_e1(e1))));
    }
    if let Some(c0) = &raw.c0 {
        out.push(Line::raw(format!("C0 {}", frame::describe_c0(c0))));
    }
    out
}

pub fn render(snap: &Snapshot, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let title = match &snap.raw {
        Some(r) if r.simulated => " Raw frames · simulated ",
        _ => " Raw frames ",
    };
    let block = Block::new().borders(Borders::ALL).border_set(border::ROUNDED).border_style(theme.dim()).title(title);
    let body = match &snap.raw {
        Some(raw) => lines(raw),
        None => {
            let mut v = vec![Line::raw("No raw frames yet: waiting for a reading from the PSU.")];
            if let Some(d) = &snap.diagnostic {
                v.push(Line::raw(format!("Last PSU status frame: {d}")));
            }
            v
        }
    };
    Paragraph::new(body).wrap(Wrap { trim: false }).block(block).render(area, buf);
}
