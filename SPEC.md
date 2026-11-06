# psutop: 12V-2x6 Cable Monitor TUI — Spec

Status: **design spec, 2026-10-15**, kept with the code: comments cite its sections (§).
It was written for one reference machine, an MSI MPG Ai1600TS feeding two GPUs;
details specific to that machine's network and accounts are left out of this copy.

## 1. Goal

A terminal program for a Linux machine with an MSI MPG Ai1600TS / Ai1300TS PSU that
shows, live, how much current each wire of both GPU power cables carries.

**Look and feel (user requirement, §7):**

- The screen feels like `nvtop`: full-screen, dense, keyboard-driven, with
  device meters, a history chart and a function-key bar.
- Each cable is drawn as MeltAlarm's cable card: `GPU power cable` with an
  `● OK` / `● Caution` pill, six cut bars with values, and `Imbalance` /
  `PSU status`. This matches the two screenshots the user supplied.

On the reference machine an MSI MPG Ai1600TS feeds two GPUs through its two 12V-2x6
connectors. Which GPU is on which connector has to be
found on each machine (§10).

psutop is built on **MeltAlarm** (§3) and reuses its code wherever it fits.
It is verified against a real Ai1600TS (§12). A small read-only Python reader was the
prototype and is the reference for the parity check.

## 2. Scope

**v1**

- Full-screen nvtop-style TUI: a PSU header meter, two MeltAlarm-style cable
  cards, a history chart, and a function-key bar (§7).
- Plain-text and machine-readable modes (`once`, `watch`) for SSH and
  logging.
- A label per cable (`GPU 0` / `GPU 1` by default), set in a config file (§10).
- Read-only PSU access, and nothing else: no GPU vendor library or tool, so it
  works the same with NVIDIA, AMD or Intel GPUs.
- Runs as a normal user without `sudo`, after a one-time udev rule and group (§11.1).

**Non-goals for v1**

- Any write to the PSU: fan mode, Safeguard+ settings, buzzer, save.
- Changing GPU power limits or acting on alarms. It is a monitor, not a
  protector; only the PSU firmware cuts power.
- Reading GPU stats (power, utilization, temperature). An early build read
  them through NVML; it was dropped to stay vendor-neutral and dependency-free.
- Daemon, systemd unit, Prometheus exporter, or Homepage widget (v2, §15).
- Hardware support on Windows or macOS. On the Mac it runs the simulator
  only.
- PSUs other than Ai1600TS `0db0:808c` and Ai1300TS `0db0:aa6f`.

## 3. MeltAlarm: reference and reuse

[MeltAlarm](https://github.com/DeniDoman/meltalarm) (MIT, © 2026 Denis
Domanskii) is a read-only Windows per-wire monitor for exactly these PSUs. It
is a Rust 2024 workspace (`rust-version` 1.88). Commit `7763086`
(2026-10-01, v0.3.9) was reviewed for this spec. psutop reads its docs as the
reference and copies its crates into `third_party/meltalarm` instead of
re-implementing them; the copies are psutop's to maintain.

- **Docs to read first:** `docs/FUNCTIONAL_SPEC.md` (verified facts F1–F21,
  limits, alarm rules, read-only contract) and `docs/ARCHITECTURE.md`. Its
  §11.1 already plans a Linux frontend that reuses every portable crate;
  psutop is in effect that frontend as a TUI.
- **Before starting:** check upstream for a newer release or an existing
  Linux frontend, and check what overlaps with Railwatch, which MeltAlarm
  mentions.

### 3.1 Crate-by-crate plan

| MeltAlarm crate | Contents | psutop use |
|---|---|---|
| `meltalarm-model` | `WIRES`, `Report`, `ConnectorReading`, `Verdict`, `DeviceStatus`, `Protection`, `Fault` | **Copied unchanged** as psutop's data vocabulary |
| `meltalarm-core` | `limits::Limits::V1`, `levels::evaluate`, cable guard / alarm state machine, `view::bar_share` knee scale, all user-facing texts | **Copied** for levels, alarms and bar heights (§8, §7); rules unchanged, texts that named MeltAlarm reworded |
| `meltalarm-source-msi` | `protocol` (closed `Request` enum, 9-packet allowlist, LINEAR11, reply classification, tests), `transport::transact` (drain, write, match echo), `MsiDriver`/`MsiSource` | **Reuse** for discovery, handshake, wires, status, flags and Safeguard+ config |
| `meltalarm-source-api` | `Driver`/`Source` traits, `HidContext` over hidapi | **Reuse**; it is required by `source-msi` (build impact: §4) |
| `meltalarm-source-sim` | Scenarios: idle, normal, caution, overload, uneven, red, alarm, critical, fault, nodata, silent, slow (env `MELTALARM_SIM`) | **Reuse** for `--sim`. Its `LOAD` and `HOT` arrays are the screenshots psutop's look is matched against |
| `meltalarm-runtime` | Acquisition thread, discovery loop, settings and alarm-log stores, `ViewModel` | **Evaluate in M1.** Use it if its `ViewModel` and log fit a TUI; otherwise drive `Source` and `Core` directly |
| `meltalarm-lifecycle`, `meltalarm-win` | Windows install, tray, notch UI | Not used |

