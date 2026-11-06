//! The simulator behind `--sim`: no device traffic at all.
//!
//! The wire values are MeltAlarm's `meltalarm-source-sim` arrays (MIT, © 2026 Denis
//! Domanskii), which match its screenshots. That crate cannot be used directly: it depends on
//! `meltalarm-source-api`, which has no HID backend on macOS. Unlike it, psutop simulates both
//! cables (the scenario's event is on cable 2, cable 1 runs the healthy `LOAD`, as in SPEC §7.5),
//! the inferred rail words and the raw frames.

use std::time::{Duration, Instant};

use meltalarm_model::*;

use crate::frame::{self, Rails, RawFrames, synth};
use crate::source::{Discovered, Feed, Sample};

pub const SCENARIOS: &[(&str, &str)] = &[
    ("cycle", "MeltAlarm's 150 s tour of every state on cable 2 (default)"),
    ("idle", "both cables idle; the live idle values from 2026-10-15"),
    ("normal", "both cables at MeltAlarm's healthy full load"),
    ("caution", "cable 2: one wire at 9.9 A, above the rating"),
    ("overload", "cable 2: one wire at 12.4 A, psutop's own alarm"),
    ("uneven", "cable 2: one wire at 4.4 A, the others ~8 A"),
    ("red", "cable 2: a bad contact, the PSU still says Normal"),
    ("alarm", "cable 2: the PSU's imbalance alarm and power-cut countdown"),
    ("critical", "cable 2: a wire at 18.6 A, the PSU's critical status"),
    ("fault", "a PSU fan fault after 5 s"),
    ("nodata", "the PSU stops answering after connecting"),
    ("silent", "the PSU is present but never answers"),
    ("slow", "the PSU answers after 10 s"),
    ("absent", "no PSU on USB"),
    ("denied", "the PSU is present but this user may not open it"),
];

const IDLE_1: [f32; 6] = [0.1875; 6];
const IDLE_2: [f32; 6] = [0.1875, 0.125, 0.125, 0.1875, 0.1875, 0.1875];
const LOAD: [f32; 6] = [8.1, 7.8, 7.7, 7.8, 8.0, 8.1];
const BAD: [f32; 6] = [9.9, 9.8, 2.1, 9.7, 9.8, 9.9];
const CRIT: [f32; 6] = [7.9, 18.6, 7.6, 7.8, 7.7, 7.9];
const HOT: [f32; 6] = [8.9, 9.1, 9.9, 9.0, 8.8, 9.0];
const OVER: [f32; 6] = [8.9, 9.1, 12.4, 9.0, 8.8, 9.0];
const UNEVEN: [f32; 6] = [8.2, 8.0, 4.4, 8.1, 8.3, 8.0];

pub fn is_scenario(name: &str) -> bool {
    SCENARIOS.iter().any(|(n, _)| *n == name)
}

pub struct SimFeed {
    scenario: String,
    first_seen: Option<Instant>,
    start: Option<Instant>,
}

/// One tick of the scenario: wires per cable, PSU status codes, flagged wires.
struct Tick {
    wires: [[f32; 6]; 2],
    status: [u8; 2],
    flagged: [[bool; 6]; 2],
    fail: bool,
    fan: bool,
}

impl SimFeed {
    pub fn new(scenario: &str) -> Self {
        SimFeed { scenario: scenario.to_owned(), first_seen: None, start: None }
    }

    pub fn discover_at(&mut self, now: Instant) -> Discovered {
        let first = *self.first_seen.get_or_insert(now);
        match self.scenario.as_str() {
            "absent" => return Discovered::Absent,
            "denied" => {
                return Discovered::Denied(
                    "PSU found but could not be opened (failed to open device with path /dev/hidraw11: Permission denied (os error 13))"
                        .into(),
                );
            }
            "silent" => return Discovered::NotReady("PSU found but not answering (Timeout)".into()),
            "slow" if now.duration_since(first) < Duration::from_secs(10) => {
                return Discovered::NotReady("PSU found but not answering (Timeout)".into());
            }
            _ => {}
        }
        self.start.get_or_insert(now);
        Discovered::Found(info())
    }

    fn tick(&self, t: u64) -> Tick {
        let flag = |i: usize| std::array::from_fn(|w| w == i);
        let none = [false; 6];
        let mut k = Tick { wires: [LOAD, LOAD], status: [0, 0], flagged: [none, none], fail: false, fan: false };
        match self.scenario.as_str() {
            "idle" => k.wires = [IDLE_1, IDLE_2],
            "caution" => k.wires[1] = HOT,
            "overload" => k.wires[1] = OVER,
            "uneven" => k.wires[1] = UNEVEN,
            "red" => k.wires[1] = BAD,
            "alarm" => (k.wires[1], k.status[1], k.flagged[1]) = (BAD, 2, flag(2)),
            "critical" => (k.wires[1], k.status[1], k.flagged[1]) = (CRIT, 3, flag(1)),
            "fault" => k.fan = t >= 5,
            "nodata" => k.fail = t >= 3,
            "normal" | "slow" => {}
            _ => {
                // MeltAlarm's cycle, on cable 2.
                let t = t % 150;
                k.wires[0] = if t < 8 { IDLE_1 } else { LOAD };
                (k.wires[1], k.status[1], k.fail) = match t {
                    0..8 => (IDLE_2, 0, false),
                    8..20 => (LOAD, 0, false),
                    20..34 => (HOT, 0, false),
                    34..44 => (OVER, 0, false),
                    44..56 => (LOAD, 0, false),
                    56..70 => (UNEVEN, 0, false),
                    70..90 => (LOAD, 0, false),
                    90..115 => (BAD, 2, false),
                    115..125 => (LOAD, 0, false),
                    125..129 => (LOAD, 0, true),
                    _ => (LOAD, 0, false),
                };
                if k.status[1] == 2 {
                    k.flagged[1] = flag(2);
                }
            }
        }
        k
    }

