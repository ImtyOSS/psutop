//! Renders the simulator's `cycle` scenario to an animated SVG for the README:
//!
//!     cargo run --example demo_svg -- docs/demo.svg
//!
//! Frames come from the real TUI drawing code on ratatui's TestBackend. Block elements and box
//! lines are drawn as shapes, not font glyphs, so bars stay solid and borders join in any
//! browser. Development only; nothing here is part of the psutop binary.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use psutop::config::Config;
use psutop::monitor::Monitor;
use psutop::sim::SimFeed;
use psutop::source::{Discovered, FeedEvent};
use psutop::ui::theme::{Depth, Theme};
use psutop::ui::{self, App};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};

const W: u16 = 120;
const H: u16 = 38;
const CW: f32 = 8.4;
const CH: f32 = 17.0;
const HOLD_S: f32 = 2.2;
const TERM_BG: &str = "#161616";
const TERM_FG: &str = "#d4d4d4";
/// Seconds into the 150 s cycle: idle, load, caution, overload, uneven, PSU alarm, cleared, OK.
const MOMENTS: [u64; 8] = [7, 18, 31, 38, 68, 100, 117, 145];

fn wall() -> String {
    "2026-10-15 14:02:11".into()
}

fn hex(c: Color, default: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => default.to_owned(),
        Color::White => "#ffffff".into(),
        Color::Black => "#000000".into(),
        Color::Red | Color::LightRed => "#ff5f5f".into(),
        Color::Green | Color::LightGreen => "#87d787".into(),
        Color::Yellow | Color::LightYellow => "#ffd75f".into(),
        Color::Cyan | Color::LightCyan => "#87d7d7".into(),
        Color::Magenta | Color::LightMagenta => "#d787d7".into(),
        Color::Gray => "#a8a8a8".into(),
        Color::DarkGray => "#585858".into(),
        _ => default.to_owned(),
    }
}

/// Foreground and background of a cell, with reverse video applied.
fn colours(cell: &Cell) -> (String, String) {
    let (fg, bg) = (hex(cell.fg, TERM_FG), hex(cell.bg, TERM_BG));
    if cell.modifier.contains(Modifier::REVERSED) { (bg, fg) } else { (fg, bg) }
}

/// A box-drawing character as a path inside the cell at (x, y), or `None`.
fn box_path(ch: &str, x: f32, y: f32) -> Option<(String, bool)> {
    let (x0, x1, y0, y1) = (x, x + CW, y, y + CH);
    let (cx, cy) = (x + CW / 2.0, y + CH / 2.0);
    let r = CW / 2.0;
    let p = match ch {
        "─" | "╌" => format!("M{x0} {cy}H{x1}"),
        "│" | "╎" => format!("M{cx} {y0}V{y1}"),
        "┌" => format!("M{x1} {cy}H{cx}V{y1}"),
        "┐" => format!("M{x0} {cy}H{cx}V{y1}"),
        "└" => format!("M{cx} {y0}V{cy}H{x1}"),
        "┘" => format!("M{cx} {y0}V{cy}H{x0}"),
        "┬" => format!("M{x0} {cy}H{x1}M{cx} {cy}V{y1}"),
        "╭" => format!("M{x1} {cy}H{}Q{cx} {cy} {cx} {}V{y1}", cx + r, cy + r),
        "╮" => format!("M{x0} {cy}H{}Q{cx} {cy} {cx} {}V{y1}", cx - r, cy + r),
        "╰" => format!("M{cx} {y0}V{}Q{cx} {cy} {} {cy}H{x1}", cy - r, cx + r),
        "╯" => format!("M{cx} {y0}V{}Q{cx} {cy} {} {cy}H{x0}", cy - r, cx - r),
        _ => return None,
    };
    Some((p, matches!(ch, "╌" | "╎")))
}

/// Eighths of a cell filled from the bottom (`▁`…`█`), or the upper half (`▀`).
fn block(ch: &str) -> Option<(f32, bool)> {
    let lower = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    if let Some(i) = lower.iter().position(|b| *b == ch) {
        return Some(((i + 1) as f32 / 8.0, false));
    }
    (ch == "▀").then_some((0.5, true))
}

