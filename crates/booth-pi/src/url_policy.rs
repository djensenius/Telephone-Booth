//! URL validation policy for remote audio fetches.
//!
//! Prevents SSRF by enforcing scheme restrictions, an optional host allowlist,
//! and blocking requests to private/link-local/loopback IP addresses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use thiserror::Error;
use url::Url;

/// Errors arising from URL policy enforcement.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UrlPolicyError {
    /// The URL could not be parsed.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    /// The URL uses a disallowed scheme (e.g. `http` when only `https` is
    /// permitted).
    #[error("disallowed scheme \"{scheme}\" (allowed: {allowed})")]
    DisallowedScheme {
        /// The scheme found in the URL.
        scheme: String,
        /// Comma-separated list of allowed schemes.
        allowed: String,
    },
    /// The URL's host is not in the configured allowlist.
    #[error("host \"{host}\" is not in the allowed audio hosts list")]
    HostNotAllowed {
        /// The host that was rejected.
        host: String,
    },
    /// The URL resolves to a private, link-local, or loopback address.
    #[error("resolved address {addr} is private/link-local/loopback")]
    PrivateAddress {
        /// The offending resolved address.
        addr: IpAddr,
    },
    /// The URL has no host component.
    #[error("URL has no host")]
    MissingHost,
}

/// Policy configuration for validating remote audio URLs.
#[derive(Debug, Clone)]
pub struct AudioFetchPolicy {
    /// Whether plain `http` is permitted (default: false — only `https`).
    pub allow_http: bool,
    /// When non-empty, only these hosts (exact match, case-insensitive) are
    /// permitted for audio downloads.
    pub allowed_hosts: Vec<String>,
    /// Whether to allow fetches to private/link-local/loopback IPs.
    pub allow_private_hosts: bool,
}

impl Default for AudioFetchPolicy {
    fn default() -> Self {
        Self {
            allow_http: false,
            allowed_hosts: Vec::new(),
            allow_private_hosts: false,
        }
    }
}

impl AudioFetchPolicy {
    /// Validate a URL string against the policy.
    ///
    /// Returns the parsed [`Url`] on success so callers can use it directly.
    pub fn validate(&self, raw_url: &str) -> Result<Url, UrlPolicyError> {
        let parsed = Url::parse(raw_url).map_err(|e| UrlPolicyError::InvalidUrl(e.to_string()))?;

        // --- Scheme check ---
        let scheme = parsed.scheme();
        let scheme_ok = scheme == "https" || (self.allow_http && scheme == "http");
        if !scheme_ok {
            let allowed = if self.allow_http {
                "https, http".to_string()
            } else {
                "https".to_string()
            };
            return Err(UrlPolicyError::DisallowedScheme {
                scheme: scheme.to_string(),
                allowed,
            });
        }

        // --- Host presence ---
        let host_str = parsed.host_str().ok_or(UrlPolicyError::MissingHost)?;

        // --- Host allowlist ---
        if !self.allowed_hosts.is_empty() {
            let host_lower = host_str.to_ascii_lowercase();
            let allowed = self
                .allowed_hosts
                .iter()
                .any(|h| h.to_ascii_lowercase() == host_lower);
            if !allowed {
                return Err(UrlPolicyError::HostNotAllowed {
                    host: host_str.to_string(),
                });
            }
        }

        // --- Private-IP blocking (using url's parsed host for proper IPv6 handling) ---
        if !self.allow_private_hosts {
            let ip = match parsed.host() {
                Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
                Some(url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
                _ => None,
            };
            if let Some(addr) = ip
                && is_private_or_reserved(addr)
            {
                return Err(UrlPolicyError::PrivateAddress { addr });
            }
        }

        Ok(parsed)
    }

    /// Check whether a resolved IP address is allowed under this policy.
    ///
    /// Call this after DNS resolution to block requests that resolve to
    /// private addresses even when the hostname itself isn't an IP literal.
    pub fn check_resolved_ip(&self, addr: IpAddr) -> Result<(), UrlPolicyError> {
        if !self.allow_private_hosts && is_private_or_reserved(addr) {
            return Err(UrlPolicyError::PrivateAddress { addr });
        }
        Ok(())
    }
}

/// Returns `true` if the address is loopback, link-local, private (RFC 1918 /
/// RFC 4193), or a well-known metadata service address.
fn is_private_or_reserved(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => is_private_ipv4(ip),
        IpAddr::V6(ip) => is_private_ipv6(ip),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()                          // 127.0.0.0/8
        || ip.is_private()                    // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()                 // 169.254.0.0/16
        || ip.is_broadcast()                  // 255.255.255.255
        || ip.is_unspecified()                // 0.0.0.0
        || is_shared_address_space(ip)        // 100.64.0.0/10 (CGN)
        || is_documentation_ipv4(ip)          // 192.0.2/24, 198.51.100/24, 203.0.113/24
        || is_metadata_service(ip) // 169.254.169.254
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()                          // ::1
        || ip.is_unspecified()                // ::
        || is_unique_local_ipv6(ip)           // fc00::/7
        || is_link_local_ipv6(ip)             // fe80::/10
        // Also check if it's a v4-mapped address wrapping a private v4
        || ip.to_ipv4_mapped().is_some_and(is_private_ipv4)
}

/// 100.64.0.0/10 — Carrier-Grade NAT (RFC 6598).
fn is_shared_address_space(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] & 0xC0) == 64
}