    pub fn poll_at(&mut self, at: Instant) -> Sample {
        let t = self.start.map_or(0, |s| at.duration_since(s).as_secs());
        let k = self.tick(t);
        if k.fail {
            return Sample::empty(at);
        }
        let cable_w = k.wires.map(|w| w.iter().sum::<f32>() * 12.0);
        let out_w = cable_w[0] + cable_w[1] + 62.0;
        let rails = Rails {
            amps: [Some(out_w * 0.96 / 12.06), Some(3.25), Some(1.25)],
            volts: [Some(12.06), Some(5.03), Some(3.33)],
            out_w: Some(out_w),
        };
        let raw = RawFrames {
            e0: Some(synth::e0(&synth::E0 {
                rail_amps: rails.amps.map(Option::unwrap_or_default),
                wires: k.wires,
                out_w,
                rail_volts: rails.volts.map(Option::unwrap_or_default),
            })),
            c1: Some(synth::c1(k.status)),
            e1: Some(synth::e1(k.flagged, k.fan)),
            c0: Some(synth::c0()),
            simulated: true,
        };
        // The report keeps MeltAlarm's published values; only the synthetic frame is quantized to
        // the PSU's 0.0625 A steps.
        let status_of = |s: u8| match s {
            1 => DeviceStatus::OverCurrent,
            2 => DeviceStatus::Imbalance,
            3 => DeviceStatus::CriticalOverCurrent,
            _ => DeviceStatus::Normal,
        };
        let report = Report {
            at,
            readings: Some((0..2).map(|i| ConnectorReading { index: i as u8, wires: k.wires[i].map(Some) }).collect()),
            verdicts: Some((0..2).map(|i| Verdict { index: i as u8, status: status_of(k.status[i]), flagged: k.flagged[i] }).collect()),
            faults: Some(if k.fan { vec![Fault::FanFailure] } else { vec![] }),
            protection: Some(protection()),
            diagnostic: (k.status != [0, 0]).then(|| format!("C1 {} (simulated)", frame::hex(raw.c1.as_deref().unwrap_or_default()))),
        };
        Sample { report, rails: Some(rails), raw: Some(raw) }
    }
}

fn info() -> SourceInfo {
    SourceInfo {
        id: SourceId("sim:ai1600ts".into()),
        vendor: "MSI".into(),
        model: "MPG Ai1600TS".into(),
        firmware: Some("sim".into()),
        serial: None,
        connectors: (0..2).map(|i| ConnectorInfo { index: i }).collect(),
        caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
        protection_name: Some("Safeguard+".into()),
    }
}

/// The reference unit's Safeguard+ settings (live 2026-10-15).
fn protection() -> Protection {
    Protection {
        enabled: true,
        wire_limit: Some(12.0),
        imbalance_limit: Some(5.5),
        wire_trigger: Some(Duration::from_secs(20)),
        imbalance_trigger: Some(Duration::from_secs(20)),
        cutoff_after: Some(Duration::from_secs(180)),
        hard_wire_limit: Some(18.0),
    }
}

impl Feed for SimFeed {
    fn discover(&mut self) -> Discovered {
        self.discover_at(Instant::now())
    }

    fn poll(&mut self) -> Sample {
        self.poll_at(Instant::now())
    }

    fn release(&mut self) {}

    fn device_node(&self) -> Option<String> {
        Some("(simulated)".into())
    }

    fn simulated(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenarios_put_the_event_on_cable_2() {
        let now = Instant::now();
        let mut f = SimFeed::new("caution");
        assert!(matches!(f.discover_at(now), Discovered::Found(_)));
        let s = f.poll_at(now);
        let r = s.report.readings.unwrap();
        assert_eq!(r[0].wires, LOAD.map(Some));
        assert_eq!(r[1].wires, HOT.map(Some));
    }

    #[test]
    fn slow_answers_after_ten_seconds_and_nodata_fails() {
        let t0 = Instant::now();
        let mut f = SimFeed::new("slow");
        assert!(matches!(f.discover_at(t0), Discovered::NotReady(_)));
        assert!(matches!(f.discover_at(t0 + Duration::from_secs(11)), Discovered::Found(_)));
        let mut f = SimFeed::new("nodata");
        f.discover_at(t0);
        assert!(f.poll_at(t0).report.readings.is_some());
        assert!(f.poll_at(t0 + Duration::from_secs(4)).report.readings.is_none());
    }

    #[test]
    fn synthetic_e0_carries_the_same_wires() {
        let now = Instant::now();
        let mut f = SimFeed::new("normal");
        f.discover_at(now);
        let s = f.poll_at(now);
        let e0 = s.raw.unwrap().e0.unwrap();
        let w3 = frame::word(&e0, 3).unwrap();
        assert!((w3 - 8.1).abs() <= 0.0625 / 2.0, "{w3}");
    }
}
