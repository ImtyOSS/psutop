//! The nvtop-style lines above the cards (SPEC §7.5): the title and the PSU line.

use std::time::Duration;

use meltalarm_model::Protection;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme::{Role, Theme};
use crate::monitor::Snapshot;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `[label ||||||       value]`, `width` columns in all, like nvtop's meters.
pub fn meter<'a>(label: &str, frac: Option<f32>, value: &str, width: usize, fill: Style) -> Vec<Span<'a>> {
    let head = format!("{label} [");
    let inside = width.saturating_sub(head.chars().count() + 1).max(value.chars().count() + 1);
    let room = inside.saturating_sub(value.chars().count() + 1);
    let bars = frac.map_or(0, |f| ((f.clamp(0.0, 1.0) * room as f32).round()) as usize);
    vec![
        Span::raw(head),
        Span::styled("|".repeat(bars), fill),
        Span::raw(format!("{}{value}]", " ".repeat(inside - bars - value.chars().count()))),
    ]
}

fn secs(d: Option<Duration>) -> String {
    d.map_or("?".into(), |d| format!("{} s", d.as_secs()))
}

/// `Safeguard+ ON (12.0 A / 5.5 A, 20 s → cut 180 s)`.
pub fn protection_text(p: Option<&Protection>, name: &str) -> String {
    match p {
        None => format!("{name} …"),
        Some(p) if !p.enabled => format!("{name} OFF"),
        Some(p) => format!(
            "{name} ON ({} / {}, {} → cut {})",
            p.wire_limit.map_or("?".into(), |a| format!("{a:.1} A")),
            p.imbalance_limit.map_or("?".into(), |a| format!("{a:.1} A")),
            secs(p.wire_trigger),
            secs(p.cutoff_after)
        ),
    }
}

/// The title line, dropping detail until it fits: firmware and limits, then the model.
pub fn title<'a>(snap: &Snapshot, theme: &Theme, clock: &str, width: u16) -> Line<'a> {
    let off = snap.protection.as_ref().is_some_and(|p| !p.enabled);
    let long = protection_text(snap.protection.as_ref(), &snap.protection_name);
    let short = format!("{} {}", snap.protection_name, if off { "OFF" } else { "ON" });
    let variants: Vec<Vec<Span<'a>>> = match &snap.model {
        Some(model) => {
            let fw = snap.firmware.as_ref().map(|f| format!(" · fw {f}")).unwrap_or_default();
            let prot = |p: String| Span::styled(p, if off { theme.fg(Role::Caution) } else { Style::new() });
            vec![
                vec![Span::raw(format!("{model}{fw} · ")), prot(long)],
                vec![Span::raw(format!("{model} · ")), prot(short.clone())],
                vec![prot(short)],
                vec![],
            ]
        }
        None => vec![vec![Span::styled("looking for the PSU…", theme.dim())], vec![]],
    };
    let name = format!(" psutop {}", VERSION.strip_suffix(".0").unwrap_or(VERSION));
    let sim = if snap.simulated { [" SIMULATED", " SIM"] } else { ["", ""] };
    let clock = if width >= 40 { clock } else { "" };
    for (i, v) in variants.iter().enumerate() {
        let tag = if i == 0 { sim[0] } else { sim[1] };
        let used = name.len() + 3 + v.iter().map(Span::width).sum::<usize>() + tag.len() + 1 + clock.len() + 1;
        if used <= width as usize || i + 1 == variants.len() {
            let mut spans = vec![Span::styled(name.clone(), Style::new().add_modifier(Modifier::BOLD)), Span::raw("   ")];
            spans.extend(v.iter().cloned());
            spans.push(Span::styled(format!(" {tag}"), Style::new().add_modifier(Modifier::BOLD)));
            let used: usize = spans.iter().map(Span::width).sum();
            spans.push(Span::raw(" ".repeat((width as usize).saturating_sub(used + clock.len() + 1).max(1))));
            spans.push(Span::raw(clock.to_owned()));
            return Line::from(spans);
        }
    }
    Line::raw(name)
}

fn capacity(model: Option<&str>) -> f32 {
    match model {
        Some(m) if m.contains("1300") => 1300.0,
        _ => 1600.0,
    }
}

pub fn psu<'a>(snap: &Snapshot, theme: &Theme, width: u16) -> Line<'a> {
    let cap = capacity(snap.model.as_deref());
    let rails = snap.rails.unwrap_or_default();
    let out = rails.out_w.map_or("—".into(), |w| format!("{w:.0}"));
    let meter_w = if width >= 120 {
        50
    } else if width >= 80 {
        34
    } else {
        24
    };
    let mut spans = vec![Span::styled(" PSU  ", Style::new().add_modifier(Modifier::BOLD))];
    spans.extend(meter("OUT", rails.out_w.map(|w| w / cap), &format!("{out} / {cap:.0} W"), meter_w, theme.meter()));
    if snap.rails.is_some() {
        let names = ["12V", "5V", "3.3V"];
        let v = |i: usize| rails.volts[i].map_or("—".into(), |v| format!("{v:.2}"));
        if width >= 100 {
            let volts: Vec<String> = (0..3).map(|i| format!("{} {} V", names[i], v(i))).collect();
            spans.push(Span::raw(format!("   {}", volts.join("  "))));
        } else {
            spans.push(Span::raw(format!("  {}/{}/{} V", v(0), v(1), v(2))));
        }
        spans.push(Span::styled(if width >= 100 { "   inferred" } else { " inferred" }, theme.dim()));
    } else if snap.model.is_some() {
        spans.push(Span::styled("   rail data unavailable", theme.dim()));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_puts_the_value_inside() {
        let s: String = meter("POW", Some(0.5), "225 / 450 W", 30, Style::new()).iter().map(|s| s.content.to_string()).collect();
        assert_eq!(s.chars().count(), 30, "{s}");
        assert_eq!(s, "POW [||||||       225 / 450 W]");
    }
}