### 3.2 Gaps psutop fills

- **Inferred PSU rail data.** `E0` words 0–2, 15 and 19–21 hold rail amps,
  total output watts and rail volts (§5). They are not exposed:
  `Frame`'s bytes are `pub(crate)` and `Report` has no rail fields. Options:
  a small upstream PR that exposes `Frame` bytes or a raw-frame hook
  (preferred), or a vendored `source-msi` with that change. Decide in M1.
  Until then, the header shows only what `Report` carries.
  *Status 2026-10-15:* vendored (`third_party/meltalarm/source-msi`) with `Frame::bytes()`, `MsiSource::last_frames()` and a read-only `Tap`
  observer; the same change is ready as an upstream patch in `docs/upstream/`.
- **Linux coexistence lock.** `PsuLock` is `None` on Linux. psutop adds an
  `flock` on `/run/lock/msi-psu.lock` (§11.3); offer it upstream.
- **New features:** the ratatui frontend, CLI modes, cable labels, and
  history.

### 3.3 Reuse rules

- *Changed 2026-11-06:* the crates are copied into `third_party/meltalarm/` (path
  dependencies; no git dependency on MeltAlarm), so psutop can change them and add
  drivers for other PSUs. Every copied file names its origin, MeltAlarm's `LICENSE`
  sits beside them, and `third_party/meltalarm/README.md` lists what changed. Later
  MeltAlarm fixes are ported by hand, then the hardware checks (§12) re-run.
- Credit MeltAlarm in psutop's `README` and `--version` output. MIT requires
  keeping the copyright and permission notice with substantial copies.
- Offer Linux fixes upstream too (framing, lock, raw-frame access).
- Keep MeltAlarm's read-only contract: its allowlist test, the single write
  call site, and the clippy `disallowed_methods` deny must stay green inside
  psutop as well.

## 4. Stack and build

