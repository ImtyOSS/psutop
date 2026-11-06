//! MeltAlarm's dark palette (SPEC §7.3) at the terminal's colour depth.
//!
//! Colours, state roles and the pill and strip looks come from MeltAlarm (MIT, © 2026 Denis Domanskii, github.com/DeniDoman/meltalarm),
//! `docs/DESIGN.md` "State colors". The series and meter colours are psutop's own.

use meltalarm_core::{Level, StatusKind};
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    True,
    C256,
    C16,
    /// `NO_COLOR` / `--no-color`: state by shape and text only.
    Mono,
}

impl Depth {
    /// From the environment: `NO_COLOR`, `COLORTERM`, `TERM`.
    pub fn detect(no_color: bool) -> Depth {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        if no_color || !env("NO_COLOR").is_empty() {
            Depth::Mono
        } else if matches!(env("COLORTERM").as_str(), "truecolor" | "24bit") {
            Depth::True
        } else if env("TERM").contains("256color") {
            Depth::C256
        } else {
            Depth::C16
        }
    }
}

/// A role in the card, resolved to a colour by [`Theme::color`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    CardBg,
    Border,
    Track,
    Neutral,
    Text,
    Text2,
    Caution,
    WarnGfx,
    WarnText,
    AlarmBand,
    PillOk,
    /// The dot in the `OK` pill (DESIGN.md "Popup": neutral fill, green dot).
    OkDot,
    PillCaution,
    ClearedBand,
    /// nvtop parts only, never inside cards: chart lines and header meters.
    Series1,
    Series2,
    Meter,
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub depth: Depth,
    pub nerd_font: bool,
    /// The terminal's background, when it told us (OSC 11): the quiet shades are blended over
    /// it instead of using fixed greys made for MeltAlarm's grey card.
    pub background: Option<(u8, u8, u8)>,
}

const fn rgb(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Role {
    fn hex(self) -> u32 {
        match self {
            Role::CardBg => 0x2B2B2B, // MeltAlarm's card fill; psutop draws no fill (see `color`)
            Role::Border => 0x3A3A3A,
            Role::Track => 0x3E3E3E,
            Role::Neutral => 0xE6E6E6,
            Role::Text => 0xF2F2F2,
            Role::Text2 => 0xA8A8A8,
            Role::Caution => 0xF5A623,
            Role::WarnGfx => 0xFF4D4F,
            Role::WarnText => 0xFF6B6D,
            Role::AlarmBand => 0xC8102E,
            Role::PillOk => 0x3A3A3A,
            Role::OkDot => 0x4ADE80,
            Role::PillCaution => 0x4A3B22,
            Role::ClearedBand => 0x1E7F45,
            Role::Series1 => 0x7FC8CF,
            Role::Series2 => 0xC79BE0,
            Role::Meter => 0x8CC48F,
        }
    }

    fn ansi16(self) -> Color {
        match self {
            Role::CardBg => Color::Reset,
            Role::Border | Role::Track | Role::PillOk => Color::DarkGray,
            Role::Neutral | Role::Text => Color::White,
            Role::Text2 => Color::Gray,
            Role::Caution => Color::Yellow,
            Role::PillCaution => Color::Reset,
            Role::WarnGfx | Role::WarnText => Color::LightRed,
            Role::AlarmBand => Color::Red,
            Role::ClearedBand | Role::Meter | Role::OkDot => Color::Green,
            Role::Series1 => Color::Cyan,
            Role::Series2 => Color::Magenta,
        }
    }
}

/// The nearest xterm-256 colour: the 6×6×6 cube or the grey ramp.
pub fn to_256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let near = |v: u8| STEPS.iter().enumerate().min_by_key(|(_, s)| (i32::from(**s) - i32::from(v)).abs()).map(|(i, _)| i).unwrap_or(0);
    let (ci, cj, ck) = (near(r), near(g), near(b));
    let cube = (STEPS[ci], STEPS[cj], STEPS[ck]);
    let grey_i = ((i32::from(r) + i32::from(g) + i32::from(b)) / 3 - 3).clamp(0, 230) / 10;
    let grey_v = (8 + grey_i * 10) as u8;
    let d = |(a, b2, c): (u8, u8, u8)| {
        let sq = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        sq(a, r) + sq(b2, g) + sq(c, b)
    };
    if d((grey_v, grey_v, grey_v)) < d(cube) { 232 + grey_i as u8 } else { 16 + 36 * ci as u8 + 6 * cj as u8 + ck as u8 }
}

