//! The instances the person added, kept in a small JSON file of the app's data directory.
//!
//! An instance is what its hub's manifest describes (`/.well-known/ruchoir-manifest`): its domain,
//! Propolis's origin (the issuer of its tokens) and its installed products with their origins.
//! Nothing secret lives here: the grants are in the system's keychain (`vault.rs`), filed by issuer
//! by `ruchoir-native`.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Why an address typed by the person is not an instance's domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    Empty,
    NotHttps,
    HasPath,
    HasCredentials,
    Invalid,
}

impl DomainError {
    /// The code the interface translates.
    pub fn code(&self) -> &'static str {
        match self {
            DomainError::Empty => "empty",
            DomainError::NotHttps => "not_https",
            DomainError::HasPath => "has_path",
            DomainError::HasCredentials => "has_credentials",
            DomainError::Invalid => "invalid",
        }
    }
}

/// What discovery is given for what the person typed: the bare domain (`example.org`, lowercase),
/// whose manifest is at `https://hub.<domain>/.well-known/ruchoir-manifest`.
///
/// With `allow_loopback_http` (debug builds only), `http://127.0.0.1:<port>` or
/// `http://localhost:<port>` names a stand-in hub on this machine and answers its manifest's full
/// address. A release build never reaches an instance in clear.
pub fn discovery_address(input: &str, allow_loopback_http: bool) -> Result<String, DomainError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DomainError::Empty);
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| DomainError::Invalid)?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    let scheme = url.scheme();
    let plain_loopback = scheme == "http" && allow_loopback_http && loopback;
    if scheme != "https" && !plain_loopback {
        return Err(DomainError::NotHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DomainError::HasCredentials);
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(DomainError::HasPath);
    }
    let host = url.host_str().ok_or(DomainError::Invalid)?;
    let host = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    Ok(if plain_loopback {
        format!("http://{host}/.well-known/ruchoir-manifest")
    } else {
        host
    })
}

/// A product of an instance, as its manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledProduct {
    /// `hub`, `drive`, `calendar`, `chat`...
    pub name: String,
    /// Its origin (`https://drive.example.org`), also the `resource` of its access tokens.
    pub url: String,
}

/// One instance the person added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// Random, chosen by the app: it names the folder of the instance's local copies.
    pub id: String,
    /// The domain its manifest gives (`example.org`).
    pub domain: String,
    /// Propolis's origin, the issuer of the instance's tokens.
    pub issuer: String,
    /// The installed products, Propolis excepted, in the manifest's order.
    pub products: Vec<InstalledProduct>,
    /// Seconds since the Unix epoch.
    pub added_at: i64,
}

impl Instance {
    /// The origin of the product named `name`, when it is installed.
    pub fn product(&self, name: &str) -> Option<&str> {
        self.products
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.url.as_str())
    }

    /// Every origin of the instance: Propolis and the products. A product's window may go to any of
    /// them (its sign-in passes through Propolis); anything else opens in the system browser.
    pub fn origins(&self) -> Vec<String> {
        std::iter::once(self.issuer.clone())
            .chain(self.products.iter().map(|p| p.url.clone()))
            .collect()
    }
}

impl From<&ruchoir_native::Instance> for Instance {
    fn from(found: &ruchoir_native::Instance) -> Self {
        Instance {
            id: random_hex(16),
            domain: found.domain.clone(),
            issuer: found.issuer.clone(),
            products: found
                .products
                .iter()
                .map(|p| InstalledProduct {
                    name: p.name.clone(),
                    url: p.url.clone(),
                })
                .collect(),
            added_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default(),
        }
    }
}

/// Every instance added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceList {
    pub instances: Vec<Instance>,
}