/// Rounds float noise (`1007.99994`, four or more decimals) to one decimal. Values shown on
/// screen never have that many decimals, so only coordinates change.
fn round_noise(svg: &str) -> String {
    let b = svg.as_bytes();
    let mut out = String::with_capacity(svg.len());
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_digit() || b[i - 1] == b'.')) {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                let dot = i;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i - dot > 4
                    && let Ok(v) = svg[start..i].parse::<f64>()
                {
                    let r = format!("{v:.1}");
                    out.push_str(r.strip_suffix(".0").unwrap_or(&r));
                    continue;
                }
            }
            out.push_str(&svg[start..i]);
            continue;
        }
        let ch = svg[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn frame_svg(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let py = f32::from(y) * CH;
        // Backgrounds, merged into runs.
        let mut x = 0;
        while x < buf.area.width {
            let bg = colours(&buf[(x, y)]).1;
            let start = x;
            while x < buf.area.width && colours(&buf[(x, y)]).1 == bg {
                x += 1;
            }
            if bg != TERM_BG {
                let _ = write!(
                    out,
                    r#"<rect x="{}" y="{py}" width="{}" height="{CH}" fill="{bg}"/>"#,
                    f32::from(start) * CW,
                    f32::from(x - start) * CW + 0.5
                );
            }
        }
        // Blocks and box lines as shapes; everything else as text runs.
        let mut run = String::new();
        let mut run_start = 0u16;
        let mut run_style = (String::new(), false);
        let flush = |out: &mut String, run: &mut String, start: u16, style: &(String, bool)| {
            if run.trim().is_empty() {
                run.clear();
                return;
            }
            let len = run.chars().count() as f32;
            let weight = if style.1 { r#" font-weight="bold""# } else { "" };
            let _ = write!(
                out,
                r#"<text x="{}" y="{}" fill="{}" textLength="{}" lengthAdjust="spacingAndGlyphs"{weight}>{}</text>"#,
                f32::from(start) * CW,
                py + CH * 0.75,
                style.0,
                len * CW,
                esc(run)
            );
            run.clear();
        };
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            let sym = cell.symbol();
            let (fg, _) = colours(cell);
            let px = f32::from(x) * CW;
            if let Some((share, upper)) = block(sym) {
                flush(&mut out, &mut run, run_start, &run_style);
                let h = CH * share;
                let top = if upper { py } else { py + CH - h };
                let _ = write!(out, r#"<rect x="{px}" y="{top}" width="{}" height="{h}" fill="{fg}"/>"#, CW + 0.3);
                continue;
            }
            if let Some((d, dashed)) = box_path(sym, px, py) {
                flush(&mut out, &mut run, run_start, &run_style);
                let dash = if dashed { r#" stroke-dasharray="3 2""# } else { "" };
                let _ = write!(out, r#"<path d="{d}" stroke="{fg}" stroke-width="1.3" fill="none"{dash}/>"#);
                continue;
            }
            let style = (fg, cell.modifier.contains(Modifier::BOLD));
            if run.is_empty() {
                run_start = x;
                run_style = style;
            } else if style != run_style {
                flush(&mut out, &mut run, run_start, &run_style);
                run_start = x;
                run_style = style;
            }
            run.push_str(if sym.is_empty() { " " } else { sym });
        }
        flush(&mut out, &mut run, run_start, &run_style);
    }
    out
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "docs/demo.svg".into());
    // DEMO_STILL=<i>: only frame i, no animation (to check one frame in a static viewer).
    let still: Option<usize> = std::env::var("DEMO_STILL").ok().and_then(|v| v.parse().ok());
    let t0 = Instant::now();
    let mut m = Monitor::new(Config::default(), Duration::from_secs(1), true, wall, t0);
    let mut feed = SimFeed::new("cycle");
    let Discovered::Found(info) = feed.discover_at(t0) else { panic!("the simulator always connects") };
    m.handle(FeedEvent::Connected(info), t0);
    let mut app = App::new(Theme::new(Depth::True, false).with_background(Some((0x16, 0x16, 0x16))), Duration::from_secs(1));
    app.window = 0;
    let mut frames = Vec::new();
    for t in 0..=*MOMENTS.last().unwrap() {
        let now = t0 + Duration::from_secs(t);
        m.handle(FeedEvent::Sample(Box::new(feed.poll_at(now))), now);
        m.wake(now);
        if MOMENTS.contains(&t) {
            let s = 11 + t;
            app.clock = format!("14:{:02}:{:02}", 2 + s / 60, s % 60);
            let snap = m.snapshot(now);
            app.observe(&snap, now);
            let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
            // Draw a moment later, after the one-off 300 ms flash a state change gets.
            let shown = now + Duration::from_secs(1);
            term.draw(|f| ui::draw(f, &app, &snap, &m.history, shown)).unwrap();
            frames.push(frame_svg(term.backend().buffer()));
        }
    }
    let (w, h) = (f32::from(W) * CW, f32::from(H) * CH);
    let total = HOLD_S * frames.len() as f32;
    let share = 100.0 / frames.len() as f32;
    // One shared timeline; each frame is visible for its slice. step-end holds each keyframe's
    // value (visibility would otherwise interpolate to visible early). Without animation, the
    // caution frame shows as a still.
    let mut css = format!(
        "text{{white-space:pre}}.f{{visibility:hidden;animation-duration:{total}s;animation-iteration-count:infinite;animation-timing-function:step-end}}#still{{visibility:visible}}"
    );
    for i in 0..frames.len() {
        let (a, b) = (share * i as f32, share * (i + 1) as f32);
        let lead = if i == 0 { String::new() } else { "0%{visibility:hidden}".into() };
        let _ = write!(
            css,
            "@keyframes f{i}{{{lead}{a:.3}%{{visibility:visible}}{b:.3}%{{visibility:hidden}}100%{{visibility:hidden}}}}#f{i}{{animation-name:f{i}}}"
        );
    }
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xml:space="preserve" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', 'Liberation Mono', monospace" font-size="14">
<title>psutop running the simulator: idle, load, caution, overload, uneven load, a PSU alarm, back to normal</title>
<style>{css}</style>
<rect width="100%" height="100%" rx="8" fill="{TERM_BG}"/>
"#
    );
    if let Some(k) = still {
        frames = vec![frames.remove(k)];
    }
    for (i, f) in frames.iter().enumerate() {
        let id = if i == 2 || still.is_some() { "still".to_owned() } else { format!("f{i}") };
        let anim = if i == 2 { format!(r#" style="animation-name:f{i}""#) } else { String::new() };
        let _ = writeln!(svg, r#"<g class="f" id="{id}"{anim}>{f}</g>"#);
    }
    svg.push_str("</svg>\n");
    std::fs::write(&path, round_noise(&svg)).expect("write the SVG");
    println!("{path}: {} frames, {} KB", frames.len(), std::fs::metadata(&path).map(|m| m.len() / 1024).unwrap_or(0));
}
