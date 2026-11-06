//! The full-screen TUI (SPEC §7): nvtop's feel, MeltAlarm's cable cards.

pub mod cable;
pub mod cutbar;
pub mod header;
pub mod help;
pub mod history;
pub mod raw;
pub mod termbg;
pub mod theme;

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use meltalarm_core::StatusKind;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::history::{History, WINDOWS};
use crate::monitor::{Link, Monitor, Snapshot};
use crate::output::CsvLog;
use crate::source::{Acquisition, FeedEvent, USB_IDS};
use theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    History,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Help,
}

/// Which arrangement fits the terminal (SPEC §7.5, §7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// ≥ 120 columns: full cards side by side.
    Wide { bar_rows: u16 },
    /// 60–119 columns and tall enough: full cards stacked.
    Stacked { bar_rows: u16 },
    /// Too short for full cards: compact cards side by side.
    Compact { bar_rows: u16 },
    /// Under 60 columns or 24 rows: one line per cable.
    OneLine,
}

/// Rows the chart keeps before cards may take them.
const CHART_MIN: u16 = 8;

pub fn mode(width: u16, height: u16, header: u16) -> Mode {
    if width < 60 || height < 24 {
        return Mode::OneLine;
    }
    // Rows left for cards and the chart, after the header, the strip and the function bar.
    let avail = height.saturating_sub(header + 2);
    let fit = |cards: u16, chart: u16, min: u16| -> Option<u16> {
        let per_card = avail.saturating_sub(chart) / cards;
        let rows = per_card.saturating_sub(cable::full_height(0, false)).min(cable::BAR_ROWS);
        (rows >= min).then_some(rows)
    };
    if width >= 120 {
        if let Some(bar_rows) = fit(1, CHART_MIN, cable::MIN_BAR_ROWS).or_else(|| fit(1, 0, cable::MIN_BAR_ROWS)) {
            return Mode::Wide { bar_rows };
        }
    } else if let Some(bar_rows) = fit(2, CHART_MIN, 6) {
        // Full cards stacked from 60 columns when the chart still gets its rows.
        return Mode::Stacked { bar_rows };
    }
    // Compact cards side by side, their bars as tall as the chart allows.
    Mode::Compact { bar_rows: avail.saturating_sub(CHART_MIN + 4).clamp(4, cable::BAR_ROWS) }
}

pub struct App {
    pub theme: Theme,
    pub pane: Pane,
    pub window: usize,
    pub zoom: Option<usize>,
    pub overlay: Option<Overlay>,
    pub paused: Option<Snapshot>,
    /// Where the chart stops while paused.
    pub paused_t: Option<f64>,
    pub interval: Duration,
    pub clock: String,
    pub log: Option<CsvLog>,
    pub log_note: Option<String>,
    pub bell: bool,
    pub quit: bool,
    flash_until: [Option<Instant>; 2],
    last_status: [Option<StatusKind>; 2],
    was_alarm: bool,
}

impl App {
    pub fn new(theme: Theme, interval: Duration) -> Self {
        App {
            theme,
            pane: Pane::History,
            window: 1,
            zoom: None,
            overlay: None,
            paused: None,
            paused_t: None,
            interval,
            clock: String::new(),
            log: None,
            log_note: None,
            bell: false,
            quit: false,
            flash_until: [None, None],
            last_status: [None, None],
            was_alarm: false,
        }
    }

    pub fn window(&self) -> Duration {
        WINDOWS[self.window % WINDOWS.len()]
    }

    fn flashing(&self, i: usize, now: Instant) -> bool {
        self.flash_until[i].is_some_and(|t| now < t)
    }

    /// Track state changes: one flash per change, the bell on entering an alarm, and an alarm
    /// always resumes a paused display.
    pub fn observe(&mut self, snap: &Snapshot, now: Instant) {
        for (i, c) in snap.cards.iter().enumerate() {
            if self.last_status[i].is_some_and(|s| s != c.status) {
                self.flash_until[i] = Some(now + Duration::from_millis(300));
            }
            self.last_status[i] = Some(c.status);
        }
        let alarm = snap.alarm_active();
        if alarm && !self.was_alarm {
            self.paused = None;
            self.paused_t = None;
            if self.bell {
                let mut out = std::io::stdout();
                let _ = out.write_all(b"\x07");
                let _ = out.flush();
            }
        }
        self.was_alarm = alarm;
    }

    pub fn next_flash_end(&self, now: Instant) -> Option<Instant> {
        self.flash_until.iter().flatten().copied().filter(|&t| t > now).min()
    }

