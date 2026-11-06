//! `once` / `watch` output (SPEC §6, §7.7): card-style lines on a TTY, plain `key=value` lines
//! when piped, JSON and CSV with stable fields. `null` (JSON) or an empty field (CSV) means
//! "not read"; never 0.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use meltalarm_core::LogEvent;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Serialize;

use crate::frame;
use crate::monitor::{Card, Link, Snapshot, StripKind};
use crate::ui::cable;
use crate::ui::header::protection_text;
use crate::ui::theme::{Depth, Theme};

/// Four decimals: the PSU's 0.0625 A steps survive, float noise does not.
fn r4(v: Option<f32>) -> Option<f64> {
    v.map(|v| (f64::from(v) * 1e4).round() / 1e4)
}

fn num(v: Option<f32>) -> String {
    r4(v).map_or(String::new(), |v| v.to_string())
}

pub fn link_name(link: &Link) -> &'static str {
    match link {
        Link::Starting => "starting",
        Link::Connected => "connected",
        Link::NotFound => "not_found",
        Link::Denied(_) => "permission_denied",
        Link::NotAnswering(_) => "not_answering",
        Link::Unusable(_) => "unusable",
    }
}

// ---------------------------------------------------------------- JSON

#[derive(Serialize)]
pub struct JsonSample {
    pub time: String,
    pub simulated: bool,
    pub link: &'static str,
    pub psu_model: Option<String>,
    pub firmware: Option<String>,
    pub protection: Option<JsonProtection>,
    pub cables: Vec<JsonCable>,
    pub psu: JsonPsu,
    /// The strip under the cards: alarm, caution or cleared, in words.
    pub alert: Option<JsonAlert>,
    /// `--raw`: the PSU's raw reply frames in hex, kept inside the JSON so it stays valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<JsonRaw>,
}

#[derive(Serialize)]
pub struct JsonRaw {
    pub e0: Option<String>,
    pub c1: Option<String>,
    pub e1: Option<String>,
    pub c0: Option<String>,
    pub simulated: bool,
}

#[derive(Serialize)]
pub struct JsonProtection {
    pub enabled: bool,
    pub wire_limit_a: Option<f64>,
    pub imbalance_limit_a: Option<f64>,
    pub wire_trigger_s: Option<u64>,
    pub imbalance_trigger_s: Option<u64>,
    pub cutoff_after_s: Option<u64>,
}

#[derive(Serialize)]
pub struct JsonCable {
    pub cable: u8,
    pub label: String,
    pub gpu: Option<String>,
    pub state: String,
    pub wires_a: Vec<Option<f64>>,
    pub imbalance_a: Option<f64>,
    pub watts: Option<f64>,
    pub psu_status: Option<String>,
    pub flagged: Vec<bool>,
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct JsonPsu {
    /// Inferred `E0` words (SPEC §5); `null` until this source can read them.
    pub out_w: Option<f64>,
    pub rails: Vec<JsonRail>,
    pub inferred: bool,
    pub faults: Vec<String>,
}

#[derive(Serialize)]
pub struct JsonRail {
    pub rail: &'static str,
    pub amps: Option<f64>,
    pub volts: Option<f64>,
}

#[derive(Serialize)]
pub struct JsonAlert {
    pub kind: &'static str,
    pub text: String,
}

fn alert_kind(k: StripKind) -> &'static str {
    match k {
        StripKind::Caution => "caution",
        StripKind::Alarm => "alarm",
        StripKind::Cleared => "cleared",
    }
}

fn state_word(c: &Card) -> String {
    c.status_text.clone()
}

