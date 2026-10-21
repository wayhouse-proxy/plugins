//! What the plugin's own `routes` declaration allows. The host refuses a whole route set
//! when one entry is outside the approval, so the plugin drops such entries itself.

use std::net::IpAddr;

/// The `routes` part of the embedded capability declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteLimits {
    pub hosts: Vec<String>,
    pub networks: Vec<(IpAddr, u8)>,
    pub max_entries: usize,
}

impl RouteLimits {
    /// Reads `routes` out of a `caps.json` document.
    pub fn from_caps_json(caps: &str) -> Result<Self, String> {
        #[derive(serde::Deserialize)]
        struct Doc {
            routes: Option<Routes>,
        }
        #[derive(serde::Deserialize)]
        struct Routes {
            hosts: Vec<String>,
            backends: Vec<String>,
            max_entries: usize,
        }
        let doc: Doc = serde_json::from_str(caps).map_err(|e| e.to_string())?;
        let r = doc.routes.ok_or("caps.json has no routes capability")?;
        let networks = r
            .backends
            .iter()
            .map(|n| parse_net(n))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            hosts: r.hosts,
            networks,
            max_entries: r.max_entries,
        })
    }

    /// Whether `host` (a concrete name) is covered by a declared exact name or `*.suffix`.
    pub fn allows_host(&self, host: &str) -> bool {
        self.hosts.iter().any(|p| match p.strip_prefix("*.") {
            Some(suffix) => host
                .strip_suffix(suffix)
                .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
            None => p == host,
        })
    }

    /// Whether `ip` is inside a declared backend network.
    pub fn allows_backend(&self, ip: IpAddr) -> bool {
        self.networks
            .iter()
            .any(|&(base, prefix)| in_net(base, prefix, ip))
    }
}

/// The host's own rule for a route hostname (`valid_pattern` without the `*.` form):
/// lowercase DNS labels of 1 to 63 characters, no edge `-`, at most 253 characters in all.
pub fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn parse_net(s: &str) -> Result<(IpAddr, u8), String> {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let base: IpAddr = addr
        .parse()
        .map_err(|_| format!("{s:?} is not a network"))?;
    let max = if base.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        Some(p) => p
            .parse::<u8>()
            .ok()
            .filter(|p| *p <= max)
            .ok_or_else(|| format!("{s:?} has an invalid prefix"))?,
        None => max,
    };
    Ok((base, prefix))
}

fn in_net(base: IpAddr, prefix: u8, ip: IpAddr) -> bool {
    let (a, b): (Vec<u8>, Vec<u8>) = match (base, ip) {
        (IpAddr::V4(a), IpAddr::V4(b)) => (a.octets().to_vec(), b.octets().to_vec()),
        (IpAddr::V6(a), IpAddr::V6(b)) => (a.octets().to_vec(), b.octets().to_vec()),
        _ => return false,
    };
    let full = usize::from(prefix / 8);
    let rem = prefix % 8;
    a[..full] == b[..full] && (rem == 0 || (a[full] ^ b[full]) >> (8 - rem) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: &str = r#"{"routes":{"hosts":["*.mc.example.com","play.example.com"],"backends":["10.0.0.0/16","192.168.1.7"],"max_entries":2}}"#;

    #[test]
    fn parses_routes() {
        let l = RouteLimits::from_caps_json(CAPS).unwrap();
        assert_eq!(l.max_entries, 2);
        assert_eq!(l.hosts.len(), 2);
        assert_eq!(l.networks.len(), 2);
    }

    #[test]
    fn missing_routes_is_an_error() {
        assert!(RouteLimits::from_caps_json(r#"{"log":true}"#).is_err());
    }

    #[test]
    fn host_patterns() {
        let l = RouteLimits::from_caps_json(CAPS).unwrap();
        assert!(l.allows_host("a.mc.example.com"));
        assert!(l.allows_host("a.b.mc.example.com"));
        assert!(l.allows_host("play.example.com"));
        assert!(!l.allows_host("mc.example.com"));
        assert!(!l.allows_host("evil.example.com"));
        assert!(!l.allows_host("amc.example.com"));
    }

    #[test]
    fn backend_networks() {
        let l = RouteLimits::from_caps_json(CAPS).unwrap();
        assert!(l.allows_backend("10.0.200.1".parse().unwrap()));
        assert!(!l.allows_backend("10.1.0.1".parse().unwrap()));
        assert!(l.allows_backend("192.168.1.7".parse().unwrap()));
        assert!(!l.allows_backend("192.168.1.8".parse().unwrap()));
        assert!(!l.allows_backend("::1".parse().unwrap()));
    }
}
