//! Feeds MeltAlarm's `Core` and keeps what psutop shows next to it; [`Monitor::snapshot`] is
//! everything the TUI and the text/JSON/CSV outputs render. Clock-injected like `Core`.

use std::time::{Duration, Instant};

use meltalarm_core::{
    AlarmKind, ConnectorView, Core, Event, Level, Limits, LogEvent, NoteKind, Settings, StatusKind, UserAction, ViewModel, WireView,
};
use meltalarm_model::{ConnectorKey, Protection, SourceInfo, WIRES};

use crate::config::Config;
use crate::frame::{Rails, RawFrames};
use crate::history::{History, Point};
use crate::source::{Discovered, FeedEvent, Sample};

pub const VOLTS: f32 = 12.0;

pub struct Monitor {
    core: Core,
    limits: Limits,
    start: Instant,
    config: Config,
    simulated: bool,
    info: Option<SourceInfo>,
    protection: Option<Protection>,
    searching: Option<Discovered>,
    rails: Option<Rails>,
    raw: Option<RawFrames>,
    diagnostic: Option<String>,
    pub history: History,
    log: Vec<LogEvent>,
    next_wake: Option<Instant>,
    samples: u64,
}

/// Where the PSU link stands, for the full-screen messages (SPEC §9).
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Starting,
    Connected,
    NotFound,
    Denied(String),
    NotAnswering(String),
    Unusable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub index: u8,
    /// Always numbered: "GPU power cable 1".
    pub label: String,
    /// "Cable 1", where space is tight.
    pub short: String,
    pub status: StatusKind,
    /// `OK`, `Caution`, `ALARM`, `No data`, `Not connected`.
    pub status_text: String,
    pub wires: [WireView; WIRES],
    pub imbalance: Option<f32>,
    pub imbalance_level: Level,
    pub psu_status: String,
    pub psu_level: Level,
    /// The full card's live line, from core's texts.
    pub reason: Option<(String, Level)>,
    /// The compact layouts' one line (core's `summary`).
    pub summary: (String, Level),
    pub limit: f32,
    pub rating: f32,
    /// What the cable powers, from the config: "GPU 0" / "GPU 1" unless relabelled.
    pub gpu: Option<String>,
    pub watts: Option<f32>,
}

