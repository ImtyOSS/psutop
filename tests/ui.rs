//! Layout × scenario snapshots on ratatui's TestBackend (SPEC §14), plus colour checks.

use std::time::{Duration, Instant};

use psutop::config::Config;
use psutop::monitor::Monitor;
use psutop::sim::SimFeed;
use psutop::source::{Discovered, FeedEvent};
use psutop::ui::theme::{Depth, Role, Theme};
use psutop::ui::{self, App, Overlay, Pane};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

fn wall() -> String {
    "2026-10-15 14:02:11".into()
}

/// Run `scenario` for `ticks` one-second samples on a fixed clock.
fn monitor(scenario: &str, ticks: u64) -> (Monitor, Instant) {
    let t0 = Instant::now();
    let mut m = Monitor::new(Config::default(), Duration::from_secs(1), true, wall, t0);
    let mut f = SimFeed::new(scenario);
    let mut now = t0;
    match f.discover_at(t0) {
        Discovered::Found(info) => {
            m.handle(FeedEvent::Connected(info), t0);
            for i in 0..ticks {
                now = t0 + Duration::from_secs(i);
                m.handle(FeedEvent::Sample(Box::new(f.poll_at(now))), now);
            }
        }
        other => m.handle(FeedEvent::Searching(other), t0),
    }
    (m, now)
}

fn render(scenario: &str, ticks: u64, w: u16, h: u16, depth: Depth, setup: impl FnOnce(&mut App)) -> Buffer {
    let (m, now) = monitor(scenario, ticks);
    let mut app = App::new(Theme::new(depth, false), Duration::from_secs(1));
    app.clock = "14:02:11".into();
    setup(&mut app);
    let snap = m.snapshot(now);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, &app, &snap, &m.history, now)).unwrap();
    term.backend().buffer().clone()
}