/// `raw`: include the raw frames (`--raw`).
pub fn json(snap: &Snapshot, time: &str, raw: bool) -> JsonSample {
    let rails = snap.rails.unwrap_or_default();
    JsonSample {
        time: time.to_owned(),
        simulated: snap.simulated,
        link: link_name(&snap.link),
        psu_model: snap.model.clone(),
        firmware: snap.firmware.clone(),
        protection: snap.protection.as_ref().map(|p| JsonProtection {
            enabled: p.enabled,
            wire_limit_a: r4(p.wire_limit),
            imbalance_limit_a: r4(p.imbalance_limit),
            wire_trigger_s: p.wire_trigger.map(|d| d.as_secs()),
            imbalance_trigger_s: p.imbalance_trigger.map(|d| d.as_secs()),
            cutoff_after_s: p.cutoff_after.map(|d| d.as_secs()),
        }),
        cables: snap
            .cards
            .iter()
            .map(|c| JsonCable {
                cable: c.index + 1,
                label: c.label.clone(),
                gpu: c.gpu.clone(),
                state: state_word(c),
                wires_a: c.wires.iter().map(|w| r4(w.amps)).collect(),
                imbalance_a: r4(c.imbalance),
                watts: r4(c.watts).map(f64::round),
                psu_status: (c.psu_status != "—").then(|| c.psu_status.clone()),
                flagged: c.wires.iter().map(|w| w.flagged).collect(),
                reason: c.reason.as_ref().map(|(t, _)| t.clone()),
            })
            .collect(),
        psu: JsonPsu {
            out_w: r4(rails.out_w),
            rails: (0..3).map(|i| JsonRail { rail: frame::RAIL_NAMES[i], amps: r4(rails.amps[i]), volts: r4(rails.volts[i]) }).collect(),
            inferred: true,
            faults: snap.vm.source.as_ref().map(|s| s.faults.clone()).unwrap_or_default(),
        },
        alert: snap.strip.as_ref().map(|s| JsonAlert { kind: alert_kind(s.kind), text: s.text.clone() }),
        raw: raw.then(|| {
            let r = snap.raw.as_ref();
            let hex = |f: Option<&Vec<u8>>| f.map(|f| frame::hex(f));
            JsonRaw {
                e0: hex(r.and_then(|r| r.e0.as_ref())),
                c1: hex(r.and_then(|r| r.c1.as_ref())),
                e1: hex(r.and_then(|r| r.e1.as_ref())),
                c0: hex(r.and_then(|r| r.c0.as_ref())),
                simulated: r.is_some_and(|r| r.simulated),
            }
        }),
    }
}

// ---------------------------------------------------------------- CSV

pub fn csv_header() -> String {
    let mut cols = vec!["time".to_owned(), "kind".to_owned()];
    for c in 1..=2 {
        cols.push(format!("c{c}_state"));
        cols.extend((1..=6).map(|w| format!("c{c}_w{w}_a")));
        cols.extend([format!("c{c}_imbalance_a"), format!("c{c}_watts"), format!("c{c}_psu_status")]);
    }
    cols.extend(["psu_out_w", "event"].map(String::from));
    cols.join(",")
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
}

pub fn csv_sample(snap: &Snapshot, time: &str) -> String {
    let mut f = vec![time.to_owned(), "sample".to_owned()];
    for c in &snap.cards {
        f.push(csv_field(&c.status_text));
        f.extend(c.wires.iter().map(|w| num(w.amps)));
        f.push(num(c.imbalance));
        f.push(c.watts.map_or(String::new(), |w| format!("{w:.0}")));
        f.push(csv_field(if c.psu_status == "—" { "" } else { &c.psu_status }));
    }
    f.push(snap.rails.and_then(|r| r.out_w).map_or(String::new(), |w| format!("{w:.0}")));
    f.push(String::new());
    f.join(",")
}

pub fn csv_event(e: &LogEvent, time: &str) -> String {
    let empty = csv_header().split(',').count() - 3;
    let line = e.format(time);
    format!("{time},event{}{}", ",".repeat(empty + 1), csv_field(line.split_once(" | ").map_or(&line, |(_, rest)| rest)))
}

/// `F9`: `~/.local/state/psutop/psutop-YYYYMMDD.csv`, a new file each day.
pub struct CsvLog {
    path: PathBuf,
    file: File,
    day: String,
}

fn wall() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