impl Card {
    pub fn in_alarm(&self) -> bool {
        self.status == StatusKind::Alarm
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripKind {
    Caution,
    Alarm,
    Cleared,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub kind: StripKind,
    pub text: String,
    /// For narrow terminals: the cable, the number that matters, the action.
    pub short: String,
}

impl Strip {
    /// `text` if it fits in `width` columns, else `short`.
    pub fn fit(&self, width: usize) -> &str {
        if self.text.chars().count() <= width { &self.text } else { &self.short }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub vm: ViewModel,
    pub link: Link,
    pub simulated: bool,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub protection: Option<Protection>,
    pub protection_name: String,
    pub cards: [Card; 2],
    pub strip: Option<Strip>,
    /// PSU-wide problems: faults, Safeguard+ off.
    pub banners: Vec<(String, Level)>,
    pub rails: Option<Rails>,
    pub raw: Option<RawFrames>,
    /// The PSU's raw status during an alarm (MSI: the `C1` frame in hex).
    pub diagnostic: Option<String>,
    pub limits: Limits,
    pub samples: u64,
}

impl Snapshot {
    pub fn alarm_active(&self) -> bool {
        self.vm.alarm.as_ref().is_some_and(|a| !a.green)
    }

    pub fn worst(&self) -> StatusKind {
        let rank = |s: StatusKind| match s {
            StatusKind::Alarm => 4,
            StatusKind::Caution => 3,
            StatusKind::NoData => 2,
            StatusKind::NotConnected => 1,
            StatusKind::Normal => 0,
        };
        self.cards.iter().map(|c| c.status).max_by_key(|s| rank(*s)).unwrap_or(StatusKind::NoData)
    }
}

pub fn cable_watts(wires: &[Option<f32>; WIRES]) -> Option<f32> {
    wires.iter().all(Option::is_some).then(|| wires.iter().flatten().sum::<f32>() * VOLTS)
}

impl Monitor {
    pub fn new(config: Config, interval: Duration, simulated: bool, wall: fn() -> String, now: Instant) -> Self {
        let settings = Settings { limits: config.limits.overrides(), run_at_startup: false, ..Settings::default() };
        let limits = settings.limits.resolve().limits;
        let mut core = Core::new(settings);
        core.set_wall_clock(wall);
        Monitor {
            core,
            limits,
            start: now,
            config,
            simulated,
            info: None,
            protection: None,
            searching: None,
            rails: None,
            raw: None,
            diagnostic: None,
            history: History::new(interval),
            log: Vec::new(),
            next_wake: None,
            samples: 0,
        }
    }

    pub fn limits_warning(&self) -> Option<String> {
        self.config.limits.overrides().resolve().warning
    }

    fn core(&mut self, ev: Event, now: Instant) {
        let out = self.core.handle(ev, now);
        self.log.extend(out.log);
        self.next_wake = out.next_wake;
    }

    pub fn handle(&mut self, ev: FeedEvent, now: Instant) {
        match ev {
            FeedEvent::Searching(d) => {
                if let Discovered::NotReady(reason) = &d {
                    self.core(Event::SourcePending(reason.clone()), now);
                }
                self.searching = Some(d);
            }
            FeedEvent::Connected(info) => {
                self.searching = None;
                self.core(Event::SourceConnected(info.clone()), now);
                // Both cables are always tracked, so core numbers them ("GPU power cable 1").
                for c in &info.connectors {
                    let key = ConnectorKey { source: info.id.clone(), index: c.index };
                    self.core(Event::User(UserAction::SetTracked(key, true)), now);
                }
                self.info = Some(info);
            }
            FeedEvent::Sample(s) => self.on_sample(*s, now),
            FeedEvent::Lost => self.core(Event::SourceLost, now),
        }
    }

    fn on_sample(&mut self, s: Sample, now: Instant) {
        self.samples += 1;
        if let Some(p) = &s.report.protection {
            self.protection = Some(p.clone());
        }
        let readings = s.report.readings.clone();
        self.diagnostic = s.report.diagnostic.clone();
        if s.report.readings.is_some() {
            self.rails = s.rails;
            self.raw = s.raw.clone();
        } else {
            self.rails = None;
        }
        self.core(Event::Report(s.report), now);

        let mut point = Point { t: now.duration_since(self.start).as_secs_f64(), cable_w: [None, None], max_wire: [None, None] };
        for r in readings.iter().flatten() {
            let c = usize::from(r.index);
            if c < 2 {
                point.cable_w[c] = cable_watts(&r.wires);
                point.max_wire[c] = r.wires.iter().flatten().copied().reduce(f32::max);
                self.history.note_wires(c, &r.wires);
            }
        }
        self.history.push(point);
    }

    /// A deadline from [`Monitor::next_wake`] passed, or the clock ticked.
    pub fn wake(&mut self, now: Instant) {
        self.core(Event::Wake, now);
    }

    pub fn next_wake(&self) -> Option<Instant> {
        self.next_wake
    }

    pub fn drain_log(&mut self) -> Vec<LogEvent> {
        std::mem::take(&mut self.log)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    pub fn source_info(&self) -> Option<&SourceInfo> {
        self.info.as_ref()
    }

    fn link(&self) -> Link {
        if self.info.is_some() && self.searching.is_none() {
            return Link::Connected;
        }
        match &self.searching {
            None => Link::Starting,
            Some(Discovered::Found(_)) => Link::Connected,
            Some(Discovered::Absent) => Link::NotFound,
            Some(Discovered::Denied(m)) => Link::Denied(m.clone()),
            Some(Discovered::NotReady(m)) => Link::NotAnswering(m.clone()),
            Some(Discovered::Unusable(m)) => Link::Unusable(m.clone()),
        }
    }

    pub fn snapshot(&self, now: Instant) -> Snapshot {
        let vm = self.core.view(now);
        let link = self.link();
        let cards: [Card; 2] = std::array::from_fn(|i| {
            let i = i as u8;
            let mut card = match vm.connectors.iter().find(|c| c.key.index == i) {
                Some(cv) => card_from(cv),
                None => placeholder(i, &link, &self.limits),
            };
            card.gpu = Some(self.config.label(i));
            card.watts = cable_watts(&card.wires.map(|w| w.amps));
            card
        });
        let strip = strip(&vm, &cards);
        let mut banners = Vec::new();
        if let Some(src) = &vm.source {
            for f in &src.faults {
                banners.push((format!("PSU fault: {f}"), Level::Warning));
            }
        }
        let protection_name = self.info.as_ref().and_then(|i| i.protection_name.clone()).unwrap_or_else(|| "Protection".into());
        if self.protection.as_ref().is_some_and(|p| !p.enabled) {
            banners.push((
                format!("PSU {protection_name} is OFF: the PSU won't cut power. psutop still raises its own alarms."),
                Level::Caution,
            ));
        }
        if let Some(w) = self.limits_warning() {
            banners.push((format!("Config: {w}"), Level::Caution));
        }
        Snapshot {
            link,
            simulated: self.simulated,
            model: self.info.as_ref().map(|i| format!("{} {}", i.vendor, i.model)),
            firmware: self.info.as_ref().and_then(|i| i.firmware.clone()),
            protection: self.protection.clone(),
            protection_name,
            cards,
            strip,
            banners,
            rails: self.rails,
            raw: self.raw.clone(),
            diagnostic: self.diagnostic.clone(),
            limits: self.limits,
            samples: self.samples,
            vm,
        }
    }
}

fn card_from(cv: &ConnectorView) -> Card {
    let reason = if cv.status_kind == StatusKind::Alarm {
        cv.alarm_reason.as_ref().map(|r| {
            let text = match cv.countdown.as_deref() {
                Some("any second") => format!("{r} · POWER CUT ANY SECOND"),
                Some(cd) => format!("{r} · power cut in {cd}"),
                None => r.clone(),
            };
            (text, Level::Warning)
        })
    } else if cv.summary_level > Level::Normal || cv.status_kind == StatusKind::NoData {
        Some((cv.summary.clone(), cv.summary_level))
    } else {
        None
    };
    Card {
        index: cv.key.index,
        label: cv.full_label.clone(),
        short: format!("Cable {}", cv.key.index + 1),
        status: cv.status_kind,
        status_text: cv.status_text.clone(),
        wires: cv.wires,
        imbalance: cv.imbalance,
        imbalance_level: cv.imbalance_level,
        psu_status: cv.psu_status.clone(),
        psu_level: cv.psu_level,
        reason,
        summary: (cv.summary.clone(), cv.summary_level),
        limit: cv.bar_limit.unwrap_or(Limits::V1.alarm),
        rating: cv.bar_rating.unwrap_or(Limits::V1.rating),
        gpu: None,
        watts: None,
    }
}

/// A card before the PSU has connected: hollow bars and `No data`.
fn placeholder(index: u8, link: &Link, limits: &Limits) -> Card {
    let line = match link {
        Link::NotFound => "PSU not found",
        Link::Denied(_) => "Permission denied",
        Link::NotAnswering(_) => "PSU not answering",
        Link::Unusable(_) => "PSU unusable",
        Link::Starting | Link::Connected => "Reading the PSU…",
    };
    Card {
        index,
        label: format!("GPU power cable {}", index + 1),
        short: format!("Cable {}", index + 1),
        status: StatusKind::NoData,
        status_text: "No data".into(),
        wires: [WireView::default(); WIRES],
        imbalance: None,
        imbalance_level: Level::Normal,
        psu_status: "—".into(),
        psu_level: Level::Normal,
        reason: Some((line.into(), Level::Normal)),
        summary: (line.into(), Level::Normal),
        limit: limits.alarm,
        rating: limits.rating,
        gpu: None,
        watts: None,
    }
}

/// "GPU power cables 1 + 2" / "GPU power cable 2" → "Cables 1 + 2" / "Cable 2".
fn short_place(connector: &str) -> String {
    connector.replace("GPU power cables", "Cables").replace("GPU power cable", "Cable")
}

/// "Cable 2" (core's short place) → "GPU power cable 2".
fn place_label(place: &str) -> String {
    match place.strip_prefix("Cable ") {
        Some(n) => format!("GPU power cable {n}"),
        None => place.to_owned(),
    }
}

/// The one-line strip under the cards: core's alarm, then core's caution strip, then a caution
/// that is still live after core's 10 s strip has gone.
fn strip(vm: &ViewModel, cards: &[Card; 2]) -> Option<Strip> {
    if let Some(a) = &vm.alarm {
        if a.green {
            let text = format!("{} · {} · {}", a.headline, a.connector, a.what);
            return Some(Strip { kind: StripKind::Cleared, short: format!("{} · back to normal", short_place(&a.connector)), text });
        }
        let mut parts = vec![a.headline.clone(), a.connector.clone()];
        if !a.what.is_empty() {
            parts.push(a.what.clone());
        }
        if !a.right_value.is_empty() {
            parts.push(format!("{} {}", a.right_label, a.right_value));
        }
        parts.push(a.action.clone());
        if a.kind == AlarmKind::DataLost {
            parts.push("PSU data lost".into());
        }
        let short = format!("{} · {} {} · {}", short_place(&a.connector), a.right_label, a.right_value, a.action);
        return Some(Strip { kind: StripKind::Alarm, text: parts.join(" · "), short });
    }
    if let Some(c) = &vm.caution {
        let mut parts: Vec<String> = c.place.iter().map(|p| place_label(p)).collect();
        parts.push(c.what.clone());
        let short = format!("{}{}", c.place.as_ref().map_or(String::new(), |p| format!("{p} · ")), c.what);
        parts.push(c.action.clone());
        return Some(Strip { kind: StripKind::Caution, text: parts.join(" · "), short });
    }
    for card in cards.iter().filter(|c| c.status == StatusKind::Caution) {
        let note = vm
            .connectors
            .iter()
            .find(|c| c.key.index == card.index)
            .and_then(|c| c.notes.iter().find(|n| n.kind == NoteKind::Caution && !n.text.starts_with("PSU ")));
        if let Some(n) = note {
            let what = n.text.trim_end_matches('.');
            return Some(Strip {
                kind: StripKind::Caution,
                text: format!("{} · {what}", card.label),
                short: format!("{} · {what}", card.short),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimFeed;

    pub fn wall() -> String {
        "2026-10-15 14:02:11".into()
    }

    fn run(scenario: &str, ticks: u64) -> (Monitor, Instant) {
        let t0 = Instant::now();
        let mut m = Monitor::new(Config::default(), Duration::from_secs(1), true, wall, t0);
        let mut f = SimFeed::new(scenario);
        let mut now = t0;
        if let Discovered::Found(info) = f.discover_at(t0) {
            m.handle(FeedEvent::Connected(info), t0);
            for i in 0..ticks {
                now = t0 + Duration::from_secs(i);
                m.handle(FeedEvent::Sample(Box::new(f.poll_at(now))), now);
            }
        }
        (m, now)
    }

    #[test]
    fn normal_matches_the_flyout_screenshot() {
        let (m, now) = run("normal", 2);
        let s = m.snapshot(now);
        let c = &s.cards[0];
        assert_eq!((c.label.as_str(), c.status_text.as_str(), c.psu_status.as_str()), ("GPU power cable 1", "OK", "Normal"));
        assert_eq!(c.wires.map(|w| w.amps.unwrap()), [8.1, 7.8, 7.7, 7.8, 8.0, 8.1]);
        assert_eq!(c.imbalance.map(|i| (i * 10.0).round() / 10.0), Some(0.4));
        assert_eq!(c.gpu.as_deref(), Some("GPU 0"));
        assert_eq!(s.cards[1].gpu.as_deref(), Some("GPU 1"));
        assert!(s.strip.is_none());
    }

    #[test]
    fn caution_has_the_reason_line_and_the_strip() {
        let (m, now) = run("caution", 12);
        let s = m.snapshot(now);
        let c = &s.cards[1];
        assert_eq!(c.status_text, "Caution");
        assert_eq!(c.reason, Some(("9.9 A · over rating".into(), Level::Caution)));
        assert_eq!(c.wires.iter().filter(|w| w.level == Level::Caution).count(), 1);
        let strip = s.strip.unwrap();
        assert_eq!(strip.kind, StripKind::Caution);
        assert_eq!(strip.text, "GPU power cable 2 · A wire at 9.9 A, above the 9.5 A rating · Ease the GPU load");
    }

    #[test]
    fn the_strip_stays_while_the_caution_is_live() {
        let (m, now) = run("caution", 40);
        let strip = m.snapshot(now).strip.unwrap();
        assert_eq!(strip.text, "GPU power cable 2 · A wire at 9.9 A, above the 9.5 A rating");
    }

    #[test]
    fn psu_alarm_counts_down() {
        let (m, now) = run("alarm", 3);
        let s = m.snapshot(now);
        assert_eq!(s.cards[1].status_text, "ALARM");
        let (reason, level) = s.cards[1].reason.clone().unwrap();
        assert_eq!(level, Level::Warning);
        assert_eq!(reason, "PSU: Current imbalance · power cut in ~2:58");
        assert!(s.alarm_active());
        assert_eq!(s.strip.unwrap().kind, StripKind::Alarm);
    }

    #[test]
    fn critical_says_any_second() {
        let (m, now) = run("critical", 1);
        let reason = m.snapshot(now).cards[1].reason.clone().unwrap().0;
        assert!(reason.ends_with("POWER CUT ANY SECOND"), "{reason}");
    }

    #[test]
    fn before_connecting_cards_are_placeholders() {
        let t0 = Instant::now();
        let mut m = Monitor::new(Config::default(), Duration::from_secs(1), true, wall, t0);
        assert_eq!(m.snapshot(t0).cards[0].status_text, "No data");
        m.handle(FeedEvent::Searching(Discovered::Absent), t0);
        let s = m.snapshot(t0);
        assert_eq!(s.link, Link::NotFound);
        assert_eq!(s.cards[1].summary.0, "PSU not found");
    }

    #[test]
    fn idle_reads_ok_with_watts() {
        let (m, now) = run("idle", 2);
        let s = m.snapshot(now);
        assert_eq!(s.cards[0].status_text, "OK");
        assert_eq!(s.cards[0].watts.map(f32::round), Some(14.0));
        assert_eq!(s.cards[1].watts.map(f32::round), Some(12.0));
    }

    #[test]
    fn safeguard_off_raises_a_banner() {
        let t0 = Instant::now();
        let mut m = Monitor::new(Config::default(), Duration::from_secs(1), true, wall, t0);
        let mut f = SimFeed::new("normal");
        let Discovered::Found(info) = f.discover_at(t0) else { panic!() };
        m.handle(FeedEvent::Connected(info), t0);
        let mut s = f.poll_at(t0);
        s.report.protection.as_mut().unwrap().enabled = false;
        m.handle(FeedEvent::Sample(Box::new(s)), t0);
        let banners = m.snapshot(t0).banners;
        assert!(banners.iter().any(|(t, l)| t.starts_with("PSU Safeguard+ is OFF") && *l == Level::Caution), "{banners:?}");
    }

    #[test]
    fn a_fan_fault_raises_a_banner() {
        let (m, now) = run("fault", 7);
        let s = m.snapshot(now);
        assert!(s.banners.iter().any(|(t, l)| t == "PSU fault: Fan failure" && *l == Level::Warning), "{:?}", s.banners);
    }
}
