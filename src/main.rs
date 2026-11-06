//! psutop: live per-wire current of the 12V-2x6 GPU power cables (SPEC.md).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

use psutop::config::{self, Config};
use psutop::monitor::Monitor;
use psutop::output;
use psutop::sim::{self, SimFeed};
use psutop::source::{self, Discovered, Feed, FeedEvent, LOCK_PATH, Sample, USB_IDS};
use psutop::ui::{self, App, theme};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " · built on MeltAlarm 0.3.9 (7763086), MIT © 2026 Denis Domanskii");
const MIN_INTERVAL: Duration = Duration::from_millis(500);
/// The alarm logic treats samples more than 3 s apart as a break and data older than 2.5 s as
/// stale, so a slower poll would delay or lose overload alarms.
const MAX_INTERVAL: Duration = Duration::from_secs(2);

const EXIT_NOT_FOUND: u8 = 2;
const EXIT_DENIED: u8 = 3;
const EXIT_NOT_ANSWERING: u8 = 4;

#[derive(Parser)]
#[command(name = "psutop", version = VERSION, about = "Live per-wire current of the 12V-2x6 GPU power cables on MSI MPG Ai1600TS / Ai1300TS PSUs")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Config file [default: ~/.config/psutop/config.toml]
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Poll interval, e.g. 1s or 500ms (500ms to 2s)
    #[arg(long, global = true, value_parser = parse_duration, default_value = "1s")]
    interval: Duration,
    /// Show raw PSU frames (TUI: start in the raw pane)
    #[arg(long, global = true)]
    raw: bool,
    /// No colour; state by text and shape only (also NO_COLOR)
    #[arg(long, global = true)]
    no_color: bool,
    /// Round pill caps from a Nerd Font
    #[arg(long, global = true)]
    nerd_font: bool,
    /// Log every packet sent to and received from the PSU on stderr
    #[arg(long, global = true)]
    trace_io: bool,
    /// Ring the terminal bell when an alarm starts (TUI)
    #[arg(long, global = true)]
    bell: bool,
    /// Use simulated data instead of the PSU; `--sim list` shows the scenarios
    #[arg(long, global = true, value_name = "SCENARIO", value_parser = parse_scenario)]
    sim: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One sample, then exit
    Once {
        #[arg(long)]
        json: bool,
    },
    /// Stream samples
    Watch {
        /// Redraw in place instead of scrolling (TTY)
        #[arg(long)]
        live: bool,
        #[arg(long, conflicts_with = "json")]
        csv: bool,
        #[arg(long)]
        json: bool,
        /// Stop after N samples
        #[arg(long, value_name = "N")]
        count: Option<u64>,
    },
    /// Identity, firmware, Safeguard+ config, device node
    Info,
    /// Check the setup (USB, device node, udev rule, group, access, lock) and print fixes
    Doctor,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => s.split_at(i),
        None => (s, "s"),
    };
    let v: f64 = num.trim().parse().map_err(|_| format!("not a duration: {s}"))?;
    let secs = match unit {
        "ms" => v / 1000.0,
        "s" => v,
        "m" | "min" => v * 60.0,
        _ => return Err(format!("unknown unit in {s}: use ms, s or m")),
    };
    if !(secs.is_finite() && secs > 0.0) {
        return Err(format!("not a positive duration: {s}"));
    }
    Duration::try_from_secs_f64(secs).map_err(|_| format!("too long: {s}"))
}

/// The interval psutop polls at, and a note when the asked-for one was out of range.
fn clamp_interval(asked: Duration) -> (Duration, Option<&'static str>) {
    if asked < MIN_INTERVAL {
        (MIN_INTERVAL, Some("interval raised to the 500 ms minimum"))
    } else if asked > MAX_INTERVAL {
        (MAX_INTERVAL, Some("interval lowered to the 2 s maximum: slower polling would delay or miss overload alarms"))
    } else {
        (asked, None)
    }
}

fn parse_scenario(s: &str) -> Result<String, String> {
    if sim::is_scenario(s) || s == "list" {
        Ok(s.to_owned())
    } else {
        let names: Vec<&str> = sim::SCENARIOS.iter().map(|(n, _)| *n).collect();
        Err(format!("unknown scenario; one of: {}", names.join(", ")))
    }
}

fn wall() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// The wall-clock time of `at`, a moment that may have passed while a sample waited in the queue.
fn local_at(at: Instant) -> chrono::DateTime<chrono::Local> {
    let age = chrono::Duration::from_std(Instant::now().saturating_duration_since(at)).unwrap_or_default();
    chrono::Local::now() - age
}

struct Ctx {
    cli: Cli,
    config: Config,
    config_path: PathBuf,
    interval: Duration,
}

