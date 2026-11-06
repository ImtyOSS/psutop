//! Where samples come from: the real PSU through `meltalarm-source-msi` (Linux) or the
//! simulator, both behind [`Feed`], and the acquisition thread that polls them.
//!
//! The MSI feed holds `flock` on [`LOCK_PATH`] for each discovery and each tick, so psutop
//! instances and `msi-psu-read.py` never interleave requests (SPEC §11.3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use meltalarm_model::{Report, SourceInfo};

use crate::frame::{Rails, RawFrames};

pub const LOCK_PATH: &str = "/run/lock/msi-psu.lock";
pub const REDISCOVER_EVERY: Duration = Duration::from_secs(2);
/// Consecutive unhealthy ticks before the connection is dropped and rediscovered.
const DROP_AFTER_UNHEALTHY: u32 = 3;
/// Samples waiting for a reader that has stalled (a `watch` pipe nobody reads, a terminal
/// paused with Ctrl-S). Past this, new samples are dropped rather than stored without limit;
/// each sample keeps the time it was read, so the ones kept are never shown as fresh.
pub const QUEUE: usize = 64;
pub const USB_IDS: &str = "0db0:808c or 0db0:aa6f";
pub const UDEV_RULE: &str = "/etc/udev/rules.d/71-msi-psu.rules";

/// What to do about a permission error, for messages.
pub fn permission_hint() -> String {
    format!("Run `sudo psutop`, or install {UDEV_RULE} for use at this machine; `psutop doctor` shows what's missing.")
}

/// One tick: MeltAlarm's report plus what psutop adds.
#[derive(Clone, Debug)]
pub struct Sample {
    pub report: Report,
    /// PSU-wide output data; `None` when this source cannot provide it (SPEC §3.2).
    pub rails: Option<Rails>,
    pub raw: Option<RawFrames>,
}

