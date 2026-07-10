//! NIP-11 Relay Information Document, built from our own config.
//!
//! We construct rust-nostr's `nostr::nips::nip11::RelayInformationDocument`
//! (which is serde-serializable) and serve it ourselves from the HTTP front
//! (see `http.rs`) — nostr-relay-builder 0.44.1 does not serve NIP-11 on its
//! own, so we do it.

use nostr::nips::nip11::RelayInformationDocument;

use crate::config::Config;

/// Software identifier advertised in the NIP-11 document.
pub const SOFTWARE: &str = "https://github.com/spr-networks/spr-nostr";

/// The NIPs this relay actually implements (verified against the
/// nostr-relay-builder 0.44.1 / nostr-lmdb 0.44.1 source):
///   * 1  — basic protocol (EVENT / REQ / CLOSE)
///   * 9  — event deletion (kind-5 handling in the database layer)
///   * 11 — this relay information document (served by our HTTP front)
///   * 45 — COUNT
///   * 77 — negentropy set reconciliation (NEG-OPEN / NEG-MSG / NEG-CLOSE)
///
/// NIP-42 (client authentication) is added only when it is actually required.
fn supported_nips(cfg: &Config) -> Vec<u16> {
    let mut nips = vec![1u16, 9, 11, 45, 77];
    if cfg.require_auth {
        nips.push(42);
    }
    nips.sort_unstable();
    nips
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Build the NIP-11 document from config. Unset text fields are left as `None`.
pub fn build_relay_info(cfg: &Config) -> RelayInformationDocument {
    let mut doc = RelayInformationDocument::new();
    doc.name = non_empty(&cfg.relay_name);
    doc.description = non_empty(&cfg.relay_description);
    doc.pubkey = non_empty(&cfg.relay_pubkey);
    doc.contact = non_empty(&cfg.relay_contact);
    doc.supported_nips = Some(supported_nips(cfg));
    doc.software = Some(SOFTWARE.to_string());
    doc.version = Some(concat!("v", env!("CARGO_PKG_VERSION")).to_string());
    doc
}

/// Serialize the NIP-11 document to the JSON body clients receive.
pub fn build_relay_info_json(cfg: &Config) -> String {
    // Serialization is infallible for this type; fall back to `{}` defensively.
    serde_json::to_string(&build_relay_info(cfg)).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    fn cfg(auth: bool) -> Config {
        Config {
            port: 7777,
            mode: Mode::Both,
            require_auth: auth,
            relay_name: "My Relay".to_string(),
            relay_description: "A relay".to_string(),
            relay_pubkey: String::new(),
            relay_contact: "mailto:me@example.com".to_string(),
        }
    }

    #[test]
    fn nips_include_42_only_when_auth_required() {
        assert!(!supported_nips(&cfg(false)).contains(&42));
        assert!(supported_nips(&cfg(true)).contains(&42));
        // core NIPs always present
        for n in [1u16, 9, 11, 45, 77] {
            assert!(supported_nips(&cfg(false)).contains(&n), "missing {n}");
        }
    }

    #[test]
    fn empty_fields_become_none() {
        let doc = build_relay_info(&cfg(false));
        assert_eq!(doc.name.as_deref(), Some("My Relay"));
        assert_eq!(
            doc.pubkey, None,
            "empty pubkey must be None, not Some(\"\")"
        );
        assert_eq!(doc.contact.as_deref(), Some("mailto:me@example.com"));
        assert_eq!(doc.software.as_deref(), Some(SOFTWARE));
        assert!(doc.version.as_deref().unwrap().starts_with('v'));
    }

    #[test]
    fn json_contains_expected_keys() {
        let json = build_relay_info_json(&cfg(true));
        assert!(json.contains("\"name\":\"My Relay\""), "{json}");
        assert!(json.contains("\"supported_nips\""), "{json}");
        assert!(json.contains("42"), "{json}");
        assert!(json.contains("\"software\""), "{json}");
    }
}