impl Ctx {
    fn feed(&self) -> Result<Box<dyn Feed>, String> {
        if let Some(s) = &self.cli.sim {
            return Ok(Box::new(SimFeed::new(s)));
        }
        #[cfg(target_os = "linux")]
        return Ok(Box::new(source::msi::MsiFeed::new(self.cli.trace_io)));
        #[cfg(not(target_os = "linux"))]
        Err("psutop reads the PSU on Linux only. Here it runs the simulator: add --sim <scenario> (--sim list).".into())
    }

    fn monitor(&self) -> Monitor {
        Monitor::new(self.config.clone(), self.interval, self.cli.sim.is_some(), wall, Instant::now())
    }

    fn theme(&self) -> theme::Theme {
        theme::Theme::new(theme::Depth::detect(self.cli.no_color), self.cli.nerd_font)
    }
}

fn fail(code: u8, msg: &str) -> ExitCode {
    eprintln!("psutop: {msg}");
    ExitCode::from(code)
}

/// Connect for the one-shot modes: exit codes 2/3/4 (SPEC §6).
fn connect(feed: &mut dyn Feed) -> Result<meltalarm_model::SourceInfo, ExitCode> {
    let mut last = String::new();
    for attempt in 0..3 {
        match feed.discover() {
            Discovered::Found(info) => return Ok(info),
            Discovered::Absent => {
                return Err(fail(EXIT_NOT_FOUND, &format!("PSU not found: no MSI MPG Ai1600TS / Ai1300TS on USB ({USB_IDS})")));
            }
            Discovered::Denied(msg) => {
                return Err(fail(EXIT_DENIED, &format!("permission denied: {msg}\n  {}", source::permission_hint())));
            }
            Discovered::Unusable(msg) => return Err(fail(EXIT_NOT_ANSWERING, &msg)),
            Discovered::NotReady(msg) => last = msg,
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Err(fail(EXIT_NOT_ANSWERING, &last))
}

/// One healthy sample if the PSU gives one within three tries.
fn sample(feed: &mut dyn Feed, caps: &meltalarm_model::Capabilities) -> (Sample, bool) {
    let mut s = feed.poll();
    for _ in 0..2 {
        if s.report.is_healthy(caps) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        s = feed.poll();
    }
    let healthy = s.report.is_healthy(caps);
    (s, healthy)
}

/// `--trace-io` prints every packet on stderr (SPEC §12 check 1). In the full-screen view that
/// would garble the screen, so stderr has to be redirected there.
fn trace_io(ctx: &Ctx, tui: bool) -> Result<(), ExitCode> {
    if !ctx.cli.trace_io {
        return Ok(());
    }
    if ctx.cli.sim.is_some() {
        eprintln!("psutop: trace-io: simulator, no device I/O");
        return Ok(());
    }
    if tui && std::io::stderr().is_terminal() {
        return Err(fail(
            1,
            "--trace-io writes every packet to stderr: redirect it (psutop --trace-io 2> trace.log) or use `once` / `watch`",
        ));
    }
    Ok(())
}

fn once(ctx: &Ctx, json: bool) -> ExitCode {
    if let Err(code) = trace_io(ctx, false) {
        return code;
    }
    let mut feed = match ctx.feed() {
        Ok(f) => f,
        Err(e) => return fail(1, &e),
    };
    let info = match connect(feed.as_mut()) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let mut m = ctx.monitor();
    let caps = info.caps;
    m.handle(FeedEvent::Connected(info), Instant::now());
    let (s, healthy) = sample(feed.as_mut(), &caps);
    let now = Instant::now();
    m.handle(FeedEvent::Sample(Box::new(s)), now);
    let snap = m.snapshot(now);
    let mut out = std::io::stdout().lock();
    if json {
        let _ = writeln!(out, "{}", serde_json::to_string_pretty(&output::json(&snap, &iso(), ctx.cli.raw)).unwrap_or_default());
    } else {
        let lines = if std::io::stdout().is_terminal() {
            output::card_block(&snap, &ctx.theme(), &chrono::Local::now().format("%H:%M:%S").to_string())
        } else {
            output::plain(&snap, &wall())
        };
        for l in lines {
            let _ = writeln!(out, "{l}");
        }
    }
    // With --json the raw frames are inside the JSON.
    if ctx.cli.raw && !json {
        for l in output::raw_lines(&snap, &wall()) {
            let _ = writeln!(out, "{l}");
        }
    }
    if healthy { ExitCode::SUCCESS } else { fail(EXIT_NOT_ANSWERING, "the PSU connected but sent no valid reading") }
}

fn watch(ctx: &Ctx, live: bool, csv: bool, json: bool, count: Option<u64>) -> ExitCode {
    if let Err(code) = trace_io(ctx, false) {
        return code;
    }
    let feed = match ctx.feed() {
        Ok(f) => f,
        Err(e) => return fail(1, &e),
    };
    let acq = source::spawn(feed, ctx.interval);
    let mut m = ctx.monitor();
    let tty = std::io::stdout().is_terminal();
    let theme = ctx.theme();
    let started = Instant::now();
    let mut connected = false;
    let mut printed = 0u64;
    let mut out = std::io::stdout().lock();
    // The newest moment handled: a sample counts at the time it was read, so one that waited
    // behind a stalled reader is printed with that time, never as fresh.
    let mut now = Instant::now();
    if csv && writeln!(out, "{}", output::csv_header()).is_err() {
        return ExitCode::SUCCESS;
    }
    for ev in acq.rx.iter() {
        let is_sample = matches!(ev, FeedEvent::Sample(_));
        if let FeedEvent::Searching(d) = &ev
            && !connected
        {
            match d {
                Discovered::Absent => return fail(EXIT_NOT_FOUND, &format!("PSU not found ({USB_IDS})")),
                Discovered::Denied(msg) => {
                    return fail(EXIT_DENIED, &format!("permission denied: {msg}\n  {}", source::permission_hint()));
                }
                Discovered::NotReady(msg) if started.elapsed() > Duration::from_secs(10) => return fail(EXIT_NOT_ANSWERING, msg),
                _ => {}
            }
        }
        connected |= matches!(ev, FeedEvent::Connected(_));
        now = now.max(match &ev {
            FeedEvent::Sample(s) => s.report.at,
            _ => Instant::now(),
        });
        m.handle(ev, now);
        // Only the full-screen view shows the event log; drop it here so it can't pile up.
        let _ = m.drain_log();
        if !is_sample {
            continue;
        }
        let snap = m.snapshot(now);
        let local = local_at(now);
        let wall_t = local.format("%Y-%m-%d %H:%M:%S").to_string();
        let ok = if json {
            let iso_t = local.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
            writeln!(out, "{}", serde_json::to_string(&output::json(&snap, &iso_t, ctx.cli.raw)).unwrap_or_default())
        } else if csv {
            writeln!(out, "{}", output::csv_sample(&snap, &wall_t))
        } else if tty {
            let block = output::card_block(&snap, &theme, &local.format("%H:%M:%S").to_string());
            // --live redraws from the top of the screen, like watch(1): counting the previous
            // block's lines breaks as soon as a line wraps on a narrow terminal.
            let mut text = if live { String::from("\x1b[H\x1b[J") } else { String::new() };
            for l in &block {
                text.push_str(l);
                text.push('\n');
            }
            write!(out, "{text}")
        } else {
            output::plain(&snap, &wall_t).iter().try_for_each(|l| writeln!(out, "{l}"))
        };
        if ctx.cli.raw && !json && !csv {
            for l in output::raw_lines(&snap, &wall_t) {
                let _ = writeln!(out, "{l}");
            }
        }
        if ok.and_then(|()| out.flush()).is_err() {
            break; // the reader went away
        }
        printed += 1;
        if count.is_some_and(|n| printed >= n) {
            break;
        }
    }
    ExitCode::SUCCESS
}

fn info(ctx: &Ctx) -> ExitCode {
    if let Err(code) = trace_io(ctx, false) {
        return code;
    }
    let mut feed = match ctx.feed() {
        Ok(f) => f,
        Err(e) => return fail(1, &e),
    };
    let node = feed.device_node();
    let info = match connect(feed.as_mut()) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let mut m = ctx.monitor();
    let caps = info.caps;
    m.handle(FeedEvent::Connected(info.clone()), Instant::now());
    let (s, _) = sample(feed.as_mut(), &caps);
    let now = Instant::now();
    m.handle(FeedEvent::Sample(Box::new(s)), now);
    let snap = m.snapshot(now);
    let protection = snap.vm.source.as_ref().map_or("—".to_owned(), |s| s.protection.clone());
    let limits = ctx.config.limits.overrides().resolve();
    println!("Device      {} ({})", node.unwrap_or_else(|| "?".into()), USB_IDS);
    println!("Model       {} {}{}", info.vendor, info.model, if snap.simulated { " (simulated)" } else { "" });
    println!("Firmware    {}", info.firmware.as_deref().unwrap_or("—"));
    println!("Source id   {}", info.id);
    println!("Protection  {protection}");
    println!("Limits      {}", limits.summary());
    if let Some(w) = &limits.warning {
        println!("            {w}");
    }
    for c in &snap.cards {
        println!("Cable {}     {}", c.index + 1, c.gpu.as_deref().unwrap_or("—"));
    }
    let found = if ctx.config_path.exists() { "" } else { " (not found; defaults)" };
    println!("Config      {}{found}", ctx.config_path.display());
    println!("Lock        {LOCK_PATH}");
    ExitCode::SUCCESS
}

fn doctor(ctx: &Ctx) -> ExitCode {
    if ctx.cli.sim.is_some() {
        println!("psutop doctor checks the real setup; with --sim there is nothing to check.");
        return ExitCode::SUCCESS;
    }
    if !cfg!(target_os = "linux") {
        println!("psutop reads the PSU on Linux only; on this system only --sim works.");
        return ExitCode::from(1);
    }
    let depth = theme::Depth::detect(ctx.cli.no_color);
    let env = psutop::doctor::Env::detect(ctx.config_path.clone(), format!("{depth:?}"));
    #[cfg(target_os = "linux")]
    let mut probe = || -> Result<String, String> {
        let mut feed = source::msi::MsiFeed::new(false);
        match feed.discover() {
            Discovered::Found(info) => {
                let s = feed.poll();
                if !s.report.is_healthy(&info.caps) {
                    return Err("connected, but the PSU sent no valid reading".into());
                }
                let sg = s.report.protection.map_or(String::new(), |p| format!(", Safeguard+ {}", if p.enabled { "ON" } else { "OFF" }));
                Ok(format!("{} {}, firmware {}{sg}", info.vendor, info.model, info.firmware.as_deref().unwrap_or("?")))
            }
            Discovered::Absent => Err("not found when psutop looked".into()),
            Discovered::NotReady(m) | Discovered::Denied(m) | Discovered::Unusable(m) => Err(m),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let mut probe = || -> Result<String, String> { Err("not Linux".into()) };
    let checks = psutop::doctor::run(&env, Some(&mut probe));
    let colour = std::io::stdout().is_terminal() && depth != theme::Depth::Mono;
    print!("{}", psutop::doctor::render(&checks, colour));
    if checks.iter().any(|c| c.status == psutop::doctor::Status::Fail) { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

fn tui(ctx: &Ctx) -> ExitCode {
    if let Err(code) = trace_io(ctx, true) {
        return code;
    }
    let feed = match ctx.feed() {
        Ok(f) => f,
        Err(e) => return fail(1, &e),
    };
    let acq = source::spawn(feed, ctx.interval);
    let monitor = ctx.monitor();
    let mut app = App::new(ctx.theme(), ctx.interval);
    app.bell = ctx.cli.bell;
    if ctx.cli.raw {
        app.pane = ui::Pane::Raw;
    }
    let term = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT, signal_hook::consts::SIGHUP] {
        let _ = signal_hook::flag::register(sig, Arc::clone(&term));
    }
    let res = ui::run(monitor, acq, app, term);
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(1, &e.to_string()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.sim.as_deref() == Some("list") {
        for (name, what) in sim::SCENARIOS {
            println!("{name:<10} {what}");
        }
        return ExitCode::SUCCESS;
    }
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => return fail(1, &format!("config: {e}")),
    };
    let (interval, note) = clamp_interval(cli.interval);
    if let Some(note) = note {
        eprintln!("psutop: {note}");
    }
    let ctx = Ctx { config, config_path, interval, cli };
    match &ctx.cli.cmd {
        Some(Cmd::Once { json }) => once(&ctx, *json),
        Some(Cmd::Watch { live, csv, json, count }) => watch(&ctx, *live, *csv, *json, *count),
        Some(Cmd::Info) => info(&ctx),
        Some(Cmd::Doctor) => doctor(&ctx),
        None => {
            let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
            if dumb || !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
                eprintln!("psutop: no interactive terminal; printing one sample (use `psutop watch` to stream)");
                once(&ctx, false)
            } else {
                tui(&ctx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1s"), Ok(Duration::from_secs(1)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("2"), Ok(Duration::from_secs(2)));
        assert_eq!(parse_duration("1m"), Ok(Duration::from_secs(60)));
        assert!(parse_duration("0s").is_err() && parse_duration("fast").is_err() && parse_duration("3h").is_err());
        // Too big for a Duration: an error, not a panic.
        assert!(parse_duration("99999999999999999999999s").is_err());
    }

    #[test]
    fn the_interval_stays_fast_enough_for_the_alarms() {
        assert_eq!(clamp_interval(Duration::from_millis(100)).0, MIN_INTERVAL);
        assert_eq!(clamp_interval(Duration::from_secs(1)), (Duration::from_secs(1), None));
        assert_eq!(clamp_interval(Duration::from_secs(2)).0, Duration::from_secs(2));
        assert_eq!(clamp_interval(Duration::from_secs(4)).0, MAX_INTERVAL);
        assert_eq!(clamp_interval(Duration::from_secs(60)).0, MAX_INTERVAL);
    }

    #[test]
    fn link_exit_codes_are_distinct() {
        assert_ne!(EXIT_NOT_FOUND, EXIT_DENIED);
        assert_eq!(psutop::output::link_name(&psutop::monitor::Link::NotFound), "not_found");
    }
}
