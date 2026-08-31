//! The one place intersphinx touches the network, behind a trait so every
//! test can stay offline.
//!
//! Sphinx routes its inventory downloads through `sphinx.util.requests.get`
//! with the shared HTTP configuration group (`tls_verify`, `tls_cacerts`,
//! `user_agent`) plus `intersphinx_timeout`
//! (`sphinx/util/requests.py:20-45,96-111`, `ext/intersphinx/_load.py:388-421`;
//! see the research spec §4). [`HttpConfig`] is that group, and
//! [`InventoryFetcher`] is the seam: production uses [`UreqFetcher`], tests
//! inject their own.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The exact `User-Agent` Sphinx 9.1.0 sends when `user_agent` is unset
/// (`sphinx/util/requests.py:20-23`, an f-string over `sphinx.__version__`).
///
/// Emitted verbatim, Sphinx version and all: servers (notably some CDN and
/// WAF configurations) gate inventory downloads on this exact string, and
/// byte-compatibility with Sphinx is worth more than announcing ourselves
/// here. Revisit at 1.0.
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:100.0) Gecko/20100101 Firefox/100.0 Sphinx/9.1.0";

/// `tls_cacerts` (`sphinx/config.py:287`), which Sphinx types
/// `str | dict[str, str] | None`: a single CA bundle path, or a per-netloc
/// map of them (`sphinx/util/requests.py:34-45`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TlsCacerts {
    /// One CA bundle used for every host.
    Bundle(String),
    /// netloc (`host` or `host:port`, userinfo stripped) -> CA bundle path.
    /// A host with no entry falls back to the default trust store.
    PerHost(BTreeMap<String, String>),
}

/// Sphinx's shared HTTP configuration group plus `intersphinx_timeout`
/// (`ext/intersphinx/_load.py:211-227`, `_InvConfig`).
#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    pub tls_verify: bool,
    pub tls_cacerts: Option<TlsCacerts>,
    pub user_agent: Option<String>,
    /// Seconds. `None` is Sphinx's default and means *no* timeout — the
    /// value is handed to `requests` as `timeout=None`
    /// (`ext/intersphinx/__init__.py:70-72`).
    pub timeout: Option<f64>,
}

/// Hand-written rather than derived: `bool::default()` is `false`, and a
/// default that silently turns off certificate verification is not a default
/// anyone should be able to reach by accident. Sphinx's `tls_verify` default
/// is `True` (`config.py:286`), and so is this one.
impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            tls_verify: true,
            tls_cacerts: None,
            user_agent: None,
            timeout: None,
        }
    }
}

impl HttpConfig {
    /// `headers.setdefault('User-Agent', _user_agent or _USER_AGENT)`
    /// (`util/requests.py:96-97`): an empty configured value is falsy in
    /// Python and so also falls back to the default.
    pub fn user_agent(&self) -> &str {
        match self.user_agent.as_deref() {
            Some(agent) if !agent.is_empty() => agent,
            _ => DEFAULT_USER_AGENT,
        }
    }

    /// `_get_tls_cacert(url, tls_cacerts)` (`util/requests.py:34-45`): a
    /// plain string is the bundle for every URL; a mapping is keyed by the
    /// URL's netloc with userinfo stripped, and a host it does not name
    /// falls back to the default trust store.
    pub fn ca_bundle_for(&self, url: &str) -> Option<&str> {
        match self.tls_cacerts.as_ref()? {
            TlsCacerts::Bundle(path) => Some(path.as_str()),
            TlsCacerts::PerHost(map) => map.get(netloc(url)?).map(String::as_str),
        }
    }
}

/// The `netloc` of a URL with any `user:password@` prefix removed —
/// `urlsplit(url).netloc.rsplit('@')[-1]` in `util/requests.py:41`.
fn netloc(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://")?.1;
    let netloc = match after_scheme.find(['/', '?', '#']) {
        Some(end) => &after_scheme[..end],
        None => after_scheme,
    };
    Some(match netloc.rsplit_once('@') {
        Some((_userinfo, host)) => host,
        None => netloc,
    })
}

/// Fetch one inventory over the network.
///
/// The whole of intersphinx's remote half lives behind this one method, so
/// the test suite can exercise loading, caching, merging and resolution with
/// an injected implementation and never open a socket.
pub trait InventoryFetcher {
    fn fetch(&self, url: &str, http: &HttpConfig) -> Result<Vec<u8>>;
}

/// The production fetcher.
///
/// **Not covered by any test**: every test in this crate injects its own
/// fetcher or uses local-file inventory locations, exactly as Sphinx's own
/// suite does (research spec §5). This type compiles and is wired into the
/// builder, but nothing verifies it against a live server — treat changes
/// here as unverified.
pub struct UreqFetcher;

/// Inventories are small (Python's is ~150 KB); this cap only exists so a
/// misconfigured URL cannot stream unbounded data into memory.
const MAX_INVENTORY_BYTES: u64 = 64 * 1024 * 1024;

