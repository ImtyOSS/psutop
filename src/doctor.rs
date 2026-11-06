//! `psutop doctor`: checks the setup step by step and prints the fix for anything that is
//! missing (SPEC §6, §11.1). It reads sysfs, `/dev`, `/etc` and `/proc` only; the optional
//! "PSU answers" check goes through the normal read-only path.
//!
//! Every file is read under [`Env::root`], so tests run against a fake tree.

use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::source::{LOCK_PATH, find_hidraw_in};

const MODELS: [(&str, &str); 2] = [("808c", "MSI MPG Ai1600TS"), ("aa6f", "MSI MPG Ai1300TS")];
const RULE_DIRS: [&str; 3] = ["etc/udev/rules.d", "usr/lib/udev/rules.d", "lib/udev/rules.d"];
const INSTALL_RULE: &str = "sudo install -m 0644 packaging/71-msi-psu.rules /etc/udev/rules.d/\n\
     sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// Works now, or may, but something is worth fixing.
    Warn,
    Fail,
    /// Not applicable, or only information.
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
}

fn check(name: &'static str, status: Status, detail: impl Into<String>, fix: Option<&str>) -> Check {
    Check { name, status, detail: detail.into(), fix: fix.map(str::to_owned) }
}

/// What the checks look at.
pub struct Env {
    /// `/`, or a fake tree in tests.
    pub root: PathBuf,
    pub config_path: PathBuf,
    /// Running over SSH, where `uaccess` does not apply.
    pub ssh: bool,
    pub utf8: bool,
    pub colour: String,
}

impl Env {
    pub fn detect(config_path: PathBuf, colour: String) -> Env {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let ssh = !var("SSH_CONNECTION").is_empty() || !var("SSH_TTY").is_empty();
        let locale = [var("LC_ALL"), var("LC_CTYPE"), var("LANG")].into_iter().find(|v| !v.is_empty()).unwrap_or_default();
        let utf8 = locale.to_ascii_uppercase().replace('-', "").contains("UTF8");
        Env { root: PathBuf::from("/"), config_path, ssh, utf8, colour }
    }

    fn path(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }

    fn read(&self, abs: &str) -> Option<String> {
        std::fs::read_to_string(self.path(abs)).ok()
    }
}

/// The PSU on USB: model, product id and the sysfs device name (`1-12.4`).
pub fn find_psu_usb(root: &Path) -> Option<(&'static str, &'static str, String)> {
    let mut devs: Vec<PathBuf> = std::fs::read_dir(root.join("sys/bus/usb/devices")).ok()?.flatten().map(|e| e.path()).collect();
    devs.sort();
    devs.into_iter().find_map(|d| {
        let vid = std::fs::read_to_string(d.join("idVendor")).ok()?;
        let pid = std::fs::read_to_string(d.join("idProduct")).ok()?;
        if !vid.trim().eq_ignore_ascii_case("0db0") {
            return None;
        }
        let (id, model) = MODELS.iter().find(|(id, _)| pid.trim().eq_ignore_ascii_case(id))?;
        Some((*model, *id, d.file_name()?.to_string_lossy().into_owned()))
    })
}

/// The name for `id` in `/etc/passwd` or `/etc/group` (both have it in field 3).
pub fn name_for_id(text: &str, id: u32) -> Option<String> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2].parse() == Ok(id)).then(|| f[0].to_owned())
    })
}

/// The real uid from `/proc/self/status`.
pub fn proc_uid(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleFile {
    pub path: PathBuf,
    pub group: Option<String>,
    pub uaccess: bool,
}

/// udev rule files with a rule for vendor `0db0`.
pub fn find_rules(root: &Path) -> Vec<RuleFile> {
    let mut out = Vec::new();
    for dir in RULE_DIRS {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "rules")).collect();
        files.sort();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let lines: Vec<&str> =
                text.lines().map(str::trim).filter(|l| !l.starts_with('#') && l.to_ascii_lowercase().contains("0db0")).collect();
            if lines.is_empty() {
                continue;
            }
            let group = lines.iter().find_map(|l| l.split("GROUP=\"").nth(1)?.split('"').next().map(str::to_owned));
            let uaccess = lines.iter().any(|l| l.contains("uaccess"));
            let shown = Path::new("/").join(path.strip_prefix(root).unwrap_or(&path));
            out.push(RuleFile { path: shown, group, uaccess });
        }
    }
    out
}

