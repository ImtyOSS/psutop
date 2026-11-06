# Contributing to psutop

psutop's goal is to show per-wire GPU cable current for **every PSU that measures it**. Today it
reads only the MSI MPG Ai1600TS (verified) and Ai1300TS (untested on Linux). Reaching more PSUs
takes people who own them, so there are two ways to help:

- **[Test a PSU](#test-a-psu-no-coding)**: run psutop on your PSU and report what you see. No
  coding needed.
- **[Add a PSU](#add-support-for-a-new-psu)**: write a read-only driver for a PSU psutop doesn't
  know yet.

Bug fixes and other improvements are welcome too; see
[Pull requests](#pull-requests).

## Which PSUs qualify

A PSU qualifies when **the PSU itself measures the current on each +12 V wire** of its 12V-2x6 /
12VHPWR GPU connectors and **reports it to the computer**, usually over a USB cable to a
motherboard header. A quick test: if the vendor's own app shows a current for each of the six
wires, the data is there.

PSUs that only measure total output, or GPU-side tools that estimate cable current, don't
qualify: psutop shows what the PSU measures, wire by wire.

## Test a PSU (no coding)

This is the most useful help right now, especially for the **MSI MPG Ai1300TS**, which shares
the Ai1600TS protocol but has never been run on Linux, and for any new driver waiting on
hardware.

1. Build psutop and set up access as in [docs/SETUP.md](docs/SETUP.md).
2. Check the setup and note what it prints:

   ```bash
   psutop doctor
   psutop info
   ```

3. Run `psutop` and watch it **idle**, then **under a GPU load** you already run (a game, a
   render, a training job). Each wire should read about 0.1–0.3 A at idle and climb evenly under
   load.
4. If you can, compare against an independent reading: the vendor's app on Windows, or a DC clamp
   meter around single wires. Note any wire that differs by more than about 0.3 A.
5. Save one sample and a packet trace:

   ```bash
   psutop once --json > sample.json
   psutop once --trace-io 2> trace.txt
   ```

   The trace shows every byte psutop sent, which is how you can see it only reads. For MSI PSUs
   psutop withholds the reply that carries the serial number; **read `trace.txt` before sharing
   it** and remove anything that looks like a serial number.
6. Open a **[PSU test report](https://github.com/ImtyOSS/psutop/issues/new?template=psu-report.yml)** issue with the results,
   your distribution, kernel, PSU model and firmware (from `psutop info`).

A report that says "it doesn't work" is just as useful as one that says it does: include the
`psutop doctor` output and any error message.

## Add support for a new PSU

A driver turns a PSU's USB protocol into psutop's vendor-neutral readings. Everything above the
driver (alert levels, alarms, cards, history, logs) works for any PSU unchanged.

Before writing code, open a **[New PSU](https://github.com/ImtyOSS/psutop/issues/new?template=new-psu.yml)** issue so work
isn't duplicated and the protocol notes have a home.

### Ground rules

These keep psutop safe to leave running on someone's machine. A pull request that breaks one
can't be merged.

- **Read-only.** A driver sends only the read requests and connect handshake the vendor's own
  software sends. Never write settings, never change protection limits, never touch firmware.
- **No guessing at the device.** Never send a byte you didn't first see the vendor's software
  send. No probing, no fuzzing, no "let's see what this register does".
- **Learn the protocol cleanly.** Capture what the vendor's app sends (see below) or use public
  vendor documentation. Don't copy code from proprietary software, and don't copy code from
  projects whose licence doesn't allow it (for example GPL code into this MIT project).
- **No personal data.** Strip serial numbers and other identifiers from captures, tests and docs
  before committing.

### 1. Capture and write down the protocol

On a Windows machine with the vendor's app installed:

1. Install [Wireshark](https://www.wireshark.org) with **USBPcap**.
2. Find the PSU's USB vendor and product ID (Device Manager → the PSU's HID device → Details →
   Hardware Ids, `VID_xxxx&PID_xxxx`).
3. Start a capture on the PSU's USB root hub, open the vendor's app, let it show per-wire current
   for a minute, then close it. Do it once at idle and once under GPU load.
4. Filter on the PSU's device address and read the HID reports going out and coming back.

Write what you find in `docs/psu/<vendor>-<model>.md`, with:

- USB vendor:product ID, the HID interface used, report size, and whether a report ID is sent.
- The connect sequence, if the app sends one before reading.
- **Every request the app sends**, byte for byte. This becomes the driver's complete allowlist.
- For each reply: where each wire's current is, its scaling and units, and anything else useful
  (the PSU's own alarm status, its limits, faults).
- How often the vendor's app polls, and whether it takes a lock (for example a named Windows
  mutex) that other readers must respect.
- A few sample replies at idle and under load, **with serial numbers removed**. These become the
  driver's golden-frame tests.

The MSI notes in [SPEC.md §5](SPEC.md) are a worked example.

### 2. Write the driver crate

Create `drivers/source-<vendor>/` as a new workspace member, modelled on the MSI driver in
[third_party/meltalarm/source-msi](third_party/meltalarm/source-msi):

| MSI file | What it does | What yours needs |
|---|---|---|
| `protocol.rs` | A closed `Request` enum, the one function that builds request bytes, reply parsing, golden-frame and allowlist tests | Your requests and replies |
| `transport.rs` | One bounded request/reply transaction and the single device-write call | The same shape, for your report format |
| `lib.rs` | `MsiDriver` (discovery and connect) and `MsiSource` (one poll → one `Report`) | Your `Driver` and `Source` |
| `lock.rs` | The vendor's coexistence lock | Your vendor's lock, if it has one |

Your crate depends on two vendor-neutral crates:

- [`meltalarm-source-api`](third_party/meltalarm/source-api/src/lib.rs) defines the `Driver` and
  `Source` traits and the shared `HidContext`.
- [`meltalarm-model`](third_party/meltalarm/model/src/lib.rs) defines what a poll returns:
  `SourceInfo`, `Report`, `ConnectorReading` (six wires per connector), `Verdict`,
  `Protection` and `Fault`.

**The driver rules** (from `source-api`; all are required):

1. A closed request enum; exactly one private function builds request bytes.
2. Exactly one device-write call site, marked `#[allow(clippy::disallowed_methods)]`.
   [`clippy.toml`](clippy.toml) denies every other HID write, so a stray write fails the build.
3. No byte sent to the device comes from settings, the UI, the command line or anything else
   outside the driver.
4. Every transaction is bounded in time; one poll takes at most 3 s.
5. Respect the vendor's coexistence lock.
6. Report anything unavailable as `None`, never as zero. A missing reading is "unknown", not
   "0 A".
7. Ship golden-frame tests built from your captures.
8. `discover` performs the vendor's complete connect sequence: `Found` means the PSU answered.
   Present but silent is `NotReady`.

**Mapping to the model:**

- `SourceInfo.id`: a stable `vendor:model` id such as `msi:ai1600ts`. It keys settings and logs.
- `connectors`: one entry per measured GPU connector, numbered as on the PSU.
- `caps`: say honestly what the PSU provides: its own verdict, its limits, a power-cut timer, and
  flags naming the wire that tripped. psutop's alarms work without any of them; it simply shows
  less.
- `protection_name`: the vendor's name for its protection feature, if it has one.
- Add a test like MSI's allowlist test that lists every request your driver can build and
  checks it against your capture notes.

### 3. Connect it to psutop

These parts of psutop still assume MSI. The first new driver gets to generalise them.

- **Feed** ([src/source.rs](src/source.rs)): add a `Feed` for your driver next to the `msi`
  module: `discover`, `poll`, `release`. Return `rails: None` and `raw: None` unless you also add
  a decoder for those panes ([src/frame.rs](src/frame.rs) is MSI-only).
- **Choosing a feed** ([src/main.rs](src/main.rs), `Ctx::feed`): today it always returns the
  MSI feed. Change it to try each driver's discovery in turn and use the first PSU found.
- **Device lookup** ([src/source.rs](src/source.rs)): `USB_IDS`, `find_hidraw_in` and
  `LOCK_PATH` hold MSI's IDs and lock file. Move them into a per-driver table.
- **Packet trace**: `trace_line` withholds MSI's serial-number reply. Add the same for your
  PSU's serial number, if its protocol carries one.
- **`psutop doctor`** ([src/doctor.rs](src/doctor.rs)): add your USB IDs and model names to its
  checks.
- **udev rule** ([packaging/71-msi-psu.rules](packaging/71-msi-psu.rules)): add a rule for your
  vendor:product ID with `TAG+="uaccess"`, so the person at the machine can read it without
  sudo. Never use `MODE="0666"`.
- **Connector count**: the screen shows exactly two cable cards (`cards: [Card; 2]` in
  [src/monitor.rs](src/monitor.rs)). If your PSU has one connector or more than two, say so in
  your issue first: the layout needs work.

### 4. Test it

On any platform:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

On Linux, also the driver crates and their allowlist tests:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Then run the hardware checks in [SPEC.md §12](SPEC.md) on the real PSU, or ask an owner to run
them through a [PSU test report](https://github.com/ImtyOSS/psutop/issues/new?template=psu-report.yml):

1. `psutop once --trace-io` shows only the requests in your allowlist.
2. Readings match an independent source at idle and under load.
3. Ten minutes in the TUI with no "no data" ticks and low CPU.
4. Unplugging and replugging the PSU's USB cable recovers on its own.
5. `psutop once` works without sudo once the udev rule is installed.

### 5. Document it

- Add the PSU to **Supported hardware** in [README.md](README.md), with its USB ID and an honest
  status ("Verified on Linux", or "Untested on Linux").
- Add a row to **Tested on** for each hardware check that was run, with the date.
- Credit where the protocol knowledge came from in **Credits**.

## Pull requests

- Keep each pull request to one change; a new driver can be one pull request.
- Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and `cargo test` before
  pushing. CI runs them on Linux and macOS.
- UI changes show up as snapshot diffs in `tests/snapshots`. Review them, then accept them with
  `INSTA_UPDATE=always cargo test`.
- Write commit messages in the [Conventional Commits](https://www.conventionalcommits.org)
  style: `feat: add Example PSU driver`, `fix: …`, `docs: …`.
- By contributing, you agree that your work is released under the [MIT License](LICENSE).

Security problems go through GitHub's private report page, not an issue: see
[SECURITY.md](SECURITY.md).