impl Theme {
    pub fn new(depth: Depth, nerd_font: bool) -> Self {
        Theme { depth, nerd_font, background: None }
    }

    pub fn with_background(mut self, background: Option<(u8, u8, u8)>) -> Self {
        self.background = background;
        self
    }

    /// Shades defined as a colour at some opacity over the background (DESIGN.md: the track is
    /// white at 9 %, the Caution chip amber at 18 %), blended over the real background.
    fn blended(&self, role: Role) -> Option<(u8, u8, u8)> {
        let bg = self.background?;
        let (colour, alpha) = match role {
            Role::Track => (0xFFFFFF, 0.09),
            Role::PillOk | Role::Border => (0xFFFFFF, 0.13),
            Role::PillCaution => (Role::Caution.hex(), 0.18),
            _ => return None,
        };
        let (r, g, b) = rgb(colour);
        let mix = |c: u8, under: u8| (f32::from(c) * alpha + f32::from(under) * (1.0 - alpha)).round() as u8;
        Some((mix(r, bg.0), mix(g, bg.1), mix(b, bg.2)))
    }

    pub fn color(&self, role: Role) -> Color {
        // Cards sit on the terminal's own background, like the chart.
        if role == Role::CardBg {
            return Color::Reset;
        }
        let (r, g, b) = self.blended(role).unwrap_or_else(|| rgb(role.hex()));
        match self.depth {
            Depth::True => Color::Rgb(r, g, b),
            Depth::C256 => Color::Indexed(to_256(r, g, b)),
            Depth::C16 => role.ansi16(),
            Depth::Mono => Color::Reset,
        }
    }

    pub fn mono(&self) -> bool {
        self.depth == Depth::Mono
    }

    pub fn fg(&self, role: Role) -> Style {
        Style::new().fg(self.color(role))
    }

    pub fn card(&self) -> Style {
        Style::new().bg(self.color(Role::CardBg))
    }

    /// Text inside a card: on the card background.
    pub fn on_card(&self, role: Role) -> Style {
        self.card().fg(self.color(role))
    }

    pub fn level_gfx(&self, level: Level) -> Role {
        match level {
            Level::Normal => Role::Neutral,
            Level::Caution => Role::Caution,
            Level::Warning => Role::WarnGfx,
        }
    }

    pub fn level_text(&self, level: Level) -> Role {
        match level {
            Level::Normal => Role::Text,
            Level::Caution => Role::Caution,
            Level::Warning => Role::WarnText,
        }
    }

    /// A value coloured by its level, bold; alarm values are reverse video without colour.
    pub fn value(&self, level: Level) -> Style {
        let s = self.on_card(self.level_text(level)).add_modifier(Modifier::BOLD);
        if self.mono() && level == Level::Warning { s.add_modifier(Modifier::REVERSED) } else { s }
    }

    /// The `( ● OK )` pill: background, dot and text styles.
    pub fn pill(&self, status: StatusKind) -> Style {
        if self.mono() {
            return match status {
                StatusKind::Alarm => Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                _ => Style::new().add_modifier(Modifier::BOLD),
            };
        }
        match status {
            StatusKind::Normal => Style::new().bg(self.color(Role::PillOk)).fg(self.color(Role::Neutral)),
            StatusKind::Caution => Style::new().bg(self.color(Role::PillCaution)).fg(self.color(Role::Caution)),
            StatusKind::Alarm => Style::new().bg(self.color(Role::AlarmBand)).fg(Color::White).add_modifier(Modifier::BOLD),
            StatusKind::NoData | StatusKind::NotConnected => Style::new().bg(self.color(Role::PillOk)).fg(self.color(Role::Text2)),
        }
    }