impl Sample {
    pub fn empty(at: Instant) -> Self {
        Sample { report: Report::empty(at), rails: None, raw: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Discovered {
    Found(SourceInfo),
    /// No supported PSU on USB.
    Absent,
    /// Present but not answering (yet).
    NotReady(String),
    /// Present, but this user may not open it.
    Denied(String),
    Unusable(String),
}

pub trait Feed: Send {
    /// Discover and connect (MSI: handshake and identity). `Found` means the PSU answered.
    fn discover(&mut self) -> Discovered;
    /// One tick. Bounded; failures are `None` fields, never zeros.
    fn poll(&mut self) -> Sample;
    /// Drop the connection; the next `discover` reconnects.
    fn release(&mut self);
    /// The device node, for `info` and messages.
    fn device_node(&self) -> Option<String> {
        None
    }
    fn simulated(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub enum FeedEvent {
    Searching(Discovered),
    Connected(SourceInfo),
    Sample(Box<Sample>),
    Lost,
}

pub struct Acquisition {
    pub rx: Receiver<FeedEvent>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Acquisition {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            // The thread may be blocked sending a state event into a full queue: keep emptying
            // it until the thread sees `stop` and ends.
            while !h.is_finished() {
                while self.rx.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = h.join();
        }
    }
}

/// Sleep until `deadline` in short steps; `false` if asked to stop.
fn sleep_until(deadline: Instant, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// Discover, then poll every `interval` until the feed goes quiet, then rediscover; forever.
pub fn spawn(mut feed: Box<dyn Feed>, interval: Duration) -> Acquisition {
    let (tx, rx) = mpsc::sync_channel(QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("psu-acquisition".into())
        .spawn(move || run(feed.as_mut(), interval, &tx, &flag))
        .expect("spawn acquisition thread");
    Acquisition { rx, stop, handle: Some(handle) }
}

fn run(feed: &mut dyn Feed, interval: Duration, tx: &SyncSender<FeedEvent>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let info = match feed.discover() {
            Discovered::Found(info) => info,
            other => {
                if tx.send(FeedEvent::Searching(other)).is_err() || !sleep_until(Instant::now() + REDISCOVER_EVERY, stop) {
                    return;
                }
                continue;
            }
        };
        let caps = info.caps;
        if tx.send(FeedEvent::Connected(info)).is_err() {
            return;
        }
        let mut unhealthy = 0;
        let mut next = Instant::now();
        loop {
            let s = feed.poll();
            let healthy = s.report.is_healthy(&caps);
            match tx.try_send(FeedEvent::Sample(Box::new(s))) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return,
            }
            unhealthy = if healthy { 0 } else { unhealthy + 1 };
            if unhealthy >= DROP_AFTER_UNHEALTHY {
                feed.release();
                if tx.send(FeedEvent::Lost).is_err() {
                    return;
                }
                break;
            }
            next += interval;
            let now = Instant::now();
            if next < now {
                next = now;
            }
            if !sleep_until(next, stop) {
                return;
            }
        }
    }
}

/// `/dev/hidrawN` of a supported PSU, from sysfs (read-only).
pub fn find_hidraw() -> Option<String> {
    find_hidraw_in(std::path::Path::new("/"))
}

/// [`find_hidraw`] under another root (tests use a fake sysfs).
pub fn find_hidraw_in(root: &std::path::Path) -> Option<String> {
    let class = root.join("sys/class/hidraw");
    let dir = std::fs::read_dir(&class).ok()?;
    let mut nodes: Vec<String> = dir.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    nodes.sort();
    nodes.into_iter().find_map(|n| {
        let uevent = std::fs::read_to_string(class.join(&n).join("device/uevent")).ok()?;
        let id = uevent.lines().find_map(|l| l.strip_prefix("HID_ID="))?.to_ascii_uppercase();
        ["0003:00000DB0:0000808C", "0003:00000DB0:0000AA6F"].contains(&id.as_str()).then(|| format!("/dev/{n}"))
    })
}

/// A `--trace-io` byte dump: the bytes up to the last non-zero one, then the count of zero
/// bytes after it, so a request reads `00 51 E0 + 62 × 00`.
pub fn trace_bytes(b: &[u8]) -> String {
    let end = b.iter().rposition(|&x| x != 0).map_or(0, |i| i + 1);
    let head: Vec<String> = b[..end].iter().map(|x| format!("{x:02X}")).collect();
    match (head.is_empty(), b.len() - end) {
        (true, z) => format!("{z} × 00"),
        (false, 0) => head.join(" "),
        (false, z) => format!("{} + {z} × 00", head.join(" ")),
    }
}

/// One `--trace-io` payload. Replies to register `0x13` carry the PSU's serial number, so their
/// data is withheld: traces get pasted into bug reports.
pub fn trace_line(b: &[u8]) -> String {
    let off = usize::from(b.first() == Some(&0));
    if b.get(off..off + 2) == Some(&[0x51, 0x13]) && b[off + 2..].iter().any(|&x| x != 0) {
        return format!("{} 51 13 + {} bytes withheld (serial number)", if off == 1 { "00" } else { "" }, b.len() - off - 2)
            .trim_start()
            .to_owned();
    }
    trace_bytes(b)
}

#[cfg(target_os = "linux")]
pub mod msi {
    use std::fs::{File, OpenOptions, TryLockError};
    use std::time::{Duration, Instant};

    use meltalarm_source_api::{Discovery, HidContext, Source};
    use meltalarm_source_msi::protocol::Frame;
    use meltalarm_source_msi::{MsiSource, Tap, Wire, discover_msi};

    use super::{Discovered, Feed, LOCK_PATH, Sample, find_hidraw, trace_line};
    use crate::frame::{self, RawFrames};

    /// MSI's own clients hold their lock for about 250 ms.
    const LOCK_TIMEOUT: Duration = Duration::from_secs(1);

    /// `flock` on [`LOCK_PATH`], shared with every other PSU reader on this host.
    ///
    /// The lock only keeps cooperating readers from interleaving requests; MeltAlarm's
    /// drain-and-echo matching already ignores foreign replies. So a lock held past the timeout
    /// (by a stuck reader or another local user) never stops monitoring: the tick runs without
    /// it. The file sits in world-writable `/run/lock`, so it is opened with `O_NOFOLLOW` and
    /// never written.
    struct PsuLock(Option<File>);

    impl PsuLock {
        fn open() -> Self {
            use std::os::unix::fs::OpenOptionsExt;
            // Anyone can create files in /run/lock. O_NONBLOCK keeps a named pipe planted there
            // from blocking the open forever, and only a regular file is used as the lock.
            let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK;
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o666)
                .custom_flags(flags)
                .open(LOCK_PATH)
                .or_else(|_| OpenOptions::new().read(true).custom_flags(flags).open(LOCK_PATH))
                .ok()
                .filter(|f| f.metadata().is_ok_and(|m| m.is_file()));
            PsuLock(file)
        }

        /// Run `f` under the lock, or without it once the lock has been busy for the timeout.
        fn with<T>(&self, f: impl FnOnce() -> T) -> T {
            let Some(file) = &self.0 else { return f() };
            let deadline = Instant::now() + LOCK_TIMEOUT;
            let locked = loop {
                match file.try_lock() {
                    Ok(()) => break true,
                    Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break false,
                }
            };
            let out = f();
            if locked {
                let _ = file.unlock();
            }
            out
        }
    }

    /// `--trace-io`: every packet on stderr, timestamped.
    fn trace_tap() -> Tap {
        Box::new(|w: Wire<'_>| {
            let (dir, b) = match w {
                Wire::Sent(b) => ("sent    ", b),
                Wire::Received(b) => ("received", b),
                Wire::Drained(b) => ("drained ", b),
            };
            eprintln!("{} {dir} {}", chrono::Local::now().format("%H:%M:%S%.3f"), trace_line(b));
        })
    }

    pub struct MsiFeed {
        hid: HidContext,
        lock: PsuLock,
        source: Option<MsiSource>,
        trace: bool,
        /// The Safeguard+ config is read once a minute; the raw pane keeps the last one.
        c0: Option<Vec<u8>>,
    }

    impl MsiFeed {
        pub fn new(trace: bool) -> Self {
            MsiFeed { hid: HidContext::new(), lock: PsuLock::open(), source: None, trace, c0: None }
        }
    }

    impl Feed for MsiFeed {
        fn discover(&mut self) -> Discovered {
            self.source = None;
            let hid = &mut self.hid;
            let tap = self.trace.then(trace_tap);
            match self.lock.with(|| discover_msi(hid, tap)) {
                Ok(src) => {
                    let info = src.info().clone();
                    self.source = Some(src);
                    Discovered::Found(info)
                }
                Err(Discovery::NotPresent) => Discovered::Absent,
                Err(Discovery::NotReady(msg)) if msg.contains("could not be opened") && msg.contains("os error 13") => {
                    Discovered::Denied(msg)
                }
                Err(Discovery::NotReady(msg)) => Discovered::NotReady(msg),
                Err(Discovery::Unusable(msg)) => Discovered::Unusable(msg),
                Err(Discovery::Found(_)) => Discovered::NotReady("unexpected discovery result".into()),
            }
        }

        fn poll(&mut self) -> Sample {
            let at = Instant::now();
            let Some(src) = self.source.as_mut() else { return Sample::empty(at) };
            let hid = &mut self.hid;
            let report = self.lock.with(|| src.poll(hid));
            let frames = src.last_frames();
            let bytes = |f: &Option<Frame>| f.as_ref().map(|f| f.bytes().to_vec());
            if let Some(c0) = bytes(&frames.safeguard_config) {
                self.c0 = Some(c0);
            }
            let raw = RawFrames {
                e0: bytes(&frames.telemetry),
                c1: bytes(&frames.safeguard_status),
                e1: bytes(&frames.protection_flags),
                c0: self.c0.clone(),
                simulated: false,
            };
            let rails = raw.e0.as_deref().map(frame::rails);
            Sample { report, rails, raw: Some(raw) }
        }

        fn release(&mut self) {
            self.source = None;
        }

        fn device_node(&self) -> Option<String> {
            find_hidraw()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that stops reading never makes the queue grow past [`QUEUE`], and dropping the
    /// acquisition still ends its thread.
    #[test]
    fn a_stalled_reader_never_grows_the_queue_past_its_bound() {
        let acq = spawn(Box::new(crate::sim::SimFeed::new("normal")), Duration::from_millis(1));
        // Hundreds of polls, none read.
        std::thread::sleep(Duration::from_millis(400));
        // Stop the thread before counting: reading while it runs frees slots it refills at once.
        acq.stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !acq.handle.as_ref().is_some_and(|h| h.is_finished()) {
            assert!(Instant::now() < deadline, "the thread did not stop");
            std::thread::sleep(Duration::from_millis(5));
        }
        let queued = acq.rx.try_iter().count();
        assert!(queued <= QUEUE, "{queued} queued");
        // A full queue never keeps shutdown waiting.
        let acq = spawn(Box::new(crate::sim::SimFeed::new("normal")), Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        drop(acq);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_serial_number_reply_is_withheld() {
        let mut reply = vec![0x51, 0x13, 0x0E];
        reply.extend_from_slice(b"20260101-00000");
        reply.resize(64, 0);
        let line = super::trace_line(&reply);
        assert_eq!(line, "51 13 + 62 bytes withheld (serial number)");
        assert!(!line.contains("32 30"));
        // The request for it, and other registers, print in full.
        let mut req = [0u8; 65];
        req[1..3].copy_from_slice(&[0x51, 0x13]);
        assert_eq!(super::trace_line(&req), "00 51 13 + 62 × 00");
        assert_eq!(super::trace_line(&[0x51, 0x11, 0x4D]), "51 11 4D");
    }

    #[test]
    fn trace_bytes_shows_the_request_head_and_its_zero_tail() {
        let mut req = [0u8; 65];
        req[1..3].copy_from_slice(&[0x51, 0xE0]);
        assert_eq!(trace_bytes(&req), "00 51 E0 + 62 × 00");
        assert_eq!(trace_bytes(&[0x51, 0xE1, 0, 0]), "51 E1 + 2 × 00");
        assert_eq!(trace_bytes(&[0, 0]), "2 × 00");
        assert_eq!(trace_bytes(&[0xFA, 0x51]), "FA 51");
    }
}