**Rust + [ratatui](https://ratatui.rs) + crossterm**, with `clap`, `serde`
and `toml`.

- **Build target: Linux (`x86_64-unknown-linux-gnu`), built on the machine with the
  PSU.** MeltAlarm's `source-api` uses hidapi's `linux-native` backend, which pulls
  `udev` → `libudev-sys` and links C `libudev`. That makes a static musl cross-build
  impractical.
  1. Build natively with a user-local `rustup` toolchain; the libudev development files
     (`libudev-dev` on Debian/Ubuntu, `systemd-devel` on Fedora) are a one-time install.
     A throwaway container with them installed works too and leaves the host unchanged.
  2. A musl static build remains possible only if `source-api`'s hidapi is swapped for a
     direct hidraw transport or a pure-Rust udev feature. Not worth it for v1.
- **No GPU libraries.** psutop links only libudev (for hidapi) and libc.
- **Mac:** `cargo test` for the portable crates and `--sim` UI work. Rust
  1.94 is installed. Hardware code paths compile only for Linux.

## 5. PSU protocol (as implemented by MeltAlarm)

- Device: hidraw by USB ID, never a fixed `hidraw11`. Handshake `FA 51` once
  per connection; after a cold boot the PSU answers nothing until it gets
  this. Reads are `51 <reg>`.
- Allowlist: `10 11 12 13 C0 C1 E0 E1` reads plus the handshake. Opcode `50`,
  register `F1` and register `C2` must never be sent.
- On Linux hidraw, writes are 65 bytes (report number `00` plus 64) and reads
  are 64 bytes. MeltAlarm's parser accepts the report ID present or
  stripped. Verified live on 2026-10-15.
- Polling: `E0`, `C1`, `E1` every tick (default 1 s, 500 ms to 2 s: the alarm logic treats
  samples more than 3 s apart as a break and data older than 2.5 s as stale, so a slower
  poll would delay or miss overload alarms). `C0`
  every 60 s. Identity at connect only.

`E0` field map. Offsets are LINEAR11 words counted from the reply's data:

| Field | Words | Source |
|---|---|---|
| Cable 1 wires 1–6, A | 3–8 | MeltAlarm, verified live |
| Cable 2 wires 1–6, A | 9–14 | MeltAlarm, verified live |
| 12V / 5V / 3.3V output A | 0 / 1 / 2 | **inferred on this unit** (§3.2) |
| Total output W | 15 | inferred: rail A × V matched within 2.3% |
| 12V / 5V / 3.3V volts | 19 / 20 / 21 | inferred (12.09 / 5.03 / 3.33 V at idle) |
| Unknown | 16 (~78), 17 (61) | raw view only |

The UI labels inferred values "inferred".

## 6. CLI

```text
psutop                     full-screen TUI (default; needs a TTY)
psutop once [--json]       one sample, text or JSON, then exit
psutop watch [--interval 1s] [--live] [--csv | --json] [--count N]
                           stream samples; on a TTY uses the card style (§7.7),
                           --live redraws in place instead of scrolling
psutop info                identity, firmware, Safeguard+ config, device node
psutop doctor              check the setup step by step and print the fix for each gap
psutop --sim <scenario>    any mode on meltalarm-source-sim data, no hardware

Global: --config <path>  --interval <dur>  --raw  --no-color
        --nerd-font      round pill caps (§7.4)
        --trace-io       log every packet sent and received to stderr (§12)
```

- With no TTY (`TERM=dumb`), `psutop` prints a hint and runs `once`.
- Exit codes: `0` ok, `2` PSU not found, `3` permission denied (the message
  names the udev rule), `4` PSU not answering.
- `--json` has stable field names, with `null` for unread values; never 0.

## 7. Look and feel (required)

The user asked for this explicitly: running the monitor in a terminal should
**feel like `nvtop`**, and each cable should **look like MeltAlarm's cable
card**. These are requirements, not suggestions, and §14 makes them
acceptance criteria.

### 7.1 Visual references

| Reference | What to take from it |
|---|---|
| `nvtop` 3.2.0 | Full-screen dashboard; one dense header line per device with inline `[label ||||   value]` meters; a framed time-series chart with a legend; a bottom function-key bar (`F1Help F2… F10Quit`) with keys in reverse video; instant start; 1 Hz refresh; no mouse needed |
| MeltAlarm `docs/img/flyout.png` (user screenshot 1) | The **OK cable card**: title `GPU power cable`, a grey pill badge `● OK`, subtitle `Current per wire, A`, six cut bars, bold value under each bar, thin divider, footer columns `Imbalance` / `PSU status` with a small grey label above a bold value |
| MeltAlarm `docs/img/floating-compact.png` (user screenshot 2) | The **Caution card**: amber-tinted pill `● Caution`; the hot wire's bar and value in amber (`9.9`), others neutral; an amber reason line `9.9 A · over rating` |
| MeltAlarm `docs/img/caution-strip.png` | The one-line caution strip: `⚠ A wire at 9.9 A, above the 9.5 A rating · Ease the GPU load` in an amber frame |
| MeltAlarm `docs/DESIGN.md` → "Decisions", "State colors", "Cut bars" | The rules below, which psutop follows |

The images are in the MeltAlarm repo at the pinned commit
(`https://github.com/DeniDoman/meltalarm/tree/7763086/docs/img`). Copy them
into psutop's `docs/reference/` with the MIT notice so the comparison
survives upstream changes.

### 7.2 Design rules (from MeltAlarm DESIGN.md)

- **Dark cockpit.** Normal wires use the neutral tone. Colour appears only
  for Caution, Warning, ALARM and No data. **Colour means state, never
  decoration**, inside the cable cards.
- **State words, exactly:** `OK`, `Caution`, `ALARM`, `No data`,
  `Not connected`. The PSU's own verdict appears only as information
  ("PSU status", "reported by the PSU").
- **Names.** Use `GPU power cable 1` / `GPU power cable 2`; there are two
  cables, so they are always numbered. Say "Imbalance", never "spread". No
  wire numbers on bars, and no total current inside a card.
- **Cut bars.** One per wire, value underneath.
  - The **top of the bar is the alarm limit** (`Limits::alarm`, 10.5 A), so a
    full bar means alarm.
  - The bar is **cut straight across at the rating** (9.5 A): the short head
    sits above the cut, the body below.
  - **Knee scale** (`core::view::bar_share`): 0–6 A fills the bottom 20%, and
    6 A up to the alarm limit fills the remaining 80%.
  - The fill takes the wire's state colour: neutral; amber from the rating,
    where the fill jumps the cut; red at the alarm limit, a full bar.
  - The track is one neutral tone, with no hatching, labels or limit lines.
- **Motion.** Nothing moves at rest, and data never animates. Only a state
  change may flash once.

### 7.3 Palette (MeltAlarm dark theme)

| Role | Hex | Terminal use |
|---|---|---|
| Card background | the terminal's own (MeltAlarm: `#2B2B2B`) | none: cards sit on the terminal background, like the chart |
| Card border | `#A8A8A8`, the chart frame's colour | rounded border `╭╮╰╯` |
| Divider | white at 13 % over the background (`#3A3A3A` without an answer) | `─` divider inside the card |
| Track | white at 9 % over the terminal's background (asked with OSC 11 at start; `#3E3E3E` if the terminal doesn't answer) | empty bar cells |
| Neutral fill | `#E6E6E6` | OK bars |
| Primary text | `#F2F2F2` | titles, values (bold) |
| Secondary text | `#A8A8A8` | `Current per wire, A`, footer labels |
| Caution | `#F5A623` | bar, value, badge dot/text, reason line, strip frame |
| Warning (graphics / text) | `#FF4D4F` / `#FF6B6D` | bars and text at the alarm limit |
| PSU alarm band | `#C8102E` + white text | ALARM banner |
| Badge pill (OK) | white at 13 % over the background (`#3A3A3A` without an answer), `#4ADE80` (green) dot, `#E6E6E6` text | `( ● OK )` |
| Badge pill (Caution) | amber at 18 % over the background (`#4A3B22` without an answer), `#F5A623` dot and text | `( ● Caution )` |