    /// One key press.
    pub fn key(&mut self, code: KeyCode, mods: KeyModifiers, monitor: &mut Monitor, snap: &Snapshot) {
        let toggle = |cur: &mut Option<Overlay>, o: Overlay| *cur = if *cur == Some(o) { None } else { Some(o) };
        match code {
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::F(10) | KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.overlay.is_some() => self.overlay = None,
            KeyCode::Esc => self.quit = true,
            KeyCode::F(1) | KeyCode::Char('?') => toggle(&mut self.overlay, Overlay::Help),
            KeyCode::F(3) | KeyCode::Char('r') => self.pane = if self.pane == Pane::Raw { Pane::History } else { Pane::Raw },
            KeyCode::F(5) | KeyCode::Char('p') => {
                (self.paused, self.paused_t) =
                    if self.paused.is_some() { (None, None) } else { (Some(snap.clone()), monitor.history.latest_t()) };
            }
            KeyCode::F(6) | KeyCode::Right => self.window = (self.window + 1) % WINDOWS.len(),
            KeyCode::Left => self.window = (self.window + WINDOWS.len() - 1) % WINDOWS.len(),
            KeyCode::F(9) | KeyCode::Char('l') => self.toggle_log(),
            KeyCode::Char(d @ ('1' | '2')) => {
                let i = (d as u8 - b'1') as usize;
                self.zoom = if self.zoom == Some(i) { None } else { Some(i) };
            }
            KeyCode::Char('c') if self.zoom.is_some() => monitor.history.clear_peaks(),
            _ => {}
        }
    }

    fn toggle_log(&mut self) {
        if self.log.take().is_some() {
            self.log_note = None;
            return;
        }
        match CsvLog::open_daily() {
            Ok(log) => {
                self.log_note = Some(log.path().display().to_string());
                self.log = Some(log);
            }
            Err(e) => self.log_note = Some(format!("log failed: {e}")),
        }
    }
}

fn hz(interval: Duration) -> String {
    let hz = 1.0 / interval.as_secs_f64();
    if (hz - hz.round()).abs() < 0.01 { format!("{hz:.0} Hz") } else { format!("{hz:.1} Hz") }
}

fn fn_bar<'a>(app: &App, width: u16) -> Line<'a> {
    let full = [("F1", "Help"), ("F3", "Raw"), ("F5", "Pause"), ("F6", "Window"), ("F9", "Log"), ("F10", "Quit")];
    let short = [("F1", "Help"), ("F3", "Raw"), ("F5", "Pause"), ("F6", "Win"), ("F9", "Log"), ("F10", "Quit")];
    let len = |keys: &[(&str, &str)], gap: usize| 1 + keys.iter().map(|(k, l)| k.len() + l.len() + gap).sum::<usize>();
    let (keys, gap): (&[(&str, &str)], usize) = if len(&full, 2) <= width as usize {
        (&full, 2)
    } else if len(&short, 1) <= width as usize {
        (&short, 1)
    } else {
        (&[("F1", "Help"), ("F10", "Quit")], 1)
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(*k, app.theme.key()));
        spans.push(Span::raw(format!("{label}{}", " ".repeat(gap))));
    }
    let mut status = vec![hz(app.interval), format!("log {}", if app.log.is_some() { "on" } else { "off" })];
    if app.paused.is_some() {
        status.insert(0, "PAUSED".into());
    }
    if let Some(z) = app.zoom {
        status.insert(0, format!("zoom cable {}", z + 1));
    }
    let status = status.join(" · ");
    let used: usize = spans.iter().map(Span::width).sum();
    if used + status.len() < width as usize {
        spans.push(Span::raw(" ".repeat(width as usize - used - status.len() - 1)));
        spans.push(Span::styled(status, if app.paused.is_some() { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() }));
    }
    Line::from(spans)
}

fn set_line(buf: &mut Buffer, area: Rect, y: u16, line: &Line) {
    if y >= area.y && y < area.bottom() {
        buf.set_line(area.x, y, line, area.width);
    }
}

/// The PSU-not-found and permission screens (SPEC §9), before the first connection.
fn link_message(snap: &Snapshot) -> Option<(String, Vec<String>)> {
    if snap.model.is_some() {
        return None;
    }
    match &snap.link {
        Link::NotFound => Some((
            "PSU not found".into(),
            vec![
                format!("No MSI MPG Ai1600TS / Ai1300TS on USB ({USB_IDS})."),
                "Check the PSU's USB cable to a motherboard USB 2.0 header.".into(),
                "Looking again every 2 s.".into(),
            ],
        )),
        Link::Denied(msg) => {
            Some(("Permission denied".into(), vec![msg.clone(), crate::source::permission_hint(), "Retrying every 2 s.".into()]))
        }
        Link::Unusable(msg) => Some(("PSU unusable".into(), vec![msg.clone()])),
        _ => None,
    }
}