/// Every certificate in a PEM bundle, in file order.
///
/// A CA bundle — which is what `tls_cacerts` names — is a *concatenation* of
/// PEM certificates: `openssl`'s `cert.pem`, a corporate root plus its
/// intermediates, a trust store shipped by a distribution. `Certificate::from_pem`
/// documents that it "picks the first certificate", so trusting its result
/// alone would silently drop every root after the first and reject servers
/// chaining to any of them.
///
/// Non-certificate PEM sections (a private key sitting in the same file, say)
/// are skipped rather than rejected, mirroring how a trust store is read.
fn root_certs_from_pem(pem: &[u8]) -> Result<Vec<ureq::tls::Certificate<'static>>> {
    let mut certs = Vec::new();
    for item in ureq::tls::parse_pem(pem) {
        match item.map_err(|e| anyhow::anyhow!("{e}"))? {
            ureq::tls::PemItem::Certificate(cert) => certs.push(cert),
            _ => continue,
        }
    }
    if certs.is_empty() {
        anyhow::bail!("no PEM-encoded certificate found");
    }
    Ok(certs)
}

impl InventoryFetcher for UreqFetcher {
    fn fetch(&self, url: &str, http: &HttpConfig) -> Result<Vec<u8>> {
        let mut tls = ureq::tls::TlsConfig::builder().disable_verification(!http.tls_verify);
        // `verify=verify and _get_tls_cacert(url, tls_cacerts)`: the bundle
        // is only consulted when verification is on at all.
        if http.tls_verify {
            if let Some(bundle) = http.ca_bundle_for(url) {
                let pem = std::fs::read(bundle)
                    .with_context(|| format!("cannot read tls_cacerts bundle {bundle}"))?;
                let certs = root_certs_from_pem(&pem)
                    .map_err(|e| anyhow::anyhow!("invalid tls_cacerts bundle {bundle}: {e}"))?;
                tls = tls.root_certs(ureq::tls::RootCerts::new_with_certs(&certs));
            }
        }

        let config = ureq::Agent::config_builder()
            .user_agent(http.user_agent().to_string())
            // `timeout=None` (the default) means no timeout at all.
            .timeout_global(http.timeout.map(std::time::Duration::from_secs_f64))
            .tls_config(tls.build())
            .build();

        let agent: ureq::Agent = config.into();
        let mut response = agent.get(url).call()?;
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_INVENTORY_BYTES)
            .read_to_vec()?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two PEM sections, which is the shape of every real CA bundle. The
    /// bodies are arbitrary: this is the PEM *framing* under test, and the
    /// parser hands back one certificate per section.
    const TWO_CERT_BUNDLE: &[u8] = b"\
-----BEGIN CERTIFICATE-----
AQID
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
BAUG
-----END CERTIFICATE-----
";

    #[test]
    fn a_ca_bundle_keeps_every_certificate_in_it() {
        let certs = root_certs_from_pem(TWO_CERT_BUNDLE).expect("a two-cert bundle parses");
        assert_eq!(
            certs.len(),
            2,
            "a bundle's later roots must not be dropped: `Certificate::from_pem` \
             returns only the first, which would reject servers chaining to any other"
        );
    }

    #[test]
    fn a_bundle_with_no_certificate_in_it_is_an_error() {
        assert!(root_certs_from_pem(b"not a pem file at all\n").is_err());
    }

    #[test]
    fn the_default_configuration_verifies_certificates() {
        assert!(
            HttpConfig::default().tls_verify,
            "a default that skips verification would be a trap"
        );
    }

    #[test]
    fn the_default_user_agent_is_sphinx_9_1_0s_verbatim() {
        let config = HttpConfig::default();
        assert_eq!(
            config.user_agent(),
            "Mozilla/5.0 (X11; Linux x86_64; rv:100.0) Gecko/20100101 Firefox/100.0 Sphinx/9.1.0"
        );
        // An empty string is falsy in Python, so it too falls back.
        let empty = HttpConfig {
            user_agent: Some(String::new()),
            ..HttpConfig::default()
        };
        assert_eq!(empty.user_agent(), DEFAULT_USER_AGENT);
        let custom = HttpConfig {
            user_agent: Some("mine/1".to_string()),
            ..HttpConfig::default()
        };
        assert_eq!(custom.user_agent(), "mine/1");
    }

    #[test]
    fn tls_cacerts_resolve_per_url_for_the_mapping_form() {
        let bundle = HttpConfig {
            tls_cacerts: Some(TlsCacerts::Bundle("/etc/ca.pem".to_string())),
            ..HttpConfig::default()
        };
        assert_eq!(
            bundle.ca_bundle_for("https://anything.example/objects.inv"),
            Some("/etc/ca.pem"),
            "a plain string is the bundle for every URL"
        );

        let per_host = HttpConfig {
            tls_cacerts: Some(TlsCacerts::PerHost(BTreeMap::from([(
                "docs.example.org".to_string(),
                "/etc/example.pem".to_string(),
            )]))),
            ..HttpConfig::default()
        };
        assert_eq!(
            per_host.ca_bundle_for("https://user:pw@docs.example.org/v1/objects.inv"),
            Some("/etc/example.pem"),
            "the key is the netloc with userinfo stripped"
        );
        assert_eq!(
            per_host.ca_bundle_for("https://other.example.org/objects.inv"),
            None,
            "an unnamed host falls back to the default trust store"
        );
        assert_eq!(HttpConfig::default().ca_bundle_for("https://x/y"), None);
    }

    #[test]
    fn netloc_keeps_the_port_and_drops_userinfo_path_and_query() {
        assert_eq!(
            netloc("https://a.example:8443/x?y#z"),
            Some("a.example:8443")
        );
        assert_eq!(
            netloc("https://u:p@a.example:8443/x"),
            Some("a.example:8443")
        );
        assert_eq!(netloc("https://a.example"), Some("a.example"));
        assert_eq!(netloc("local.inv"), None);
    }
}