- **Colour depth:**
  - `COLORTERM=truecolor`/`24bit`: use these exact hex values.
  - Otherwise, the nearest xterm-256 colours.
  - With 16 colours: default background, white or grey neutrals, yellow
    for Caution, red for Warning and ALARM.
  - With `NO_COLOR` or `--no-color`: state shown by shape only. Caution
    bars use `▓`, alarm bars are reverse video, and the badge text carries
    the state.
- The nvtop-style parts (header meters, chart lines) may use per-device
  accent colours like nvtop does, for example GPU0 cyan and GPU1 magenta.
  Cards never do.

### 7.4 Drawing the card in a terminal

- **Card:** a rounded-border block, about 56 columns × 17 rows.
  - Title line: `GPU power cable N` (bold), then the badge pill, with the
    mapped GPU name right-aligned in secondary text.
  - The pill is padded text on its pill background. With `--nerd-font`,
    round caps `` / `` are added.
- **Bars:** each bar is 3 cells wide with 4 cells between bars, and 10 rows
  of height plus 1 cut row.
  - Fill and track are drawn with half-block cells (`▀`/`▄` with fg/bg
    colours), giving 2 vertical steps per row.
  - Rounded ends use quadrant caps: `▗▄▖` on top, `▝▀▘` at the bottom.
  - The cut is one blank half-row in the card background at
    `bar_share(9.5, 10.5)`.
- **Values:** 1 decimal, bold, centred under each bar, in the state colour.
- **Below the values:** a `─` divider, then two footer columns: `Imbalance`
  (value coloured by its level) and `PSU status`. Each has a secondary-text
  label above a bold value.
- **Live reason lines** go between the bars and the divider, in amber or
  red: `9.9 A · over rating`, `Uneven load`, `PSU: Imbalance · power cut in
  ~2:47`. Texts come from `meltalarm-core` where it provides them.
- **No data:** hollow bars (track only) and `—` instead of values; the badge
  reads `● No data` in grey.

### 7.5 Full screen (wide, ≥ 120 × 40)

Structure only. Legend: `░` track, `█`/`▄` neutral fill, `▓` amber fill.
The blank row inside the bars is the cut at 9.5 A. Bar values are
MeltAlarm's `normal`/`caution` simulator values; they are higher than these
450 W-limited GPUs would draw.