fn text(buf: &Buffer) -> String {
    let a = buf.area;
    (a.y..a.bottom())
        .map(|y| (a.x..a.right()).map(|x| buf[(x, y)].symbol().to_owned()).collect::<String>().trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    let a = buf.area;
    for y in a.y..a.bottom() {
        let row: Vec<String> = (a.x..a.right()).map(|x| buf[(x, y)].symbol().to_owned()).collect();
        let line = row.concat();
        if let Some(byte) = line.find(needle) {
            let col = line[..byte].chars().count() as u16;
            return Some((a.x + col, y));
        }
    }
    None
}

const LAYOUTS: [(&str, u16, u16); 4] = [("wide", 120, 40), ("stacked", 100, 52), ("compact", 70, 30), ("oneline", 50, 20)];
const SCENARIOS: [(&str, u64); 10] = [
    ("normal", 3),
    ("caution", 12),
    ("idle", 3),
    ("overload", 6),
    ("uneven", 12),
    ("alarm", 3),
    ("critical", 2),
    ("fault", 7),
    ("nodata", 8),
    ("absent", 0),
];

#[test]
fn layouts_by_scenario() {
    for (layout, w, h) in LAYOUTS {
        for (scenario, ticks) in SCENARIOS {
            let buf = render(scenario, ticks, w, h, Depth::True, |_| {});
            insta::assert_snapshot!(format!("{layout}_{scenario}"), text(&buf));
        }
    }
}

/// The alarm logic is copied from MeltAlarm, a Windows app; none of its texts may name it on
/// psutop's screen. Every scenario, including the cycle's data loss at 125–129 s.
#[test]
fn no_screen_names_meltalarm() {
    for (scenario, _) in psutop::sim::SCENARIOS {
        for ticks in [0, 3, 12, 40, 100, 127, 140] {
            let screen = text(&render(scenario, ticks, 120, 40, Depth::True, |_| {}));
            assert!(!screen.contains("MeltAlarm"), "{scenario} after {ticks} s:\n{screen}");
        }
    }
}

#[test]
fn zoom_raw_help_and_mono() {
    let buf = render("caution", 12, 120, 40, Depth::True, |a| a.zoom = Some(1));
    insta::assert_snapshot!("wide_caution_zoom2", text(&buf));
    let buf = render("normal", 3, 120, 40, Depth::True, |a| a.pane = Pane::Raw);
    insta::assert_snapshot!("wide_normal_raw", text(&buf));
    let buf = render("normal", 3, 120, 40, Depth::True, |a| a.overlay = Some(Overlay::Help));
    insta::assert_snapshot!("wide_normal_help", text(&buf));
    let buf = render("caution", 12, 120, 40, Depth::Mono, |_| {});
    insta::assert_snapshot!("wide_caution_mono", text(&buf));
}

#[test]
fn caution_colours_only_the_hot_wire() {
    let theme = Theme::new(Depth::True, false);
    let buf = render("caution", 12, 120, 40, Depth::True, |_| {});
    let amber = theme.color(Role::Caution);
    let neutral = theme.color(Role::Neutral);
    // The values row of card 2: "8.9 9.1 9.9 9.0 8.8 9.0", only 9.9 in amber.
    let (x, y) = find(&buf, "9.9").expect("9.9 on screen");
    assert_eq!(buf[(x, y)].fg, amber);
    let (x8, y8) = find(&buf, "8.9").unwrap();
    assert_eq!(y8, y);
    assert_eq!(buf[(x8, y8)].fg, theme.color(Role::Text));
    // The hot bar's fill in the head (above the cut) is amber; its neighbour's body is neutral.
    let bar_top = (y - 11..y).find(|&yy| buf[(x, yy)].fg == amber && buf[(x, yy)].symbol() != " ");
    assert!(bar_top.is_some(), "amber fill above the cut");
    assert!((y - 11..y).any(|yy| buf[(x8, yy)].fg == neutral), "neutral fill on a normal wire");
    // The pill and the reason line.
    let (px, py) = find(&buf, "● Caution").unwrap();
    assert_eq!(buf[(px, py)].fg, amber);
    let (rx, ry) = find(&buf, "9.9 A · over rating").unwrap();
    assert_eq!(buf[(rx, ry)].fg, amber);
}

#[test]
fn ok_cards_use_no_state_colour() {
    let theme = Theme::new(Depth::True, false);
    let buf = render("normal", 3, 120, 40, Depth::True, |_| {});
    let (x0, y0) = find(&buf, "╭ GPU power cable 1").unwrap();
    for y in y0..y0 + 20 {
        for x in x0..x0 + 59 {
            let fg = buf[(x, y)].fg;
            assert!(fg != theme.color(Role::Caution) && fg != theme.color(Role::WarnGfx), "state colour at {x},{y}");
        }
    }
}

#[test]
fn the_ok_pill_has_a_green_dot_and_caution_an_amber_one() {
    let theme = Theme::new(Depth::True, false);
    let buf = render("caution", 12, 120, 40, Depth::True, |_| {});
    let (x, y) = find(&buf, "● OK").unwrap();
    assert_eq!(buf[(x, y)].fg, theme.color(Role::OkDot), "OK dot");
    assert_eq!(buf[(x + 2, y)].fg, theme.color(Role::Neutral), "OK text stays neutral");
    assert_eq!(buf[(x, y)].bg, theme.color(Role::PillOk));
    let (cx, cy) = find(&buf, "● Caution").unwrap();
    assert_eq!(buf[(cx, cy)].fg, theme.color(Role::Caution));
}

#[test]
fn mono_caution_is_readable_without_colour() {
    let buf = render("caution", 12, 120, 40, Depth::Mono, |_| {});
    let t = text(&buf);
    assert!(t.contains("( ● Caution )"), "{t}");
    assert!(t.contains("▓▓▓"), "{t}");
    assert!(t.contains("9.9 A · over rating"));
}

#[test]
fn layout_modes_follow_the_width() {
    use psutop::ui::Mode;
    assert!(matches!(ui::mode(120, 40, 4), Mode::Wide { bar_rows: 11 }));
    assert_eq!(ui::mode(90, 52, 4), Mode::Stacked { bar_rows: 10 });
    assert!(matches!(ui::mode(90, 30, 4), Mode::Compact { .. }));
    assert_eq!(ui::mode(70, 30, 4), Mode::Compact { bar_rows: 11 });
    assert_eq!(ui::mode(75, 46, 4), Mode::Stacked { bar_rows: 7 });
    assert_eq!(ui::mode(72, 43, 4), Mode::Compact { bar_rows: 11 });
    assert_eq!(ui::mode(50, 30, 4), Mode::OneLine);
    assert_eq!(ui::mode(120, 23, 4), Mode::OneLine);
}

#[test]
#[ignore = "prints frames for a manual look: cargo test --test ui show -- --ignored --nocapture"]
fn show() {
    let scenario = std::env::var("SCENARIO").unwrap_or_else(|_| "caution".into());
    let w: u16 = std::env::var("W").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let h: u16 = std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let ticks: u64 = std::env::var("TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let buf = render(&scenario, ticks, w, h, Depth::True, |a| a.window = 0);
    println!("{}", text(&buf));
}