pub fn draw(f: &mut Frame, app: &App, live: &Snapshot, hist: &History, now: Instant) {
    let area = f.area();
    let buf = f.buffer_mut();
    let snap = app.paused.as_ref().unwrap_or(live);
    let t = &app.theme;
    let (w, h) = (area.width, area.height);
    if h < 4 || w < 20 {
        buf.set_stringn(area.x, area.y, "psutop: terminal too small", w as usize, Style::new());
        return;
    }
    let bottom = area.bottom() - 1;
    set_line(buf, area, bottom, &fn_bar(app, w));

    let mut header: Vec<Line> = vec![header::title(snap, t, &app.clock, w)];
    for (text, level) in &snap.banners {
        header.push(Line::styled(format!(" ⚠ {text}"), t.banner(*level)));
    }
    let extra: Vec<Line> = vec![header::psu(snap, t, w)];

    let n_header = (header.len() + extra.len()) as u16;
    let mode = mode(w, h, n_header);
    let mut y = area.y;
    for l in &header {
        set_line(buf, area, y, l);
        y += 1;
    }
    let strip_line = snap.strip.as_ref().map(|s| Line::styled(format!(" ⚠ {}", s.fit(w.saturating_sub(4) as usize)), t.strip(s.kind)));
    let message = link_message(snap);

    if mode == Mode::OneLine {
        match &message {
            Some((title, lines)) => {
                set_line(buf, area, y, &Line::styled(format!(" {title}: {}", lines[0]), Style::new().add_modifier(Modifier::BOLD)));
                y += 1;
            }
            None => {
                for (i, c) in snap.cards.iter().enumerate() {
                    set_line(buf, area, y, &cable::one_line(c, t, w, app.flashing(i, now)));
                    y += 1;
                }
            }
        }
        if let Some(l) = &strip_line {
            set_line(buf, area, y, l);
            y += 1;
        }
        if w >= 80 {
            for l in &extra {
                if y + 1 < bottom {
                    set_line(buf, area, y, l);
                    y += 1;
                }
            }
        }
    } else {
        for l in &extra {
            set_line(buf, area, y, l);
            y += 1;
        }
        let cards_top = y;
        let card_rows = match (app.zoom, mode) {
            (Some(_), _) => cable::full_height(cable::BAR_ROWS, true).min(bottom.saturating_sub(y + 1)),
            (None, Mode::Wide { bar_rows }) => cable::full_height(bar_rows, false),
            (None, Mode::Stacked { bar_rows }) => 2 * cable::full_height(bar_rows, false),
            (None, Mode::Compact { bar_rows }) => bar_rows + 4,
            _ => cable::COMPACT_HEIGHT,
        };
        let cards_area = Rect::new(area.x, cards_top, w, card_rows.min(bottom.saturating_sub(cards_top)));
        if let Some((title, lines)) = &message {
            let bw = 84.min(cards_area.width);
            let rows: usize = lines.iter().map(|l| (l.chars().count() + 1).div_ceil(bw.saturating_sub(2).max(1) as usize)).sum();
            let r = help::centered(cards_area, bw, rows as u16 + 2);
            let body: Vec<Line> = lines.iter().map(|l| Line::raw(format!(" {l}"))).collect();
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .block(
                    Block::new()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .title(format!(" {title} "))
                        .title_style(Style::new().add_modifier(Modifier::BOLD)),
                )
                .render(r, buf);
        } else {
            draw_cards(buf, app, snap, hist, mode, cards_area, now);
        }
        y = cards_area.bottom();
        if let Some(l) = &strip_line {
            set_line(buf, area, y, l);
        }
        y += 1;
    }

    if bottom > y + 2 {
        let pane = Rect::new(area.x, y, w, bottom - y);
        match app.pane {
            Pane::History => {
                // Each line is named by the GPU its cable feeds, as on the card ("GPU 0" unless
                // the config labels it).
                let name = |i: usize| snap.cards[i].gpu.clone().unwrap_or_else(|| format!("GPU {i}"));
                history::render(hist, app.window(), app.paused_t, &[name(0), name(1)], pane, buf, t)
            }
            Pane::Raw => raw::render(snap, pane, buf, t),
        }
    }
    if let Some(note) = &app.log_note
        && app.log.is_none()
    {
        let r = Rect::new(area.x, bottom.saturating_sub(1), w, 1);
        buf.set_stringn(r.x + 1, r.y, note, w as usize - 2, t.fg(theme::Role::Caution));
    }
    match app.overlay {
        Some(Overlay::Help) => help::help(area, buf, t),
        None => {}
    }
}

