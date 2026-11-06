//! Raw MSI reply frames: the extra `E0` words MeltAlarm does not expose (SPEC §3.2, §5), and the
//! simulator's synthetic frames.
//!
//! The frame layout, the register numbers and the LINEAR11 rule follow `meltalarm-source-msi`
//! (MeltAlarm, MIT, © 2026 Denis Domanskii), which took them from MSI Center. The rail and
//! output-watt words are psutop's own inference from live frames.
//!
//! Frames here start at the opcode: `[51][reg][word0 lo][word0 hi]…`, as the hidraw reply arrives
//! without the report number. Decoding only: nothing in this module builds a request.

use meltalarm_model::WIRES;

/// `E0` words 0–2: 12V / 5V / 3.3V output amps. Inferred from live frames of an Ai1600TS.
pub const RAIL_AMPS: [usize; 3] = [0, 1, 2];
/// `E0` word 15: total output watts. Inferred.
pub const OUT_WATTS: usize = 15;
/// `E0` words 19–21: 12V / 5V / 3.3V volts. Inferred.
pub const RAIL_VOLTS: [usize; 3] = [19, 20, 21];
/// `E0` words 16 and 17: meaning unknown; shown in the raw pane only.
pub const UNKNOWN: [usize; 2] = [16, 17];
pub const RAIL_NAMES: [&str; 3] = ["12V", "5V", "3.3V"];

/// PMBus LINEAR11 as MSI decodes it: signed 5-bit exponent, unsigned 11-bit mantissa. The same
/// rule as `meltalarm_source_msi::protocol::linear11` (a test on Linux checks they agree).
pub fn linear11(lo: u8, hi: u8) -> f32 {
    let w = u16::from_le_bytes([lo, hi]);
    let exp = i32::from(w as i16 >> 11);
    f32::from(w & 0x07FF) * 2f32.powi(exp)
}

/// The inverse, for the simulator: `value` in steps of `2^exp`.
pub fn encode_linear11(value: f32, exp: i32) -> [u8; 2] {
    let mantissa = (value / 2f32.powi(exp)).round().clamp(0.0, 2047.0) as u16;
    let w = ((exp as i16 as u16) << 11) | mantissa;
    w.to_le_bytes()
}

/// Word `k` of a reply, or `None` if the reply is too short to hold it.
pub fn word(frame: &[u8], k: usize) -> Option<f32> {
    let at = 2 + 2 * k;
    Some(linear11(*frame.get(at)?, *frame.get(at + 1)?))
}

/// PSU-wide output data decoded from `E0`. Every field is inferred (SPEC §5).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rails {
    pub amps: [Option<f32>; 3],
    pub volts: [Option<f32>; 3],
    pub out_w: Option<f32>,
}

pub fn rails(e0: &[u8]) -> Rails {
    if e0.get(..2) != Some(&[0x51, 0xE0]) {
        return Rails::default();
    }
    Rails { amps: RAIL_AMPS.map(|k| word(e0, k)), volts: RAIL_VOLTS.map(|k| word(e0, k)), out_w: word(e0, OUT_WATTS) }
}

/// The last reply of each polled register, from the opcode on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawFrames {
    pub e0: Option<Vec<u8>>,
    pub c1: Option<Vec<u8>>,
    pub e1: Option<Vec<u8>>,
    pub c0: Option<Vec<u8>>,
    /// The frames were made up by the simulator.
    pub simulated: bool,
}

pub fn hex(frame: &[u8]) -> String {
    let end = frame.iter().rposition(|&b| b != 0).map_or(2, |i| i + 1).max(2);
    frame[..end.min(frame.len())].iter().map(|b| format!("{b:02X}")).collect()
}

/// One decoded line per word group, for the raw pane.
pub fn describe_e0(e0: &[u8]) -> Vec<String> {
    let w = |k| word(e0, k).map_or("—".into(), |v| format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_owned());
    let wires = |first: usize| (first..first + WIRES).map(w).collect::<Vec<_>>().join(" ");
    vec![
        format!("w0–2   rail A   12V {}  5V {}  3.3V {}   (inferred)", w(0), w(1), w(2)),
        format!("w3–8   cable 1  {} A", wires(3)),
        format!("w9–14  cable 2  {} A", wires(9)),
        format!("w15    output   {} W   (inferred)", w(OUT_WATTS)),
        format!("w16–17 unknown  {}  {}", w(UNKNOWN[0]), w(UNKNOWN[1])),
        format!("w18    unknown  {}", w(18)),
        format!("w19–21 rail V   12V {}  5V {}  3.3V {}   (inferred)", w(19), w(20), w(21)),
    ]
}

pub fn describe_c1(c1: &[u8]) -> String {
    match (c1.get(2), c1.get(23)) {
        (Some(a), Some(b)) => format!("status cable 1 = {a}, cable 2 = {b}   (0 normal, 1 over-current, 2 imbalance, 3 critical)"),
        _ => "short reply".into(),
    }
}

pub fn describe_e1(e1: &[u8]) -> String {
    let set: Vec<String> = (2..20).filter(|&i| e1.get(i).is_some_and(|&b| b != 0)).map(|i| format!("byte {i}")).collect();
    if set.is_empty() { "no protection flags set".into() } else { format!("flags set: {}", set.join(", ")) }
}

pub fn describe_c0(c0: &[u8]) -> String {
    let b = |i: usize| c0.get(i).copied();
    match (b(2), b(3), b(4), b(5), b(6), b(7), b(8), b(9)) {
        (Some(on), Some(w0), Some(w1), Some(i0), Some(i1), Some(wt), Some(it), Some(cut)) => format!(
            "Safeguard+ {} · wire {:.1} A for {wt} s · imbalance {:.1} A for {it} s · cut {cut} s",
            if on == 1 { "ON" } else { "OFF" },
            linear11(w0, w1),
            linear11(i0, i1)
        ),
        _ => "short reply".into(),
    }
}