    /// A dot coloured apart from the pill's text: the green one in `OK`. Other states draw the
    /// dot in their text colour, and `No data` has none.
    pub fn pill_dot(&self, status: StatusKind) -> Option<Style> {
        (status == StatusKind::Normal && !self.mono()).then(|| self.pill(status).fg(self.color(Role::OkDot)))
    }

    /// The dot inside the pill: none for `No data` (DESIGN.md "Popup").
    pub fn pill_text(&self, status: StatusKind, text: &str) -> String {
        match status {
            StatusKind::NoData | StatusKind::NotConnected => format!(" {text} "),
            _ => format!(" ● {text} "),
        }
    }

    /// Round pill caps with `--nerd-font`.
    pub fn pill_caps(&self) -> (&'static str, &'static str) {
        if self.nerd_font { ("\u{e0b6}", "\u{e0b4}") } else { ("", "") }
    }

    pub fn border(&self, alarm: bool) -> Style {
        if alarm {
            if self.mono() { Style::new().add_modifier(Modifier::BOLD) } else { self.card().fg(self.color(Role::WarnGfx)) }
        } else {
            // The chart's frame colour: without a card fill, MeltAlarm's dark #3A3A3A border
            // would all but vanish.
            self.card().fg(self.color(Role::Text2))
        }
    }

    /// A cable's colour in the chart (its watts and its hottest wire); never inside cards.
    pub fn series(&self, i: usize) -> Style {
        if self.mono() {
            return Style::new();
        }
        self.fg([Role::Series1, Role::Series2][i % 2])
    }

    pub fn meter(&self) -> Style {
        if self.mono() { Style::new() } else { self.fg(Role::Meter) }
    }

    pub fn dim(&self) -> Style {
        if self.mono() { Style::new() } else { Style::new().fg(self.color(Role::Text2)) }
    }

    pub fn strip(&self, kind: crate::monitor::StripKind) -> Style {
        use crate::monitor::StripKind::*;
        if self.mono() {
            return match kind {
                Alarm => Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                _ => Style::new().add_modifier(Modifier::BOLD),
            };
        }
        match kind {
            Caution => Style::new().fg(self.color(Role::Caution)),
            Alarm => Style::new().bg(self.color(Role::AlarmBand)).fg(Color::White).add_modifier(Modifier::BOLD),
            Cleared => Style::new().bg(self.color(Role::ClearedBand)).fg(Color::White).add_modifier(Modifier::BOLD),
        }
    }

    pub fn banner(&self, level: Level) -> Style {
        if self.mono() {
            return Style::new().add_modifier(Modifier::BOLD);
        }
        match level {
            Level::Warning => Style::new().bg(self.color(Role::AlarmBand)).fg(Color::White).add_modifier(Modifier::BOLD),
            _ => Style::new().fg(self.color(Role::Caution)),
        }
    }

    /// Function-bar key names: reverse video, like nvtop.
    pub fn key(&self) -> Style {
        Style::new().add_modifier(Modifier::REVERSED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_256_colours() {
        assert_eq!(to_256(0, 0, 0), 16);
        assert_eq!(to_256(255, 255, 255), 231);
        assert_eq!(to_256(0x2B, 0x2B, 0x2B), 236);
        assert_eq!(to_256(0xF5, 0xA6, 0x23), 214);
        assert_eq!(to_256(0xC8, 0x10, 0x2E), 160);
    }

    #[test]
    fn shades_blend_over_the_terminal_background() {
        let plain = Theme::new(Depth::True, false);
        assert_eq!(plain.color(Role::Track), Color::Rgb(0x3E, 0x3E, 0x3E), "no answer: MeltAlarm's grey");
        let navy = Theme::new(Depth::True, false).with_background(Some((0x20, 0x24, 0x30)));
        // white at 9 % over #202430
        assert_eq!(navy.color(Role::Track), Color::Rgb(0x34, 0x38, 0x43));
        assert_eq!(navy.color(Role::Neutral), plain.color(Role::Neutral), "fills and state colours don't change");
        assert_eq!(navy.color(Role::CardBg), Color::Reset);
    }
}