/// `crw-rw----`, like `ls -l`.
pub fn mode_string(mode: u32, char_device: bool) -> String {
    let mut s = String::from(if char_device { "c" } else { "-" });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Run every check in order. `probe` talks to the PSU once and describes what answered.
pub fn run(env: &Env, probe: Option<&mut dyn FnMut() -> Result<String, String>>) -> Vec<Check> {
    let mut out = Vec::new();
    let groups_text = env.read("/etc/group").unwrap_or_default();
    let passwd_text = env.read("/etc/passwd").unwrap_or_default();
    let group_name = |gid: u32| name_for_id(&groups_text, gid).unwrap_or_else(|| gid.to_string());

    // 1. On USB.
    let usb = find_psu_usb(&env.root);
    out.push(match &usb {
        Some((model, pid, dev)) => check("PSU on USB", Status::Ok, format!("{model} (0db0:{pid}) at USB {dev}"), None),
        None => check(
            "PSU on USB",
            Status::Fail,
            "no MSI MPG Ai1600TS / Ai1300TS (0db0:808c / 0db0:aa6f) found",
            Some("Connect the PSU's USB cable to a USB 2.0 header on the motherboard; `lsusb | grep -i 0db0` should then list it."),
        ),
    });

    // 2. Device node.
    let node = find_hidraw_in(&env.root);
    let meta = node.as_deref().and_then(|n| std::fs::metadata(env.path(n)).ok());
    out.push(match (&node, &meta) {
        (Some(n), Some(m)) => {
            let owner = name_for_id(&passwd_text, m.uid()).unwrap_or_else(|| m.uid().to_string());
            let mode = mode_string(m.mode(), m.file_type().is_char_device());
            check("Device node", Status::Ok, format!("{n}  {mode} {owner}:{}", group_name(m.gid())), None)
        }
        (Some(n), None) => {
            check("Device node", Status::Fail, format!("{n} is in sysfs but not in /dev"), Some("Re-plug the PSU's USB cable."))
        }
        (None, _) if usb.is_some() => check(
            "Device node",
            Status::Fail,
            "the PSU has no hidraw device node",
            Some("Re-plug the PSU's USB cable; `dmesg | grep -i hid` shows whether the kernel's HID driver took it."),
        ),
        (None, _) => check("Device node", Status::Info, "nothing to look for without the PSU", None),
    });

    // 3. udev rule. Optional: without one, `sudo psutop` works.
    let rules = find_rules(&env.root);
    out.push(match rules.first() {
        None => check(
            "udev rule",
            Status::Info,
            "none installed: run psutop with sudo, or install the rule (below) for use at this machine",
            None,
        ),
        Some(r) => {
            let who = match (&r.group, r.uaccess) {
                (Some(g), true) => format!("the user at this machine (uaccess) and group {g}"),
                (Some(g), false) => format!("group {g}"),
                (None, true) => "the user at this machine (uaccess)".into(),
                (None, false) => "nobody beyond root (no uaccess tag or group)".into(),
            };
            check("udev rule", Status::Ok, format!("{} lets {who} open the PSU", r.path.display()), None)
        }
    });

    // 4. Access.
    let root_user = proc_uid(&env.read("/proc/self/status").unwrap_or_default()) == Some(0);
    let access = node.as_deref().filter(|_| meta.is_some()).map(|n| OpenOptions::new().read(true).write(true).open(env.path(n)));
    let can_open = matches!(access, Some(Ok(_)));
    let denied_fix = if env.ssh {
        "Run `sudo psutop`. Over SSH the rule's uaccess tag doesn't apply; packaging/71-msi-psu.rules \
         describes an optional group for SSH use without sudo."
            .to_owned()
    } else if rules.is_empty() {
        format!("Run `sudo psutop`, or install the rule so whoever is logged in at this machine can open the PSU:\n{INSTALL_RULE}")
    } else {
        "Re-apply the rule (it takes effect for the session at this machine), or use `sudo psutop`:\n\
         sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw"
            .to_owned()
    };
    out.push(match access {
        Some(Ok(_)) if root_user => check("Access", Status::Ok, "psutop can open the PSU (running as root)", None),
        Some(Ok(_)) => check("Access", Status::Ok, "your user can open the PSU", None),
        Some(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            check("Access", Status::Fail, "permission denied", Some(&denied_fix))
        }
        Some(Err(e)) => check("Access", Status::Fail, e.to_string(), Some("Re-plug the PSU's USB cable.")),
        None => check("Access", Status::Info, "nothing to open", None),
    });

    // 6. Lock file.
    let lock = env.path(LOCK_PATH);
    out.push(match std::fs::symlink_metadata(&lock) {
        Ok(m) if m.file_type().is_symlink() => check(
            "Lock file",
            Status::Warn,
            format!("{LOCK_PATH} is a symlink; psutop won't follow it and runs without the lock"),
            Some("sudo rm /run/lock/msi-psu.lock"),
        ),
        Ok(_) => match File::open(&lock).map(|f| (f.try_lock(), f)) {
            Ok((Ok(()), f)) => {
                let _ = f.unlock();
                check("Lock file", Status::Ok, format!("{LOCK_PATH} is free"), None)
            }
            Ok((Err(TryLockError::WouldBlock), _)) => check(
                "Lock file",
                Status::Info,
                format!("{LOCK_PATH} is held by another PSU reader right now; psutop waits up to 1 s, then polls without it"),
                None,
            ),
            _ => check("Lock file", Status::Warn, format!("{LOCK_PATH} can't be opened; psutop runs without the lock"), None),
        },
        Err(_) if env.path("/run/lock").is_dir() => check("Lock file", Status::Ok, format!("{LOCK_PATH} is created on first run"), None),
        Err(_) => check("Lock file", Status::Warn, "/run/lock does not exist; psutop runs without the lock", None),
    });

    // 7. The PSU answers.
    out.push(match probe {
        Some(p) if can_open => match p() {
            Ok(what) => check("PSU answers", Status::Ok, what, None),
            Err(e) => check(
                "PSU answers",
                Status::Fail,
                e,
                Some("Re-plug the PSU's USB cable. Other programs talking to the PSU should take the same lock."),
            ),
        },
        _ => check("PSU answers", Status::Info, "not tried: psutop can't open the PSU yet", None),
    });

    // 8. Config.
    let shown = env.config_path.display();
    out.push(match Config::load(&env.config_path) {
        Ok(c) => {
            let source = if env.config_path.exists() { shown.to_string() } else { "no config file, defaults".to_owned() };
            check("Config", Status::Ok, format!("{source}: cable 1 = {}, cable 2 = {}", c.label(0), c.label(1)), None)
        }
        Err(e) => check("Config", Status::Fail, e, Some("Fix or remove the file; README shows the format.")),
    });

    // 9. Terminal.
    out.push(if env.utf8 {
        check("Terminal", Status::Ok, format!("UTF-8, colour: {}", env.colour), None)
    } else {
        check(
            "Terminal",
            Status::Warn,
            "the locale is not UTF-8: bars and boxes may show as ?",
            Some("Use a UTF-8 locale, e.g. export LANG=C.UTF-8"),
        )
    });
    out
}

/// The report: one line per check, the fix indented under it.
pub fn render(checks: &[Check], colour: bool) -> String {
    let paint = |code: &str, s: &str| if colour { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_owned() };
    let mut out = String::from("psutop doctor\n\n");
    for c in checks {
        let mark = match c.status {
            Status::Ok => paint("32", "✓"),
            Status::Warn => paint("33", "!"),
            Status::Fail => paint("31", "✗"),
            Status::Info => paint("2", "·"),
        };
        out.push_str(&format!("  {mark} {:<14} {}\n", c.name, c.detail));
        for line in c.fix.iter().flat_map(|f| f.lines()) {
            out.push_str(&format!("      {}\n", paint("2", &format!("→ {line}"))));
        }
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    out.push('\n');
    out.push_str(&match (fails, warns) {
        (0, 0) => "All good.\n".to_owned(),
        (0, w) => format!("Works; {w} thing(s) worth fixing above.\n"),
        (f, _) => format!("{f} problem(s): start with the first ✗ above.\n"),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake root with the PSU on USB, its hidraw node, the rule and the group.
    fn fake_root(name: &str, gid: u32) -> PathBuf {
        let root = std::env::temp_dir().join(format!("psutop-doctor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let put = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        put("sys/bus/usb/devices/1-12.4/idVendor", "0db0\n");
        put("sys/bus/usb/devices/1-12.4/idProduct", "808c\n");
        put("sys/bus/usb/devices/1-1/idVendor", "1d6b\n");
        put("sys/bus/usb/devices/1-1/idProduct", "0002\n");
        put("sys/class/hidraw/hidraw11/device/uevent", "DRIVER=hid-generic\nHID_ID=0003:00000DB0:0000808C\n");
        put("dev/hidraw11", "");
        put("etc/passwd", "root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/home/alice:/bin/bash\n");
        put("etc/group", &format!("root:x:0:\nplugdev:x:46:alice\nmsipsu:x:{gid}:alice\n"));
        put("proc/self/status", &format!("Name:\tpsutop\nUid:\t1000\t1000\t1000\t1000\nGroups:\t46 {gid} \n"));
        put("etc/udev/rules.d/71-msi-psu.rules", include_str!("../packaging/71-msi-psu.rules"));
        std::fs::create_dir_all(root.join("run/lock")).unwrap();
        root
    }

    fn env(root: PathBuf, ssh: bool) -> Env {
        Env { root, config_path: PathBuf::from("/nonexistent/psutop.toml"), ssh, utf8: true, colour: "True".into() }
    }

    #[test]
    fn parsers() {
        assert_eq!(name_for_id("root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/h:/b\n", 1000).as_deref(), Some("alice"));
        assert_eq!(proc_uid("Name:\tpsutop\nUid:\t1000\t1000\t1000\t1000\n"), Some(1000));
        assert_eq!(mode_string(0o20660, true), "crw-rw----");
        assert_eq!(mode_string(0o100644, false), "-rw-r--r--");
    }

    #[test]
    fn finds_the_psu_the_rule_and_its_settings() {
        let root = fake_root("find", 996);
        assert_eq!(find_psu_usb(&root), Some(("MSI MPG Ai1600TS", "808c", "1-12.4".into())));
        let rules = find_rules(&root);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path, PathBuf::from("/etc/udev/rules.d/71-msi-psu.rules"));
        assert_eq!((rules[0].group.as_deref(), rules[0].uaccess), (None, true), "the shipped rule is uaccess only");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_complete_setup_has_no_failures() {
        let root = fake_root("complete", 996);
        let mut probe = || Ok::<_, String>("MSI MPG Ai1600TS, firmware 10, Safeguard+ ON".to_owned());
        let checks = run(&env(root.clone(), true), Some(&mut probe));
        let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
        assert_eq!(names, ["PSU on USB", "Device node", "udev rule", "Access", "Lock file", "PSU answers", "Config", "Terminal"]);
        assert!(checks.iter().all(|c| c.status == Status::Ok), "{checks:#?}");
        assert!(checks[2].detail.ends_with("lets the user at this machine (uaccess) open the PSU"), "{:?}", checks[2]);
        assert_eq!(checks[5].detail, "MSI MPG Ai1600TS, firmware 10, Safeguard+ ON");
        assert!(render(&checks, false).ends_with("All good.\n"), "{}", render(&checks, false));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn denied_points_at_sudo_or_the_rule() {
        use std::os::unix::fs::PermissionsExt;
        let root = fake_root("denied", 996);
        std::fs::remove_dir_all(root.join("etc/udev")).unwrap();
        let node = root.join("dev/hidraw11");
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o000)).unwrap();
        if OpenOptions::new().read(true).write(true).open(&node).is_ok() {
            return; // running as root: nothing is denied
        }
        let over_ssh = run(&env(root.clone(), true), None);
        assert_eq!(over_ssh[2].status, Status::Info);
        assert_eq!(over_ssh[3].status, Status::Fail);
        assert!(over_ssh[3].fix.as_deref().unwrap().starts_with("Run `sudo psutop`"));
        let local = run(&env(root.clone(), false), None);
        assert!(local[3].fix.as_deref().unwrap().contains("71-msi-psu.rules /etc/udev/rules.d/"));
        assert!(render(&local, false).contains("start with the first ✗"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn any_working_rule_is_fine_and_no_psu_fails_first() {
        let root = fake_root("plugdev", 996);
        std::fs::write(
            root.join("etc/udev/rules.d/71-msi-psu.rules"),
            "KERNEL==\"hidraw*\", ATTRS{idVendor}==\"0db0\", ATTRS{idProduct}==\"808c\", MODE=\"0660\", GROUP=\"plugdev\"\n",
        )
        .unwrap();
        let checks = run(&env(root.clone(), true), None);
        assert_eq!(checks[2].status, Status::Ok);
        assert!(checks[2].detail.contains("lets group plugdev open"), "{:?}", checks[2]);
        std::fs::remove_dir_all(root.join("sys/bus/usb/devices/1-12.4")).unwrap();
        let checks = run(&env(root.clone(), false), None);
        assert_eq!(checks[0].status, Status::Fail);
        let _ = std::fs::remove_dir_all(root);
    }
}
