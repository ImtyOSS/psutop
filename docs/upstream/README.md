# Upstream proposal: raw frames and a packet observer for `meltalarm-source-msi`

`meltalarm-raw-frames.patch` is the same change as the vendored copy in
`third_party/meltalarm/source-msi`, written against MeltAlarm commit `7763086`. It applies
with `git apply` from the MeltAlarm repository root. It has **not** been sent upstream; the
text below is a draft pull-request description.

---

## source-msi: expose the last poll's frames and an optional packet observer

**Why.** Frontends other than the Windows tray app want more than the `Report`. A Linux TUI
built on these crates (psutop) shows the raw replies, decodes `E0` words MeltAlarm leaves
unused (rail amps, output watts, rail volts), and offers a packet trace that proves which bytes
went to the PSU. `Frame` bytes are `pub(crate)` and `MsiDriver::discover` returns a boxed
`dyn Source`, so none of this is possible from outside the crate today.

**What changes** (3 files, about +140 / −44):

- `Frame::bytes()` returns the reply from the opcode on (64 bytes).
- `transport::Wire` (`Sent` / `Drained` / `Received`) and `Tap = Box<dyn Fn(Wire) + Send>`.
  `transact` takes `Option<&Tap>` and calls it with borrowed bytes: after the request is
  written, and for every report it reads, including the ones it drains.
- `discover_msi(hid, tap) -> Result<MsiSource, Discovery>` keeps the concrete source.
  `MsiDriver::discover` now calls it with `None`, so its behaviour is unchanged.
- `MsiSource::last_frames()` returns `PollFrames` (E0, C1, E1 and, when it was due, C0) from
  the last `poll`.

**Read-only contract.** Unchanged and still enforced:

- `packet()` is still the only packet builder, over the closed `Request` enum, and the
  allowlist test is untouched.
- There is still exactly one `#[allow(clippy::disallowed_methods)]` write call site.
- The observer is called after the write and only ever sees `&[u8]`; it has no path to the
  device and cannot change what is sent.
- A new transport test checks that the tap sees the exact packet that was written and that
  observing sends nothing.

**Not changed.** The Windows lock, timing, polling cadence and `Report` contents. No new
dependencies.

**Docs to update with it.** `docs/ARCHITECTURE.md` §4 (the source API surface) and §11.1 (a
Linux frontend can use `discover_msi` directly).