/// Documentation ranges: 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24.
fn is_documentation_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
}

/// Cloud metadata service at 169.254.169.254.
fn is_metadata_service(ip: Ipv4Addr) -> bool {
    ip == Ipv4Addr::new(169, 254, 169, 254)
}

/// fc00::/7 — Unique Local Addresses (RFC 4193).
fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    (segments[0] & 0xFE00) == 0xFC00
}

/// fe80::/10 — Link-Local.
fn is_link_local_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    (segments[0] & 0xFFC0) == 0xFE80
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn default_policy() -> AudioFetchPolicy {
        AudioFetchPolicy::default()
    }

    #[test]
    fn accepts_https_url() {
        let policy = default_policy();
        let result = policy.validate("https://cdn.example.com/audio/clip.flac");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_http_by_default() {
        let policy = default_policy();
        let result = policy.validate("http://cdn.example.com/audio/clip.flac");
        assert!(matches!(
            result,
            Err(UrlPolicyError::DisallowedScheme { .. })
        ));
    }

    #[test]
    fn allows_http_when_configured() {
        let policy = AudioFetchPolicy {
            allow_http: true,
            ..Default::default()
        };
        let result = policy.validate("http://cdn.example.com/audio/clip.flac");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_ftp_scheme() {
        let policy = default_policy();
        let result = policy.validate("ftp://cdn.example.com/audio/clip.flac");
        assert!(matches!(
            result,
            Err(UrlPolicyError::DisallowedScheme { .. })
        ));
    }

    #[test]
    fn rejects_host_not_in_allowlist() {
        let policy = AudioFetchPolicy {
            allowed_hosts: vec!["cdn.example.com".to_string()],
            ..Default::default()
        };
        let result = policy.validate("https://evil.attacker.com/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::HostNotAllowed { .. })));
    }

    #[test]
    fn accepts_host_in_allowlist_case_insensitive() {
        let policy = AudioFetchPolicy {
            allowed_hosts: vec!["CDN.Example.Com".to_string()],
            ..Default::default()
        };
        let result = policy.validate("https://cdn.example.com/audio/clip.flac");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_loopback_ip() {
        let policy = default_policy();
        let result = policy.validate("https://127.0.0.1/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }

    #[test]
    fn rejects_private_rfc1918() {
        let policy = default_policy();
        for url in [
            "https://10.0.0.1/audio.flac",
            "https://172.16.0.1/audio.flac",
            "https://192.168.1.1/audio.flac",
        ] {
            let result = policy.validate(url);
            assert!(
                matches!(result, Err(UrlPolicyError::PrivateAddress { .. })),
                "expected rejection for {url}"
            );
        }
    }

    #[test]
    fn rejects_link_local() {
        let policy = default_policy();
        let result = policy.validate("https://169.254.1.1/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }

    #[test]
    fn rejects_metadata_service() {
        let policy = default_policy();
        let result = policy.validate("https://169.254.169.254/latest/meta-data/");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }

    #[test]
    fn allows_private_when_configured() {
        let policy = AudioFetchPolicy {
            allow_private_hosts: true,
            ..Default::default()
        };
        let result = policy.validate("https://192.168.1.1/audio.flac");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_ipv6_loopback() {
        let policy = default_policy();
        let result = policy.validate("https://[::1]/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }

    #[test]
    fn rejects_ipv6_unique_local() {
        let policy = default_policy();
        let result = policy.validate("https://[fd12::1]/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }

    #[test]
    fn rejects_ipv6_link_local() {
        let policy = default_policy();
        let result = policy.validate("https://[fe80::1]/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }

    #[test]
    fn check_resolved_ip_rejects_private() {
        let policy = default_policy();
        let addr: IpAddr = "10.0.0.5".parse().unwrap();
        assert!(matches!(
            policy.check_resolved_ip(addr),
            Err(UrlPolicyError::PrivateAddress { .. })
        ));
    }

    #[test]
    fn check_resolved_ip_allows_public() {
        let policy = default_policy();
        let addr: IpAddr = "93.184.216.34".parse().unwrap();
        assert!(policy.check_resolved_ip(addr).is_ok());
    }

    #[test]
    fn rejects_invalid_url() {
        let policy = default_policy();
        let result = policy.validate("not a url at all");
        assert!(matches!(result, Err(UrlPolicyError::InvalidUrl(_))));
    }

    #[test]
    fn rejects_cgn_address_space() {
        let policy = default_policy();
        let result = policy.validate("https://100.64.0.1/audio.flac");
        assert!(matches!(result, Err(UrlPolicyError::PrivateAddress { .. })));
    }
}