impl CsvLog {
    pub fn open_daily() -> io::Result<CsvLog> {
        let day = chrono::Local::now().format("%Y%m%d").to_string();
        let dir = crate::config::state_dir();
        std::fs::create_dir_all(&dir)?;
        CsvLog::open(&dir.join(format!("psutop-{day}.csv")), day)
    }

    fn open(path: &Path, day: String) -> io::Result<CsvLog> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        if file.metadata()?.len() == 0 {
            writeln!(file, "{}", csv_header())?;
        }
        Ok(CsvLog { path: path.to_owned(), file, day })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn roll(&mut self) -> io::Result<()> {
        let day = chrono::Local::now().format("%Y%m%d").to_string();
        if day != self.day {
            *self = CsvLog::open(&self.path.with_file_name(format!("psutop-{day}.csv")), day)?;
        }
        Ok(())
    }

    pub fn sample(&mut self, snap: &Snapshot) -> io::Result<()> {
        self.roll()?;
        writeln!(self.file, "{}", csv_sample(snap, &wall()))
    }

    pub fn event(&mut self, e: &LogEvent) -> io::Result<()> {
        self.roll()?;
        writeln!(self.file, "{}", csv_event(e, &wall()))
    }
}

// ---------------------------------------------------------------- text

/// Plain, stable `key=value` lines for pipes and files: no colour, no glyphs.
pub fn plain(snap: &Snapshot, time: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in &snap.cards {
        let wires: Vec<String> = c.wires.iter().map(|w| r4(w.amps).map_or("-".into(), |v| v.to_string())).collect();
        out.push(format!(
            "{time} cable={} state={} wires_a={} imbalance_a={} watts={} psu_status={} gpu={} reason={}",
            c.index + 1,
            c.status_text.replace(' ', "_"),
            wires.join(","),
            r4(c.imbalance).map_or("-".into(), |v| v.to_string()),
            c.watts.map_or("-".into(), |w| format!("{w:.0}")),
            if c.psu_status == "—" { "-".into() } else { c.psu_status.replace(' ', "_") },
            c.gpu.as_deref().map_or("-".into(), |g| format!("{g:?}")),
            c.reason.as_ref().map_or("-".into(), |(t, _)| format!("{t:?}")),
        ));
    }
    if let Some(s) = &snap.strip {
        out.push(format!("{time} alert={} text={:?}", alert_kind(s.kind), s.text));
    }
    out
}

fn sgr(style: Style, depth: Depth) -> String {
    if depth == Depth::Mono {
        let mut codes = Vec::new();
        if style.add_modifier.contains(Modifier::BOLD) {
            codes.push("1".to_owned());
        }
        if style.add_modifier.contains(Modifier::REVERSED) {
            codes.push("7".to_owned());
        }
        return if codes.is_empty() { String::new() } else { format!("\x1b[{}m", codes.join(";")) };
    }
    let color = |c: Color, bg: bool| -> Option<String> {
        let base = if bg { 40 } else { 30 };
        Some(match c {
            Color::Reset => return None,
            Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
            Color::Indexed(i) => format!("{};5;{i}", base + 8),
            Color::Black => format!("{base}"),
            Color::Red => format!("{}", base + 1),
            Color::Green => format!("{}", base + 2),
            Color::Yellow => format!("{}", base + 3),
            Color::Blue => format!("{}", base + 4),
            Color::Magenta => format!("{}", base + 5),
            Color::Cyan => format!("{}", base + 6),
            Color::Gray => format!("{}", base + 7),
            Color::DarkGray => format!("{}", base + 60),
            Color::LightRed => format!("{}", base + 61),
            Color::LightGreen => format!("{}", base + 62),
            Color::LightYellow => format!("{}", base + 63),
            Color::LightBlue => format!("{}", base + 64),
            Color::LightMagenta => format!("{}", base + 65),
            Color::LightCyan => format!("{}", base + 66),
            Color::White => format!("{}", base + 67),
        })
    };
    let mut codes: Vec<String> = Vec::new();
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if style.add_modifier.contains(Modifier::REVERSED) {
        codes.push("7".into());
    }
    codes.extend(style.fg.and_then(|c| color(c, false)));
    codes.extend(style.bg.and_then(|c| color(c, true)));
    if codes.is_empty() { String::new() } else { format!("\x1b[{}m", codes.join(";")) }
}