/// Synthetic frames in the MSI layout, so the simulator exercises the raw pane and rail decoding.
pub mod synth {
    use super::encode_linear11;
    use meltalarm_model::WIRES;

    pub struct E0 {
        pub rail_amps: [f32; 3],
        pub wires: [[f32; WIRES]; 2],
        pub out_w: f32,
        pub rail_volts: [f32; 3],
    }

    pub fn e0(v: &E0) -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[0] = 0x51;
        f[1] = 0xE0;
        let mut put = |k: usize, value: f32, exp: i32| f[2 + 2 * k..4 + 2 * k].copy_from_slice(&encode_linear11(value, exp));
        for (k, a) in v.rail_amps.iter().enumerate() {
            put(k, *a, -4);
        }
        for (c, wires) in v.wires.iter().enumerate() {
            for (i, a) in wires.iter().enumerate() {
                put(3 + 6 * c + i, *a, -4);
            }
        }
        put(15, v.out_w, 1);
        put(16, 77.5, -3);
        put(17, 61.0, 0);
        for (k, volts) in v.rail_volts.iter().enumerate() {
            put(19 + k, *volts, if k == 0 { -6 } else { -7 });
        }
        f
    }

    pub fn c1(status: [u8; 2]) -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[..2].copy_from_slice(&[0x51, 0xC1]);
        f[2] = status[0];
        f[23] = status[1];
        f
    }

    pub fn e1(wires: [[bool; WIRES]; 2], fan: bool) -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[..2].copy_from_slice(&[0x51, 0xE1]);
        for (c, w) in wires.iter().enumerate() {
            for (i, on) in w.iter().enumerate() {
                f[5 + 6 * c + i] = u8::from(*on);
            }
        }
        f[19] = u8::from(fan);
        f
    }

    pub fn c0() -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[..10].copy_from_slice(&[0x51, 0xC0, 0x01, 0xC0, 0xE0, 0x58, 0xE0, 0x14, 0x14, 0xB4]);
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        let mut v: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        v.resize(64, 0);
        v
    }

    /// MeltAlarm's reference `E0` reply from an idle Ai1300TS (its `protocol::tests::E0_IDLE`).
    const E0_IDLE_AI1300TS: &str = "51E016F034E014E002E002E002E002E002E002E000E000E000E000E000E000E02B086CEA1E00001006D385CAAAC9";

    #[test]
    fn rails_decode_from_the_reference_frame() {
        let r = rails(&bytes(E0_IDLE_AI1300TS));
        assert_eq!(r.amps, [Some(5.5), Some(3.25), Some(1.25)]);
        assert_eq!(r.out_w, Some(86.0));
        let v = r.volts.map(Option::unwrap);
        assert!((v[0] - 12.094).abs() < 0.001 && (v[1] - 5.039).abs() < 0.001 && (v[2] - 3.328).abs() < 0.001, "{v:?}");
        // The inference behind these words: amps × volts ≈ word 15.
        let sum: f32 = (0..3).map(|i| r.amps[i].unwrap() * v[i]).sum();
        assert!((sum - 86.0).abs() / 86.0 < 0.025, "{sum}");
    }

    #[test]
    fn other_registers_and_short_frames_have_no_rails() {
        assert_eq!(rails(&bytes("51C1")), Rails::default());
        let short = rails(&bytes("51E016F0")[..4]);
        assert_eq!(short.amps[0], Some(5.5));
        assert_eq!(short.out_w, None);
    }

    #[test]
    fn linear11_matches_msi_examples_and_round_trips() {
        assert_eq!(linear11(0xC0, 0xE0), 12.0);
        assert_eq!(linear11(0x58, 0xE0), 5.5);
        assert_eq!(linear11(0x0A, 0x08), 20.0);
        for (v, exp) in [(9.9375, -4), (0.1875, -4), (12.06, -6), (1084.0, 1)] {
            let [lo, hi] = encode_linear11(v, exp);
            assert!((linear11(lo, hi) - v).abs() <= 2f32.powi(exp) / 2.0, "{v}");
        }
    }

    #[test]
    fn synthetic_frames_decode_like_real_ones() {
        let e0 = synth::e0(&synth::E0 {
            rail_amps: [90.0, 3.25, 1.25],
            wires: [[8.125; 6], [9.9375, 9.0, 8.875, 9.0, 9.0, 9.0]],
            out_w: 1084.0,
            rail_volts: [12.06, 5.03, 3.33],
        });
        assert_eq!(word(&e0, 5), Some(8.125));
        assert_eq!(word(&e0, 9), Some(9.9375));
        assert_eq!(rails(&e0).out_w, Some(1084.0));
        assert_eq!(describe_c0(&synth::c0()), "Safeguard+ ON · wire 12.0 A for 20 s · imbalance 5.5 A for 20 s · cut 180 s");
        assert!(describe_c1(&synth::c1([0, 2])).contains("cable 2 = 2"));
        assert_eq!(hex(&synth::c0()), "51C001C0E058E01414B4");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linear11_agrees_with_meltalarm() {
        for w in [0u16, 0x0001, 0x07FF, 0x082B, 0xE002, 0xF016, 0xD306, 0xC9AA, 0xFFFF] {
            let [lo, hi] = w.to_le_bytes();
            assert_eq!(linear11(lo, hi), meltalarm_source_msi::protocol::linear11(lo, hi));
        }
    }
}
