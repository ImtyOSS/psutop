# Setting up psutop

This guide takes a Linux machine with an MSI MPG Ai1600TS or Ai1300TS from nothing to a
running monitor that needs no `sudo`. It takes about ten minutes. Steps marked **(root)**
need `sudo` once; everything else runs as your normal user.

## 1. Check the hardware

The PSU talks over USB: its USB cable must be connected to a USB 2.0 header on the
motherboard. Check that Linux sees it:

```bash
lsusb | grep -i 0db0
```

You should see `0db0:808c` (Ai1600TS) or `0db0:aa6f` (Ai1300TS). If nothing shows up, check
the cable first; no MSI software is needed.

psutop reads only the PSU, so it doesn't matter which GPUs you have (NVIDIA, AMD or Intel)
or whether their drivers are installed.

## 2. Install the build tools (root)

psutop is built from source. It needs a C toolchain, `pkg-config`, `git`, `curl` and
the libudev development files (for USB device discovery).

Debian, Ubuntu:

```bash
sudo apt install build-essential pkg-config libudev-dev git curl
```

Fedora:

```bash
sudo dnf install gcc pkgconf-pkg-config systemd-devel git curl
```

Arch Linux (libudev comes with systemd):

```bash
sudo pacman -S --needed base-devel pkgconf systemd git curl
```

Check that libudev is found; this prints a version number:

```bash
pkg-config --modversion libudev
```

## 3. Install Rust (your user only)

psutop needs Rust 1.89 or newer. If `cargo --version` already shows that, skip this
step. Otherwise install rustup, which keeps everything under `~/.rustup` and `~/.cargo`:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
```

Open a new shell, or run `source ~/.cargo/env`, so `cargo` is on your `PATH`.

## 4. Build

```bash
git clone <this repository's URL> psutop
cd psutop
cargo build --release --locked
```

Optionally run the tests. On Linux, `--workspace` also runs the tests of the MeltAlarm
crates copied into psutop, including the read-only allowlist test:

```bash
cargo test --workspace --locked
```

## 5. Install the binary

For your user:

```bash
install -Dm755 target/release/psutop ~/.local/bin/psutop
```

`~/.local/bin` is on the `PATH` of most distributions; if `psutop --version` is not
found, add `export PATH="$HOME/.local/bin:$PATH"` to your shell profile.

Or for every user **(root)**:

```bash
sudo install -m 0755 target/release/psutop /usr/local/bin/psutop
```

## 6. Access to the PSU

Linux lets only root open the PSU's USB device, so there are two ways to run psutop:

- **With `sudo`:** `sudo psutop` works with no setup at all. Use this over SSH or on a
  headless machine. psutop still reads your own `~/.config/psutop/config.toml`.
- **Without `sudo`, at the machine itself (root, once):** install the udev rule. Whoever is
  logged in on the machine's own screen and keyboard can then open the PSU, the same way
  webcams and USB drives work. No group, no re-login.

```bash
sudo install -m 0644 packaging/71-msi-psu.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=hidraw
```

The rule doesn't cover SSH sessions, because there is no local session for them. If you
want SSH use without `sudo`, the rule file describes an optional group. Be careful with it:
anyone who can open the device can send the PSU **any** command, including ones that switch
off its own Safeguard+ protection. psutop only ever reads, but the device can't tell programs
apart, so add only users you'd trust with the PSU's settings, and avoid broad groups such as
`plugdev` on shared machines.

Check the setup with:

```bash
psutop doctor
```

It checks each step (PSU on USB, device node, rule, access, lock, and whether the PSU
answers) and prints the fix for anything that's missing.

## 7. First run

```bash
psutop info
```

This prints the device node, the PSU model and firmware, its Safeguard+ settings, and the
cable labels. Then:

```bash
psutop once
```

```bash
psutop
```

The last one is the full-screen monitor; press `?` for help and `q` to quit. Over SSH, use
`ssh -t yourmachine psutop`. For full colour, use a terminal that sets
`COLORTERM=truecolor`; psutop falls back to 256 or 16 colours, or to shapes only with
`--no-color` or `NO_COLOR=1`.

## 8. Label your cables

The PSU only knows "cable 1" and "cable 2"; psutop labels them `GPU 0` and `GPU 1`. To see
which GPU is on which cable, load **one** GPU with work you already have (a game, a render,
a training step) and watch which cable's watts rise in the monitor. Then name them in
`~/.config/psutop/config.toml`:

```toml
[cables.1]
label = "RTX 4090"

[cables.2]
label = "RX 7900 XTX"
```

Idle readings can't tell the cables apart: both sit at the sensors' floor.

## 9. Logging

Press `F9` in the monitor to append to `~/.local/state/psutop/psutop-YYYYMMDD.csv`,
or run a logger on its own:

```bash
psutop watch --csv >> ~/psu-cables.csv
```

Several psutop instances can run at once: each poll takes a shared lock on
`/run/lock/msi-psu.lock`. Other scripts that talk to the PSU should take the same lock
(`flock`).

## Troubleshooting

| Symptom | What to do |
|---|---|
| `PSU not found` (exit code 2) | `lsusb \| grep -i 0db0` shows nothing: reseat the PSU's USB cable on a motherboard USB 2.0 header. `psutop doctor` checks this too. |
| `permission denied` (exit code 3) | Use `sudo psutop`, or install the udev rule (step 6) when you're at the machine itself. `psutop doctor` says which applies. |
| `PSU not answering` (exit code 4), or `No data` | Another program may be talking to the PSU without the shared lock. Unplug and replug the PSU's USB cable; psutop reconnects by itself. |
| Garbled colours or boxes | Use a UTF-8 terminal font with box-drawing characters; try `--no-color`. |
| Layout looks cramped | The monitor adapts to the window. 120 × 40 shows everything side by side; under 60 columns it shows one line per cable. |

Use `psutop once --trace-io 2> trace.log` to see every packet sent to and received from
the PSU. The reply that carries the PSU's serial number is withheld from the trace.
Readings are still in it, so review a trace before sharing it.

## Uninstall

```bash
rm -f ~/.local/bin/psutop
rm -rf ~/.config/psutop ~/.local/state/psutop
```

**(root)**, if you installed these:

```bash
sudo rm -f /usr/local/bin/psutop /etc/udev/rules.d/71-msi-psu.rules
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw
```
