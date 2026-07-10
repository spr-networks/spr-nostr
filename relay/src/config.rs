//! Plugin configuration: load / validate / persist.
//!
//! Config lives at `/configs/spr-nostr/config.json` (mounted from
//! `configs/plugins/spr-nostr`). It is loaded on start, validated, and written
//! atomically (tmp + rename, mode 0600). The relay has no secret material, so
//! there is nothing to redact on read — every field is safe to echo back to the
//! UI. (If a NIP-42 admin key or similar is ever added, redact it here.)

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Default port the relay listens on (on the container IP of the spr-nostr
/// bridge). 7777 is a common default for standalone Nostr relays.
pub const DEFAULT_PORT: u16 = 7777;

/// Relay access mode.
///
/// Implemented with real nostr-relay-builder policy plugins (there is no
/// native read/write toggle in 0.44.1):
///   * `Read`  — a `WritePolicy` rejects every incoming EVENT (read-only).
///   * `Write` — a `QueryPolicy` rejects every incoming REQ (write-only).
///   * `Both`  — no restriction policies are attached.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "both")]
    #[default]
    Both,
}

/// Persisted plugin configuration.
///
/// Field names are PascalCase to match the convention used by the SPR plugin
/// UIs (the same JSON the React frontend reads/writes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// TCP port the relay listens on (container IP, spr-nostr bridge).
    #[serde(rename = "Port")]
    pub port: u16,
    /// Access mode (read / write / both).
    #[serde(rename = "Mode", default)]
    pub mode: Mode,
    /// Require NIP-42 client authentication (for both reads and writes).
    /// Maps to `RelayBuilder::nip42(RelayBuilderNip42 { mode: Both })`.
    #[serde(rename = "RequireAuth", default)]
    pub require_auth: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: DEFAULT_PORT,
            mode: Mode::default(),
            require_auth: false,
        }
    }
}

impl Config {
    /// Validate all user-controllable fields. `Mode` is a closed enum and
    /// `require_auth` is a bool, so only the port needs range checking.
    pub fn validate(&self) -> Result<(), String> {
        if self.port < 1 {
            return Err("Port must be between 1 and 65535".to_string());
        }
        Ok(())
    }

    /// Load config from `path`. A missing file yields the defaults (and the
    /// caller is expected to persist them on first boot).
    pub fn load(path: &Path) -> Result<Config, String> {
        match fs::read(path) {
            Ok(bytes) => {
                let cfg: Config = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("parsing {}: {e}", path.display()))?;
                cfg.validate()?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("reading {}: {e}", path.display())),
        }
    }

    /// Persist config atomically (tmp + rename) with mode 0600.
    pub fn save_atomic(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| format!("opening {}: {e}", tmp.display()))?;
            f.write_all(&data).map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp, path).map_err(|e| format!("renaming into {}: {e}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        let c = Config::default();
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.mode, Mode::Both);
        assert!(!c.require_auth);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn rejects_port_zero() {
        let c = Config {
            port: 0,
            mode: Mode::Both,
            require_auth: false,
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn json_round_trip_pascal_case_and_mode_strings() {
        let c = Config {
            port: 8000,
            mode: Mode::Read,
            require_auth: true,
        };
        let s = serde_json::to_string(&c).unwrap();
        // PascalCase field names + lowercase mode string are the wire contract
        // the React UI depends on.
        assert!(s.contains("\"Port\":8000"), "{s}");
        assert!(s.contains("\"Mode\":\"read\""), "{s}");
        assert!(s.contains("\"RequireAuth\":true"), "{s}");
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn parse_tolerates_missing_optional_fields() {
        // Mode/RequireAuth default when absent so older config files still load.
        let c: Config = serde_json::from_str("{\"Port\":7000}").unwrap();
        assert_eq!(c.port, 7000);
        assert_eq!(c.mode, Mode::Both);
        assert!(!c.require_auth);
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let p = std::env::temp_dir().join("spr-nostr-does-not-exist-xyz.json");
        let _ = fs::remove_file(&p);
        let c = Config::load(&p).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn save_then_load_round_trips_and_is_0600() {
        let dir = std::env::temp_dir().join(format!("spr-nostr-cfg-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("config.json");
        let c = Config {
            port: 9001,
            mode: Mode::Write,
            require_auth: false,
        };
        c.save_atomic(&p).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config file must be private");
        let back = Config::load(&p).unwrap();
        assert_eq!(back, c);
        let _ = fs::remove_dir_all(&dir);
    }
}
