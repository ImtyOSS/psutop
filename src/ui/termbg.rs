//! Asks the terminal for its background colour (OSC 11), so the card's quiet shades (bar
//! tracks, the OK pill, the divider) can be blended over it: MeltAlarm's track is "white at
//! 9 %" over the card (SPEC §7.3), and psutop's cards sit on the terminal's own background.
//!
//! The query is followed by a Device Attributes request (DA1), which every terminal answers.
//! Replies come back in order, so once the DA1 reply is in, an OSC 11 reply either came first
//! or isn't coming: psutop never waits on a terminal that ignores OSC 11, and it reads both
//! replies itself before the TUI starts reading input.

use std::io::Write;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};

pub type Rgb = (u8, u8, u8);

/// The colour in an OSC 11 reply: `rgb:RRRR/GGGG/BBBB`, 1–4 hex digits per channel.
pub fn parse_osc11(reply: &[u8]) -> Option<Rgb> {
    let s = String::from_utf8_lossy(reply);
    let rest = &s[s.find("]11;rgb:")? + 8..];
    let chan = |p: &str| -> Option<u8> {
        let hex: String = p.chars().take_while(char::is_ascii_hexdigit).collect();
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(&hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len())) - 1;
        u8::try_from((v * 255 + max / 2) / max).ok()
    };
    let mut parts = rest.splitn(3, '/');
    Some((chan(parts.next()?)?, chan(parts.next()?)?, chan(parts.next()?)?))
}

/// The DA1 reply (`ESC [ ? … c`) has arrived.
fn da1_done(b: &[u8]) -> bool {
    b.windows(3).position(|w| w == b"\x1b[?").is_some_and(|i| b[i + 3..].contains(&b'c'))
}

/// Ask the terminal for its background. Needs raw mode (no echo, no line buffering); waits at
/// most `timeout`, normally a few milliseconds.
pub fn query(timeout: Duration) -> Option<Rgb> {
    let mut out = std::io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
    out.flush().ok()?;
    let stdin = std::io::stdin();
    let fd = stdin.as_fd();
    let deadline = Instant::now() + timeout;
    let mut got = Vec::new();
    while !da1_done(&got) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let ts = Timespec::try_from(left).ok()?;
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        if !matches!(poll(&mut fds, Some(&ts)), Ok(n) if n > 0) {
            break;
        }
        let mut buf = [0u8; 256];
        match rustix::io::read(fd, &mut buf) {
            Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    parse_osc11(&got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_osc11_replies() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:1f1f/2424/3030\x1b\\\x1b[?62;22c"), Some((0x1f, 0x24, 0x30)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ff/80/00\x07"), Some((255, 128, 0)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:f/0/8\x07"), Some((255, 0, 136)));
        assert_eq!(parse_osc11(b"\x1b[?1;2c"), None, "only DA1: the terminal ignores OSC 11");
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zz/00/00\x07"), None);
    }

    #[test]
    fn da1_ends_the_wait() {
        assert!(!da1_done(b"\x1b]11;rgb:1f1f/2424/3030\x1b\\"));
        assert!(da1_done(b"\x1b]11;rgb:1f1f/2424/3030\x1b\\\x1b[?62;22c"));
        assert!(da1_done(b"\x1b[?1;2c"));
    }
}
