# Code copied from MeltAlarm

These crates are copied from [MeltAlarm](https://github.com/DeniDoman/meltalarm) at commit
`7763086` (v0.3.9), © 2026 Denis Domanskii, under the MIT License in [`LICENSE`](LICENSE).
psutop owns and maintains these copies; nothing is fetched from MeltAlarm at build time.
Each copied file starts with a line naming its MeltAlarm origin.

| Here | MeltAlarm | What it is |
|---|---|---|
| `model/` | `crates/model` | The vendor-neutral data model: wires, readings, verdicts, faults |
| `core/` | `crates/core` | Levels, the alarm state machine, log events and the view model |
| `source-api/` | `crates/source-api` | The `Driver`/`Source` traits and the shared HID context |
| `source-msi/` | `crates/source-msi` | The read-only driver for MSI MPG Ai1300TS / Ai1600TS PSUs |

`model` and `source-api` say nothing about any vendor, so a driver for another PSU with
per-wire sensing goes in a new crate next to `source-msi`.

## What changed from MeltAlarm

All crates:

- `Cargo.toml`: MeltAlarm's workspace keys replaced with explicit values; the crates refer
  to each other by path. Lints are MeltAlarm's.
- An attribution header at the top of every source file.

`source-msi`, for raw frames and `--trace-io` (SPEC §3.2):

- `protocol.rs`: `Frame::bytes()`.
- `transport.rs`: the `Wire` observer events and `Tap`; `transact` takes an optional tap. One
  new test.
- `lib.rs`: `discover_msi`, `PollFrames` and `MsiSource::last_frames()`; `MsiDriver::discover`
  calls `discover_msi(hid, None)`.

The same `source-msi` change, as a patch against MeltAlarm, is in
[`docs/upstream`](../../docs/upstream).

`core`, so that no screen names the Windows app (a psutop test checks every simulator
scenario):

- `lib.rs`: "MeltAlarm can't read the PSU" is now "The PSU stopped answering"; the
  can't-reach notice says psutop keeps trying.
- `alarm.rs`: "measured by psutop", "psutop cannot confirm…", and "The note is in
  MeltAlarm." dropped.
- `ladder.rs`: "Details are in MeltAlarm." dropped.
- `view.rs`: tooltip and connecting texts start with `psutop ·`; "psutop still alarms."
- `settings.rs`, `state.rs`: file headers say psutop.
- `limits.rs`: an `alarm_seconds` of NaN falls back to the defaults with a warning instead of
  panicking.
- `tests.rs`: the same texts.

The `INSTALL` log line in `log.rs` still names MeltAlarm: only MeltAlarm's Windows installer
writes it.

The read-only contract is unchanged: one packet builder over the closed request enum, one
device-write call site, the allowlist test, and the clippy `disallowed_methods` deny. The
crates' tests run with `cargo test --workspace` (`source-api` and `source-msi` on Linux
only).

## Taking later MeltAlarm changes

Nothing updates these copies automatically. To take a fix from MeltAlarm, diff its crate
against the copy at `7763086`, port the change by hand, run `cargo test --workspace` on Linux
and the hardware checks in SPEC §12, and record the new commit here and in the file headers.
