//! `~/.config/psutop/config.toml`: a label per cable (what it powers; "GPU 0" and "GPU 1"
//! by default) and optional meltalarm-core limit overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use meltalarm_core::LimitOverrides;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Keyed by cable number as text: `[cables.1]`, `[cables.2]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cables: BTreeMap<String, CableConfig>,
    #[serde(default, skip_serializing_if = "LimitsConfig::is_empty")]
    pub limits: LimitsConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CableConfig {
    /// Shown on the cable's card and in the chart, e.g. "GPU 0" or "RTX 4090".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Accepted and ignored: written by earlier versions that read GPU stats.
    #[serde(default, skip_serializing)]
    gpu_uuid: Option<toml::Value>,
}

/// meltalarm-core `LimitOverrides`; unset values keep `Limits::V1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarm_seconds: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instant: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uneven: Option<f32>,
}

impl LimitsConfig {
    pub fn is_empty(&self) -> bool {
        self.overrides().is_empty()
    }

    pub fn overrides(&self) -> LimitOverrides {
        LimitOverrides {
            rating: self.rating,
            alarm: self.alarm,
            alarm_seconds: self.alarm_seconds,
            fast: self.fast,
            instant: self.instant,
            uneven: self.uneven,
        }
    }
}

impl Config {
    pub fn cable(&self, index: u8) -> Option<&CableConfig> {
        self.cables.get(&(index + 1).to_string())
    }

    /// The label of cable `index` (0-based): the configured one, or `GPU <index>`.
    pub fn label(&self, index: u8) -> String {
        self.cable(index).and_then(|c| c.label.clone()).unwrap_or_else(|| format!("GPU {index}"))
    }

    /// Labels are shown on screen and in logs, so control characters are replaced.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut c: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        for cable in c.cables.values_mut() {
            cable.label = cable.label.as_deref().map(printable);
        }
        Ok(c)
    }

    /// A missing file is the default config; a malformed one is an error.
    pub fn load(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// `s` with every control character (escape sequences included) replaced by `?`, for text
/// that comes from files or other programs and ends up on a terminal.
pub fn printable(s: &str) -> String {
    s.chars().map(|ch| if ch.is_control() { '?' } else { ch }).collect()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(fallback)))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.config/psutop/config.toml`. Under `sudo`, the invoking user's file: their cable labels
/// still apply, and nothing is read from or written to root's home.
pub fn default_path() -> PathBuf {
    let sudo_home = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root")
        .and_then(|u| std::fs::read_to_string("/etc/passwd").ok().and_then(|p| home_of(&p, &u)));
    match sudo_home {
        Some(h) => h.join(".config/psutop/config.toml"),
        None => xdg("XDG_CONFIG_HOME", ".config").join("psutop/config.toml"),
    }
}

/// The home directory of `user` in `/etc/passwd` text.
pub fn home_of(passwd: &str, user: &str) -> Option<PathBuf> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 5 && f[0] == user && !f[5].is_empty()).then(|| PathBuf::from(f[5]))
    })
}

/// Where `F9` writes its CSV log.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("psutop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_limits_and_ignored_gpu_uuids() {
        let text = r#"
[cables.1]
gpu_uuid = "GPU-aaaa"
label = "RTX PRO 6000"

[cables.2]
label = "RTX 5090"

[limits]
rating = 9.5
"#;
        let c = Config::parse(text).unwrap();
        assert_eq!((c.label(0).as_str(), c.label(1).as_str()), ("RTX PRO 6000", "RTX 5090"));
        assert_eq!(c.limits.overrides().rating, Some(9.5));
        assert_eq!((Config::default().label(0).as_str(), Config::default().label(1).as_str()), ("GPU 0", "GPU 1"));
    }

    #[test]
    fn home_of_finds_the_invoking_user() {
        let passwd = "root:x:0:0::/root:/bin/sh\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n";
        assert_eq!(home_of(passwd, "alice"), Some(PathBuf::from("/home/alice")));
        assert_eq!(home_of(passwd, "bob"), None);
    }

    #[test]
    fn labels_lose_control_characters() {
        let c = Config::parse("[cables.1]\nlabel = \"RTX\\u001b[2J 5090\"\n").unwrap();
        assert_eq!(c.cable(0).unwrap().label.as_deref(), Some("RTX?[2J 5090"));
    }

    #[test]
    fn empty_and_wrong_files() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert!(Config::parse("[limits]\nratting = 9").is_err());
        assert!(Config::default().limits.is_empty());
    }
}