impl InstanceList {
    /// Read the list. A missing or unreadable file is an empty list: the person adds an instance
    /// again, which is better than an app that will not start.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write the list, through a temporary file so a crash never leaves half of it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// Add an instance at the end of the list. One already there (same domain) keeps its id and
    /// takes the newly discovered issuer and products, so adding it again refreshes it.
    pub fn add(&mut self, instance: Instance) -> Instance {
        if let Some(known) = self
            .instances
            .iter_mut()
            .find(|i| i.domain == instance.domain)
        {
            known.issuer = instance.issuer;
            known.products = instance.products;
            return known.clone();
        }
        self.instances.push(instance.clone());
        instance
    }

    /// Forget an instance.
    pub fn remove(&mut self, id: &str) -> Option<Instance> {
        let index = self.instances.iter().position(|i| i.id == id)?;
        Some(self.instances.remove(index))
    }

    /// An instance by id.
    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }
}

/// `bytes` random bytes, as lowercase hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the system's random source");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_is_bare_and_lowercase() {
        assert_eq!(
            discovery_address("example.org", false).unwrap(),
            "example.org"
        );
        assert_eq!(
            discovery_address(" HTTPS://Example.org/ ", false).unwrap(),
            "example.org"
        );
        assert_eq!(
            discovery_address("example.org:8443", false).unwrap(),
            "example.org:8443"
        );
        for (input, error) in [
            ("", DomainError::Empty),
            ("http://example.org", DomainError::NotHttps),
            ("example.org/app", DomainError::HasPath),
            ("https://example.org/?a=1", DomainError::HasPath),
            ("https://u:p@example.org", DomainError::HasCredentials),
            ("https://", DomainError::Invalid),
        ] {
            assert_eq!(discovery_address(input, false), Err(error), "{input}");
        }
    }

    #[test]
    fn loopback_http_only_when_allowed() {
        assert_eq!(
            discovery_address("http://127.0.0.1:18090", true).unwrap(),
            "http://127.0.0.1:18090/.well-known/ruchoir-manifest"
        );
        assert_eq!(
            discovery_address("http://127.0.0.1:18090", false),
            Err(DomainError::NotHttps)
        );
        assert_eq!(
            discovery_address("http://example.org", true),
            Err(DomainError::NotHttps)
        );
    }

    fn instance(domain: &str) -> Instance {
        Instance {
            id: random_hex(16),
            domain: domain.into(),
            issuer: format!("https://auth.{domain}"),
            products: vec![
                InstalledProduct {
                    name: "hub".into(),
                    url: format!("https://hub.{domain}"),
                },
                InstalledProduct {
                    name: "drive".into(),
                    url: format!("https://drive.{domain}"),
                },
            ],
            added_at: 0,
        }
    }

    #[test]
    fn list_round_trips_and_holds_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instances.json");
        let mut list = InstanceList::default();
        let added = list.add(instance("a.example"));
        assert_eq!(added.id.len(), 32);
        list.save(&path).unwrap();
        assert_eq!(InstanceList::load(&path), list);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("token"));
    }

    #[test]
    fn a_missing_or_broken_file_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            InstanceList::load(&dir.path().join("none.json")),
            InstanceList::default()
        );
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        assert_eq!(InstanceList::load(&broken), InstanceList::default());
    }

    #[test]
    fn adding_a_known_domain_refreshes_it_and_keeps_its_id() {
        let mut list = InstanceList::default();
        let first = list.add(instance("a.example"));
        let mut again = instance("a.example");
        again.products.pop();
        let refreshed = list.add(again);
        assert_eq!(refreshed.id, first.id);
        assert_eq!(list.instances.len(), 1);
        assert_eq!(list.instances[0].products.len(), 1);
        list.add(instance("b.example"));
        assert_eq!(list.instances.len(), 2);
        assert!(list.remove(&first.id).is_some());
        assert!(list.get(&first.id).is_none());
    }

    #[test]
    fn an_instance_names_its_products_and_origins() {
        let i = instance("a.example");
        assert_eq!(i.product("drive"), Some("https://drive.a.example"));
        assert_eq!(i.product("chat"), None);
        assert_eq!(
            i.origins(),
            [
                "https://auth.a.example",
                "https://hub.a.example",
                "https://drive.a.example"
            ]
        );
    }
}