```text
 psutop 0.1   MSI MPG Ai1600TS · fw 10 · Safeguard+ ON (12.0 A / 5.5 A, 20 s → cut 180 s)                        14:02:11
 PSU  OUT [|||||||||||||||||||||||                           1084 / 1600 W]   12V 12.06 V  5V 5.03 V  3.3V 3.33 V   inferred
╭ GPU power cable 1  ( ● OK )                      GPU 0 ╮ ╭ GPU power cable 2  ( ● Caution )                      GPU 1 ╮
│ Current per wire, A                                     │ │ Current per wire, A                                     │
│                                                         │ │                                                         │
│    ░░░    ░░░    ░░░    ░░░    ░░░    ░░░               │ │    ░░░    ░░░    ▄▄▄    ░░░    ░░░    ░░░               │
│    ░░░    ░░░    ░░░    ░░░    ░░░    ░░░               │ │    ░░░    ░░░    ▓▓▓    ░░░    ░░░    ░░░               │
│                                                         │ │                                                         │
│    ░░░    ░░░    ░░░    ░░░    ░░░    ░░░               │ │    ▄▄▄    ▄▄▄    ▓▓▓    ▄▄▄    ░░░    ▄▄▄               │
│    ░░░    ░░░    ░░░    ░░░    ░░░    ░░░               │ │    ███    ███    ▓▓▓    ███    ███    ███               │
│    ▄▄▄    ░░░    ░░░    ░░░    ▄▄▄    ▄▄▄               │ │    ███    ███    ▓▓▓    ███    ███    ███               │
│    ███    ███    ▄▄▄    ███    ███    ███               │ │    ███    ███    ▓▓▓    ███    ███    ███               │
│    ███    ███    ███    ███    ███    ███               │ │    ███    ███    ▓▓▓    ███    ███    ███               │
│    ███    ███    ███    ███    ███    ███               │ │    ███    ███    ▓▓▓    ███    ███    ███               │
│    ███    ███    ███    ███    ███    ███               │ │    ███    ███    ▓▓▓    ███    ███    ███               │
│    8.1    7.8    7.7    7.8    8.0    8.1               │ │    8.9    9.1    9.9    9.0    8.8    9.0               │
│                                                         │ │ 9.9 A · over rating                                     │
│ ─────────────────────────────────────────────────────── │ │ ─────────────────────────────────────────────────────── │
│ Imbalance                 PSU status                    │ │ Imbalance                 PSU status                    │
│ 0.4 A                     Normal                        │ │ 1.1 A                     Normal                        │
╰─────────────────────────────────────────────────────────╯ ╰─────────────────────────────────────────────────────────╯
 ⚠ GPU power cable 2 · A wire at 9.9 A, above the 9.5 A rating · Ease the GPU load
╭ History · 5 min ───────────────────────────────────────────────────────── ── cable 1 W  ── cable 2 W  ┄┄ max wire A ╮
│ 700 W│                 ⢀⣠⣴⣶⣾⣿⣿⣿⣿⣿⣿⣷⣶⣦⣤⣀                                                                           │
│      │           ⣀⣤⣶⣿⠿⠛⠉               ⠉⠛⠿⣷⣦⡀                                                                    │
│   0 W│⣀⣀⣀⣀⣀⣀⣤⠾⠋                              ⠙⢷⣄⣀⣀⣀                                                            │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
 F1Help  F3Raw  F5Pause  F6Window  F9Log  F10Quit                                                           1 Hz · log off
```

- **Header (nvtop style):**
  - A title line with PSU identity and Safeguard+ config.
  - A `PSU` line: total output meter out of 1600 W and the rail volts,
    marked `inferred`.
  - Meter fill uses `|`, like nvtop.
- **Cards** follow §7.4. The card for a cable in an alarm state gets a red
  border. The caution or alarm strip sits under the cards; it is one line,
  amber or red, and only shown while active.