/// A ratatui line as ANSI text, for output outside the TUI.
pub fn ansi(line: &Line, depth: Depth) -> String {
    line.spans
        .iter()
        .map(|s: &Span| {
            let style = line.style.patch(s.style);
            let open = sgr(style, depth);
            if open.is_empty() { s.content.to_string() } else { format!("{open}{}\x1b[0m", s.content) }
        })
        .collect()
}

/// The card-style block `watch` prints on a TTY (SPEC §7.7).
pub fn card_block(snap: &Snapshot, theme: &Theme, time: &str) -> Vec<String> {
    let dim = theme.dim();
    let mut head = vec![Span::styled(time.to_owned(), Style::new().add_modifier(Modifier::BOLD))];
    if let Some(m) = &snap.model {
        head.push(Span::styled(format!("  {m} · {}", protection_text(snap.protection.as_ref(), &snap.protection_name)), dim));
    }
    if snap.simulated {
        head.push(Span::styled("  SIMULATED", dim));
    }
    let mut lines = vec![ansi(&Line::from(head), theme.depth)];
    for c in &snap.cards {
        let mut spans = vec![Span::styled(format!("  {:<18}", c.label), Style::new().add_modifier(Modifier::BOLD))];
        let pill = cable::pill(c, theme, false);
        let pill_w: usize = pill.iter().map(Span::width).sum();
        spans.extend(pill);
        spans.push(Span::raw(" ".repeat(13usize.saturating_sub(pill_w))));
        spans.extend(c.wires.iter().map(|w| {
            let ch = crate::ui::cutbar::mini(w.amps, c.limit).to_string();
            if theme.mono() {
                let rev = w.level == meltalarm_core::Level::Warning;
                Span::styled(ch, if rev { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() })
            } else {
                Span::styled(ch, theme.fg(theme.level_gfx(w.level)))
            }
        }));
        spans.push(Span::raw(" "));
        for w in &c.wires {
            let v = w.amps.map_or("—".into(), |a| format!("{a:.1}"));
            spans.push(Span::styled(format!("{v:>5}"), theme.fg(theme.level_text(w.level))));
        }
        spans.push(Span::styled("   Imbalance ", dim));
        spans.push(Span::styled(
            c.imbalance.map_or("—".into(), |i| format!("{i:.1} A")),
            theme.fg(theme.level_text(c.imbalance_level)).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("   PSU status ", dim));
        spans.push(Span::styled(c.psu_status.clone(), theme.fg(theme.level_text(c.psu_level)).add_modifier(Modifier::BOLD)));
        if let Some(g) = &c.gpu {
            spans.push(Span::styled(format!("   {g}"), dim));
        }
        lines.push(ansi(&Line::from(spans), theme.depth));
        if let Some((text, level)) = &c.reason {
            let style = if *level == meltalarm_core::Level::Normal { dim } else { theme.fg(theme.level_text(*level)) };
            lines.push(ansi(&Line::from(vec![Span::raw("    "), Span::styled(text.clone(), style)]), theme.depth));
        }
    }
    if let Some(s) = &snap.strip {
        lines.push(ansi(&Line::styled(format!("  ⚠ {}", s.text), theme.strip(s.kind)), theme.depth));
    }
    lines
}

pub fn raw_lines(snap: &Snapshot, time: &str) -> Vec<String> {
    match &snap.raw {
        Some(r) => {
            let tag = if r.simulated { " (simulated)" } else { "" };
            [("E0", &r.e0), ("C1", &r.c1), ("E1", &r.e1), ("C0", &r.c0)]
                .iter()
                .filter_map(|(n, f)| f.as_ref().map(|f| format!("{time}  raw {n} {}{tag}", frame::hex(f))))
                .collect()
        }
        None => vec![format!("{time}  raw frames unavailable this tick")],
    }
}
