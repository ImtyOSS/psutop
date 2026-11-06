# Security

## What psutop touches

| Resource | Access |
|---|---|
| The PSU's USB HID node (`/dev/hidrawN`, `0db0:808c` or `0db0:aa6f`) | Opened read-write, because the PSU answers requests. Only MSI's connect handshake and an allowlist of eight read registers are ever sent. |
| `/run/lock/msi-psu.lock` | Created if missing and locked with `flock`; never written. |
| `~/.config/psutop/config.toml` | Read at start; never written. |
| `~/.local/state/psutop/*.csv` | Written only while CSV logging is on (`F9`). |
| Network | None. psutop opens no sockets and sends no telemetry. |
| Other programs | None. psutop starts no child processes and loads no GPU or vendor libraries. |

It runs with `sudo`, or as a normal user at the machine itself after the one-time udev
rule in [docs/SETUP.md](docs/SETUP.md). Under `sudo` it reads the invoking user's config
file, not root's.

## The read-only contract

The PSU code comes from [MeltAlarm](https://github.com/DeniDoman/meltalarm) and is read-only
by construction:

- Requests come from a closed enum; one function builds every packet.
- Exactly one call site writes to the device, marked with the only
  `#[allow(clippy::disallowed_methods)]` in the crate. A clippy lint denies every other
  HID write method.
- A test checks that exactly nine packets can exist: eight register reads and the
  handshake.

The packet observer psutop adds for `--trace-io` receives borrowed bytes after a packet
is written and cannot change what is sent. `psutop once --trace-io` lets you check
what goes to the device yourself.

## The device node is not read-only

psutop only reads, but anyone who can open the PSU's device node can send it any
command, including writes that change or switch off its Safeguard+ protection. So the
included udev rule grants access narrowly: only through the `uaccess` tag, to whoever is
logged in at the machine's own screen and keyboard, while that session lasts (the same way
local users get webcams and USB drives). Everyone else uses `sudo`.

If you give the device to a group instead (the rule file shows how), treat membership like
trust to change the PSU's settings, and avoid broad groups such as `plugdev` on machines
with several users.

## Hardening in psutop

- **Shared lock.** The lock file lives in world-writable `/run/lock`. It is opened with
  `O_NOFOLLOW`, never written, and if another process holds it for more than a second
  psutop polls without it. Another local user can therefore not stop the monitor by
  holding the lock.
- **Display.** Labels from the config file have control characters replaced before they
  reach the terminal; plain-text output escapes quoted values.
- **Traces.** `--trace-io` withholds the reply that contains the PSU's serial number.
  Traces still contain readings and timestamps; review them before sharing.
- **Dependencies.** Versions are pinned in `Cargo.lock`. The MeltAlarm code is copied into
  the repository, so a build fetches nothing from MeltAlarm; the copies and what changed
  are documented in [third_party/meltalarm](third_party/meltalarm). psutop itself forbids `unsafe` code.

## Reporting a problem

Please report security issues privately, through this repository's
**Security → Report a vulnerability** page, rather than in a public issue. Include the
psutop version (`psutop --version`), your PSU model, and how to reproduce the
problem. Don't include your PSU's serial number.