fn draw_cards(buf: &mut Buffer, app: &App, snap: &Snapshot, hist: &History, mode: Mode, area: Rect, now: Instant) {
    let t = &app.theme;
    if let Some(z) = app.zoom {
        let bar_rows = area.height.saturating_sub(cable::full_height(0, true)).min(cable::BAR_ROWS);
        let opts = cable::Opts { bar_rows, peaks: Some(&hist.peaks()[z]), flash: app.flashing(z, now) };
        cable::full(&snap.cards[z], area, buf, t, &opts);
        return;
    }
    let half = |i: u16| {
        let cw = (area.width.saturating_sub(1) / 2).min(72);
        Rect::new(area.x + i * (cw + 1), area.y, cw, area.height)
    };
    match mode {
        Mode::Wide { bar_rows } => {
            for i in 0..2u16 {
                let opts = cable::Opts { bar_rows, peaks: None, flash: app.flashing(i as usize, now) };
                cable::full(&snap.cards[i as usize], half(i), buf, t, &opts);
            }
        }
        Mode::Stacked { bar_rows } => {
            let ch = cable::full_height(bar_rows, false);
            let cw = area.width.min(cable::FULL_WIDTH + 13);
            for i in 0..2u16 {
                let r = Rect::new(area.x, area.y + i * ch, cw, ch.min(area.height.saturating_sub(i * ch)));
                let opts = cable::Opts { bar_rows, peaks: None, flash: app.flashing(i as usize, now) };
                cable::full(&snap.cards[i as usize], r, buf, t, &opts);
            }
        }
        _ => {
            for i in 0..2u16 {
                let r = half(i);
                let r = Rect::new(r.x, r.y, r.width.min(44), r.height);
                cable::compact(&snap.cards[i as usize], r, buf, t, app.flashing(i as usize, now));
            }
        }
    }
}

/// `HH:MM:SS` for the title line.
pub fn clock_now() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Run the TUI until the user quits or a termination signal arrives.
pub fn run(mut monitor: Monitor, acq: Acquisition, mut app: App, term: Arc<AtomicBool>) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, &mut monitor, &acq, &mut app, &term);
    ratatui::restore();
    res
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    monitor: &mut Monitor,
    acq: &Acquisition,
    app: &mut App,
    term: &AtomicBool,
) -> std::io::Result<()> {
    let mut dirty = true;
    let mut snap = monitor.snapshot(Instant::now());
    // Ask for the terminal's background before the first frame (real terminals answer in
    // milliseconds) and before anything reads input, so the reply is never taken for keys.
    if matches!(app.theme.depth, theme::Depth::True | theme::Depth::C256) {
        app.theme = app.theme.with_background(termbg::query(Duration::from_secs(1)));
    }
    while !app.quit && !term.load(Ordering::Relaxed) {
        let now = Instant::now();
        let mut new_sample = false;
        while let Ok(ev) = acq.rx.try_recv() {
            new_sample |= matches!(ev, FeedEvent::Sample(_));
            monitor.handle(ev, now);
            dirty = true;
        }
        if monitor.next_wake().is_some_and(|t| t <= now) {
            monitor.wake(now);
            dirty = true;
        }
        if app.next_flash_end(now).is_none() && app.flash_until.iter().flatten().any(|&t| t <= now) {
            app.flash_until = [None, None];
            dirty = true;
        }
        let clock = clock_now();
        if clock != app.clock {
            app.clock = clock;
            dirty = true;
        }
        if dirty {
            snap = monitor.snapshot(now);
            app.observe(&snap, now);
            let events = monitor.drain_log();
            if let Some(log) = app.log.as_mut() {
                let written =
                    events.iter().try_for_each(|e| log.event(e)).and_then(|()| if new_sample { log.sample(&snap) } else { Ok(()) });
                if let Err(e) = written {
                    app.log_note = Some(format!("log stopped: {e}"));
                    app.log = None;
                }
            }
            terminal.draw(|f| draw(f, app, &snap, &monitor.history, now))?;
            dirty = false;
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                TermEvent::Key(k) if k.kind == KeyEventKind::Press => {
                    app.key(k.code, k.modifiers, monitor, &snap);
                    dirty = true;
                }
                TermEvent::Resize(..) => dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}
