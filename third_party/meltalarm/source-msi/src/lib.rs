// Copied from MeltAlarm (MIT, © 2026 Denis Domanskii), crates/source-msi/src/lib.rs at commit 7763086.
// Maintained in psutop; see third_party/meltalarm/README.md and LICENSE.
//! MeltAlarm source for MSI MPG Ai1300TS / Ai1600TS power supplies (GPU Safeguard+).
//!
//! Read-only by construction: the only bytes this crate can send come from `protocol::packet`
//! over the closed `Request` enum (8 reads + MSI's connect handshake), and they reach the
//! device through a single call site in `transport` (docs/ARCHITECTURE.md §9).
//!
//! For frontends that show more than the [`Report`] (raw frames, packet traces),
//! [`discover_msi`] keeps the concrete [`MsiSource`]: [`MsiSource::last_frames`] returns the
//! last poll's replies and an optional [`Tap`] observes every packet. Neither can change what
//! is sent.

mod lock;
pub mod protocol;
mod transport;

use std::time::{Duration, Instant};

use meltalarm_model::{Capabilities, ConnectorInfo, ConnectorReading, Report, SourceId, SourceInfo, Verdict};
use meltalarm_source_api::hidapi::HidDevice;
use meltalarm_source_api::{Discovery, Driver, HidContext, Source};

use lock::PsuLock;
use protocol::{Frame, PID_AI1300TS, PID_AI1600TS, Reg, Request, VID, device_status};
pub use transport::{Tap, TxError, Wire};

const CONFIG_REFRESH: Duration = Duration::from_secs(60);

pub struct MsiDriver;

impl Driver for MsiDriver {
    fn name(&self) -> &'static str {
        "MSI MPG Ai1300TS / Ai1600TS"
    }

    fn discover(&self, hid: &mut HidContext) -> Discovery {
        match discover_msi(hid, None) {
            Ok(src) => Discovery::Found(Box::new(src)),
            Err(d) => d,
        }
    }
}

/// [`MsiDriver::discover`] keeping the concrete source, with `tap` observing every packet from
/// the handshake on. `Err` is any outcome but `Found`.
pub fn discover_msi(hid: &mut HidContext, tap: Option<Tap>) -> Result<MsiSource, Discovery> {
    let api = match hid.refreshed() {
        Ok(api) => api,
        Err(e) => return Err(Discovery::NotReady(format!("USB HID is not available ({e})"))),
    };
    // The product id identifies a Safeguard+ (TS) model: MSI Center and the Afterburner
    // plugin select the TS protocol by these PIDs alone.
    let Some(dev_info) = api.device_list().find(|d| d.vendor_id() == VID && [PID_AI1300TS, PID_AI1600TS].contains(&d.product_id())) else {
        return Err(Discovery::NotPresent);
    };
    let pid = dev_info.product_id();
    let device = match dev_info.open_device(api) {
        Ok(d) => d,
        Err(e) => return Err(Discovery::NotReady(format!("PSU found but could not be opened ({e})"))),
    };
    let lock = match PsuLock::platform() {
        Ok(l) => l,
        Err(msg) => return Err(Discovery::Unusable(msg)),
    };
    MsiSource::identify(device, lock, pid, tap).map_err(|e| Discovery::NotReady(format!("PSU found but not answering ({e:?})")))
}

/// The replies of the last poll, from the opcode on; `None` where the read failed or, for the
/// Safeguard+ config, was not due this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PollFrames {
    pub telemetry: Option<Frame>,
    pub safeguard_status: Option<Frame>,
    pub protection_flags: Option<Frame>,
    pub safeguard_config: Option<Frame>,
}

pub struct MsiSource {
    device: HidDevice,
    lock: PsuLock,
    info: SourceInfo,
    last_config: Option<Instant>,
    tap: Option<Tap>,
    frames: PollFrames,
}

impl MsiSource {
    /// MSI's connect sequence (`CONNECT_PSU`): handshake, then identity. The PSU must answer to
    /// count as found (source rule 8). Its model string is only for display.
    fn identify(device: HidDevice, lock: PsuLock, pid: u16, tap: Option<Tap>) -> Result<Self, TxError> {
        transport::transact(&device, &lock, Request::Handshake, tap.as_ref())?;
        let text = |reg| transport::transact(&device, &lock, Request::Read(reg), tap.as_ref()).map(|f| f.text());
        let (id, default_model) = if pid == PID_AI1600TS { ("msi:ai1600ts", "MPG Ai1600TS") } else { ("msi:ai1300ts", "MPG Ai1300TS") };
        let reported = text(Reg::MfrModel)?;
        let model = if reported.starts_with("MPG") { reported } else { default_model.to_owned() };
        let info = SourceInfo {
            id: SourceId(id.into()),
            vendor: text(Reg::MfrId).unwrap_or_else(|_| "MSI".into()),
            model,
            firmware: text(Reg::MfrRevision).ok().filter(|s| !s.is_empty()),
            serial: text(Reg::MfrSerial).ok().filter(|s| !s.is_empty()),
            connectors: (0..2).map(|i| ConnectorInfo { index: i }).collect(),
            caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
            protection_name: Some("Safeguard+".into()),
        };
        Ok(MsiSource { device, lock, info, last_config: None, tap, frames: PollFrames::default() })
    }

    fn read(&self, reg: Reg) -> Result<Frame, TxError> {
        transport::transact(&self.device, &self.lock, Request::Read(reg), self.tap.as_ref())
    }

    fn read_config(&mut self, now: Instant) -> Option<Frame> {
        let due = self.last_config.is_none_or(|t| now.duration_since(t) >= CONFIG_REFRESH);
        if !due {
            return None;
        }
        let f = self.read(Reg::SafeguardConfig).ok()?;
        self.last_config = Some(now);
        Some(f)
    }

    /// The replies of the last [`Source::poll`].
    pub fn last_frames(&self) -> &PollFrames {
        &self.frames
    }
}

impl Source for MsiSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn poll(&mut self, _hid: &mut HidContext) -> Report {
        let at = Instant::now();
        let telemetry = self.read(Reg::Telemetry);
        let status = self.read(Reg::SafeguardStatus);
        let flags_frame = self.read(Reg::ProtectionFlags);
        let config = self.read_config(at);
        let flags = flags_frame.as_ref().map(|f| f.flags());
        let protection = config.as_ref().map(Frame::protection);
        self.frames = PollFrames {
            telemetry: telemetry.as_ref().ok().cloned(),
            safeguard_status: status.as_ref().ok().cloned(),
            protection_flags: flags_frame.as_ref().ok().cloned(),
            safeguard_config: config,
        };

        let readings = telemetry.ok().map(|f| {
            let w = f.wire_currents();
            (0..2).map(|i| ConnectorReading { index: i as u8, wires: w[i].map(Some) }).collect()
        });
        let verdicts = status.as_ref().ok().map(|f| {
            let codes = f.status_codes();
            (0..2)
                .map(|i| Verdict {
                    index: i as u8,
                    status: device_status(codes[i]),
                    // Wire flags are secondary detail: if E1 failed this tick, show none.
                    flagged: flags.as_ref().map(|fl| fl.wires[i]).unwrap_or([false; 6]),
                })
                .collect()
        });
        let diagnostic = status.as_ref().ok().filter(|f| f.status_codes() != [0, 0]).map(|f| format!("C1 {}", f.hex()));

        Report { at, readings, verdicts, faults: flags.ok().map(|f| f.faults()), protection, diagnostic }
    }
}