- **History (nvtop chart):** step lines in box-drawing characters on a fixed
  0–20 A scale (ticks every 5 A). Each cable has one line, in its own colour:
  its hottest wire, so a hot wire is tied to its cable and what it powers. As in
  nvtop, a slot holds whole samples (one per poll, or several when the window
  needs more; a slot shows the highest), counted from the start, so the chart
  scrolls one slot at a time and old slots never change. The two lines take
  turns by column (cable 1 left, cable 2 right in each slot); in a cable's
  column the other line carries on at its level and meets a step with `┴ ┬ ┼`,
  so neither hides the other. Samples missed while the PSU was away become
  gaps, so the time axis stays true. The legend, one line per cable, sits at the top left
  inside the plot: the GPU the cable feeds (`GPU 0`, or the cable's label from the config) and
  its line's newest value (`8.3 A`). Window 1 / 5 / 15 / 60 min. The ring buffer holds 60 min at
  1 Hz.
- **Function bar (nvtop style):** key names in reverse video, then the label.
  - `F1`/`?` Help, `F3`/`r` Raw pane, `F5`/`p` Pause.
  - `F6`/`←→` history window, `F9`/`l` CSV log
    (`~/.local/state/psutop/psutop-YYYYMMDD.csv`), `F10`/`q`/`Esc` Quit.
  - `1`/`2` zooms one card to full width; `c` clears the peak markers (zoom
    view only).
- **Raw pane:** replaces the history and shows the last `E0`/`C1`/`E1`/`C0`
  frames in hex, with inferred and unknown words decoded beside them.

### 7.6 Smaller terminals

- **80–119 columns:** the cards stack vertically. The header keeps one line
  per GPU with shorter meters, and the history shrinks to 4 rows.
- **60–79 columns:** compact cards in the `floating-compact.png` layout:
  title and badge, bars at 6 rows plus the cut, values, reason line, and no
  footer.
- **Under 60 columns or under 24 rows:** one line per cable, plus the
  function bar:

  ```text
  GPU power cable 1 ( ● OK )      ▆▅▅▅▆▆  8.1 7.8 7.7 7.8 8.0 8.1  Imbalance 0.4 A
  GPU power cable 2 ( ● Caution ) ▇▇█▇▇▇  8.9 9.1 9.9 9.0 8.8 9.0  9.9 A · over rating
  ```

### 7.7 Monitoring from the command line (`watch`, `once`)

The non-full-screen modes share the same vocabulary and colours, so a
monitoring command in a normal terminal still looks like the card.

- **TTY output:** `psutop watch` prints one block per tick. It has a
  timestamp, then one line per cable with mini cut-bars (`▁▂▃▄▅▆▇█` on the
  knee scale, each in its wire's state colour), the values, the badge,
  `Imbalance`, `PSU status`, and any reason line.
- **Updating in place:** `--live` rewrites the block instead of scrolling,
  for a `watch`-like single screen without the full TUI.
- **Piped output, `--csv` and `--json`:** no colour and no glyphs, with
  stable fields.
- `psutop once` prints the same block once.

### 7.8 Rendering details

- **Start-up:** draw the first frame within 300 ms, showing
  `Not connected`/`No data` until the first sample arrives.
- **Redraws:** only on new data, a state change or a resize. No spinner and
  no animated bars; at most one flash on a state change.
- **Screen handling:** use the alternate screen, hide the cursor, and
  restore both on exit or crash (panic hook).
- **Glyphs:** stay within Unicode block elements and box-drawing. Use
  braille for charts only, and Nerd-font glyphs only with `--nerd-font`.

## 8. Alert levels (MeltAlarm core)

psutop's copy of `meltalarm-core` keeps MeltAlarm's rules unchanged (only texts
that named MeltAlarm are reworded), so its alerts match MeltAlarm's.
Defaults `Limits::V1` (engineering defaults, not safe limits; overridable in
config):

| Rule | Value |
|---|---|
| Caution: a wire at or above the contact rating | 9.5 A |
| Overload episode: alarm after a delay | ≥ 10.5 A for 4 s |
| Fast alarm: second reading at or above | 12.0 A |
| Instant alarm: one reading at or above | 15.0 A |
| Uneven load: imbalance (max − min) while average ≥ 3.0 A | 3.0 A |
| Release hysteresis | 0.5 A |

- **PSU verdict on top:** `C1` status ≠ 0 or any `E1` wire flag always raises
  the alarm. Status 3 (> 18 A, critical) shows `POWER CUT ANY SECOND`.
  Status 1 or 2 shows an approximate countdown: cut-after (`C0`, 180 s)
  minus the time since the status went non-zero.
- PSU-wide `E1` flags (rail OCP, OPP, OTP, fan) raise a header banner.
- `--bell` rings the terminal bell on entering an alarm; it is off by
  default.
- Each level change writes a CSV log line when logging is on. Reuse core's
  log events or runtime's `LogSink` (decide in M1).

## 9. TUI states without data

| Situation | Display |
|---|---|
| PSU not found | Full-screen message with the USB IDs; rediscover every 2 s |
| Permission denied | Message naming the udev rule (§11.1); exit code 3 in non-TUI modes |
| PSU present, not answering | Panels show NO DATA; handshake retried every 2 s |

## 10. Cable labels

The PSU only knows "cable 1" and "cable 2". Each card and chart line also shows
a label for what the cable powers, from `~/.config/psutop/config.toml`:

```toml
[cables.1]
label = "GPU 0"      # the default; e.g. "RTX 4090" or "top card"

[cables.2]
label = "GPU 1"

[limits]             # optional meltalarm-core LimitOverrides
rating = 9.5
```

Without a config, cable 1 is `GPU 0` and cable 2 is `GPU 1`. To find which GPU
is on which cable, load one GPU and watch which cable's watts rise; idle values
cannot tell them apart (on 2026-10-15 at idle, both cables read 12–14 W).
`gpu_uuid` keys from earlier builds are accepted and ignored.

## 11. Installation and permissions

### 11.1 Permissions

Linux creates the PSU's hidraw node for root only. psutop supports two ways in:

- **`sudo psutop`**, with no setup. The default over SSH and on headless machines.
  Under `sudo`, the config is read from the invoking user's home (`SUDO_USER`).
- **The udev rule** `packaging/71-msi-psu.rules`, for use at the machine itself:

```text
KERNEL=="hidraw*", ATTRS{idVendor}=="0db0", ATTRS{idProduct}=="808c", TAG+="uaccess"
KERNEL=="hidraw*", ATTRS{idVendor}=="0db0", ATTRS{idProduct}=="aa6f", TAG+="uaccess"
```

```bash
sudo install -m 0644 packaging/71-msi-psu.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw
```

`TAG+="uaccess"` gives the user of the active local session access through a
systemd-logind ACL (the file sorts before `73-seat-late.rules`), with no group and no
re-login, the same pattern liquidctl and OpenRGB use. It does not cover SSH sessions. Anyone
who can open the node can send the PSU any command, including ones that switch off
Safeguard+, so no group is granted by default; the rule file describes an optional
dedicated group for SSH use without `sudo`. `psutop doctor` (§6) checks the setup.

### 11.2 Install

- Build from source (§4, `docs/SETUP.md`); install the binary as
  `~/.local/bin/psutop`, or `/usr/local/bin/psutop` for system-wide use.

### 11.3 Coexistence and robustness

- Linux has no MSI mutex, and replies reach every open handle. Each
  transaction takes `flock` on `/run/lock/msi-psu.lock`; other readers on the
  machine should take the same lock.
- MeltAlarm's drain-and-echo check already ignores foreign replies.
- **USB reset or a new hidraw number:** rediscover by USB ID, handshake
  again, and show NO DATA in between.
- **Shutdown:** `SIGTERM`/`SIGINT` and resizes restore the terminal cleanly.

## 12. Hardware verification

These checks need a machine with the PSU attached. Everything else (unit tests,
simulator UI work) runs anywhere, macOS included.

**Loop**

1. Develop with `--sim`. Run `cargo test` there.
2. Build on the machine with the PSU and run `cargo test --workspace`.
3. Until the udev rule and group exist, hardware runs need `sudo`.

**Reference values**

- An independent reader (the Python prototype) is the oracle: psutop must print
  the same wire currents, statuses and Safeguard+ config.
- Live idle reference from the reference Ai1600TS, 2026-10-15:
  - Cable 1: 0.1875 A ×6.
  - Cable 2: 0.1875 / 0.125 / 0.125 / 0.1875 / 0.1875 / 0.1875 A.
  - Safeguard+ ON, 12.0 A / 5.5 A / 20 s / 180 s.
  - Statuses 0 and flags clear.

**Hardware checks**

1. **Read-only proof:** `psutop once --trace-io` shows only `FA 51` once,
   then `51 <reg>` with reg in the allowlist. Optional, with sudo: confirm on
   the wire with `usbmon` that no other OUT reports are sent.
2. **Parity:** `psutop once --json` matches the prototype's values taken
   back-to-back.
3. **Soak:** the TUI runs 10 min at 1 Hz with no NO DATA ticks and under 1%
   CPU (`pidstat` or `top`).
4. **Labels:** load one GPU with work you already run (a render, a training
   step, an inference request), see which cable's watts rise, and set the
   labels (§10). Don't start, stop or reconfigure services just for this.
5. **Recovery:** re-plug the PSU's USB cable, or simulate it by unbinding
   with sudo. psutop shows NO DATA, then recovers by itself.
6. **Coexistence:** a `watch --csv` logger and the TUI run at the same time
   without errors.
7. **Permissions:** after the udev rule, `psutop once` works without sudo.

**Recording.** Record every hardware finding with date and time: firmware text,
mapping, anything that differs from MeltAlarm's facts, and the meaning of `E0`
words 16 and 17 if found. Never record serial numbers.

## 13. Code layout (proposed)

```text
psutop/
  Cargo.toml           git deps on meltalarm-{model,core,source-api,source-msi,source-sim}
  src/main.rs          clap entry, mode dispatch
  src/source.rs        wraps MsiDriver/SimDriver; flock; --trace-io; rail fields (§3.2)
  src/history.rs       ring buffer, peaks
  src/ui/{mod,cable,header,history,raw,help}.rs
  src/output.rs        once/watch text, JSON, CSV
  src/ui/theme.rs      §7.3 palette, truecolor/256/16/NO_COLOR fallbacks
  src/ui/cutbar.rs     §7.4 cut-bar widget on core::view::bar_share
  tests/fixtures/      live frames from 2026-10-15
  tests/snapshots/     insta snapshots of every layout × sim scenario
  docs/reference/      MeltAlarm flyout.png, floating-compact.png, caution-strip.png (+ MIT notice)
  third_party/meltalarm/          MeltAlarm's model, core, source-api, source-msi (copied) + LICENSE
  SPEC.md  README.md  LICENSE
```

## 14. Testing

- **Unit (Mac and server):** the copied MeltAlarm crates' own tests run as
  workspace members.
- **psutop tests:**
  - Rail-field decoding on the 2026-10-15 live frames.
  - JSON/CSV output shape.
- **UI:** ratatui `TestBackend` snapshot tests (`insta`) for wide, stacked and
  compact layouts in each `--sim` scenario.
- **Hardware:** the §12 checks, run on a machine with the PSU before each release.

**Look-and-feel acceptance** (§7), checked by the user before M2 and M3 are
done:

1. Run `psutop --sim normal` in a truecolor terminal of at least 120 × 40,
   side by side with `docs/reference/flyout.png`. The card must show the
   same title, `● OK` pill, `Current per wire, A`, six cut bars whose
   heights follow the knee scale, the values `8.1 7.8 7.7 7.8 8.0 8.1`, a
   divider, and `Imbalance 0.4 A` / `PSU status Normal`.
2. `psutop --sim caution` against `docs/reference/floating-compact.png`: an
   amber `● Caution` pill, only the 9.9 A bar and value in amber with its
   fill above the cut, the amber line `9.9 A · over rating`, and the caution
   strip under the cards.
3. Next to `nvtop` over `ssh -t`: the same full-screen feel,
   header meters, framed chart, function-key bar in reverse video, an
   immediate first frame, and steady 1 Hz updates without flicker.
4. Resize through 120 → 90 → 70 → 50 columns: wide, stacked, compact, then
   one line per cable (§7.6), with no broken borders.
5. `NO_COLOR=1 psutop --sim caution`: the state is still readable from the
   pill text and `▓` bars alone.
6. `psutop watch` on a TTY shows the card-style lines (§7.7). Piped to a
   file, it has no escape codes.

## 15. Milestones

| # | Deliverable |
|---|---|
| M0 | Repo created; a hello-world using a `meltalarm-model` git dependency builds and runs on the target machine |
| M1 | `once`/`info`/`watch` on `MsiDriver`; flock; `--trace-io`; decide runtime use and rail-field access (§3.2); §12 checks 1–2 |
| M2 | Theme + cut-bar widget + wide layout on `--sim` (cards, nvtop header, function bar, core levels/texts); look-and-feel checks 1–2 (§14) |
| M3 | History chart, raw pane, stacked/compact/one-line layouts, keys, CSV log, `watch` card style; look-and-feel checks 3–6 |
| M4 | Config labels, udev rule; all §12 checks pass |
| v2 | Optional collector service, Prometheus `/metrics`, Homepage widget, notifications; upstream PRs to MeltAlarm |

## 16. Open questions

1. **Toolchain:** user-local rustup, or builds in a container (§4)? *Resolved:
   user-local rustup on the reference machine.*
2. **Install path:** `~/.local/bin` or `/usr/local/bin`? *Either; see `docs/SETUP.md`.*
3. **Rail fields:** upstream PR or vendored `source-msi` (§3.2)? *Vendored for now
   (2026-10-15); upstream patch drafted, not sent.*
4. **Unverified facts:** `E0` words 16/17,
   whether `E0` values are instantaneous, averaged or held peaks, and the
   wire index → pin mapping.
