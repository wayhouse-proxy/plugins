//! Reading the Pelican Application API and turning servers into routes.
//!
//! Wire format checked against Pelican's source (`app/Data/Api/Application/ServerData.php`,
//! `AllocationData.php`, `app/Data/Api/Envelope.php`): a list is `{object: "list", data:
//! [{object, attributes}], meta: {pagination: {total_pages, ...}}}` and `?include=allocations`
//! nests the server's allocations under `attributes.relationships.allocations`.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;

use crate::caps::RouteLimits;

/// The plugin's non-secret `init` config.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Base URL of the panel, for example `https://panel.example`.
    pub panel_url: String,
    /// Hostname per server; `{identifier}`, `{name}` (slugified), `{external_id}` and `{id}`.
    pub host_template: String,
    /// Node id to the address the entry node reaches it at; wins over the allocation's ip.
    #[serde(default)]
    pub node_addresses: BTreeMap<String, String>,
    /// Ports a route may point at, `"lo-hi"` (or one port). Unset allows any port on an
    /// allowed backend address; the `routes.backends` capability limits addresses only.
    #[serde(default)]
    pub backend_ports: Option<PortRange>,
}

/// An inclusive port range, written `"25565-25999"` or `"25565"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct PortRange {
    pub lo: u16,
    pub hi: u16,
}

impl PortRange {
    pub fn contains(self, port: u16) -> bool {
        (self.lo..=self.hi).contains(&port)
    }
}

impl TryFrom<String> for PortRange {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        let bad = || format!("backend_ports {s:?} must be a port or `lo-hi` (1 to 65535)");
        let (lo, hi) = match s.split_once('-') {
            Some((lo, hi)) => (lo.trim().parse::<u16>(), hi.trim().parse::<u16>()),
            None => (s.trim().parse::<u16>(), s.trim().parse::<u16>()),
        };
        match (lo, hi) {
            (Ok(lo), Ok(hi)) if lo >= 1 && lo <= hi => Ok(Self { lo, hi }),
            _ => Err(bad()),
        }
    }
}

impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("bad config: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    pub id: u64,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub id: u64,
    pub identifier: String,
    pub name: String,
    pub external_id: Option<String>,
    pub suspended: bool,
    /// Pelican's `status`: null when the server is installed and usable.
    pub status: Option<String>,
    pub node: u64,
    /// Id of the primary allocation.
    pub allocation: Option<u64>,
    pub allocations: Vec<Allocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub servers: Vec<Server>,
    pub total_pages: u32,
    /// Servers on the panel when this page was read; differs between pages of one listing when
    /// a server was created or deleted in between.
    pub total: u64,
}

/// Parses one `GET /api/application/servers?include=allocations` response.
pub fn parse_page(body: &[u8]) -> Result<Page, String> {
    #[derive(Deserialize)]
    struct List {
        object: String,
        data: Vec<Item>,
        meta: Option<Meta>,
    }
    #[derive(Deserialize)]
    struct Meta {
        pagination: Option<Pagination>,
    }
    #[derive(Deserialize)]
    struct Pagination {
        total_pages: u32,
        total: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Item {
        attributes: Attrs,
    }
    #[derive(Deserialize)]
    struct Attrs {
        id: u64,
        external_id: Option<String>,
        identifier: String,
        name: String,
        #[serde(default)]
        suspended: bool,
        status: Option<String>,
        node: u64,
        allocation: Option<u64>,
        relationships: Option<Rel>,
    }
    #[derive(Deserialize)]
    struct Rel {
        allocations: Option<AllocList>,
    }
    #[derive(Deserialize)]
    struct AllocList {
        object: Option<String>,
        #[serde(default)]
        data: Vec<AllocItem>,
    }
    #[derive(Deserialize)]
    struct AllocItem {
        attributes: AllocAttrs,
    }
    #[derive(Deserialize)]
    struct AllocAttrs {
        id: u64,
        ip: String,
        port: u16,
    }

    let list: List = serde_json::from_slice(body).map_err(|e| format!("bad server list: {e}"))?;
    if list.object != "list" {
        return Err(format!("expected a list, got {:?}", list.object));
    }
    // A key without Allocations read access still gets the include, as a `null_resource`;
    // reading that as "no allocations" would withdraw every route.
    if list.data.iter().any(|i| {
        i.attributes
            .relationships
            .as_ref()
            .and_then(|r| r.allocations.as_ref())
            .is_some_and(|l| l.object.as_deref() == Some("null_resource"))
    }) {
        return Err(
            "the API key cannot read allocations: give it read access to Allocations".into(),
        );
    }
    let servers = list
        .data
        .into_iter()
        .map(|i| {
            let a = i.attributes;
            let allocations = a
                .relationships
                .and_then(|r| r.allocations)
                .map(|l| {
                    l.data
                        .into_iter()
                        .map(|x| Allocation {
                            id: x.attributes.id,
                            ip: x.attributes.ip,
                            port: x.attributes.port,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Server {
                id: a.id,
                identifier: a.identifier,
                name: a.name,
                external_id: a.external_id,
                suspended: a.suspended,
                status: a.status,
                node: a.node,
                allocation: a.allocation,
                allocations,
            }
        })
        .collect();
    // Without the page count the list could be a partial view; fail closed.
    let pagination = list
        .meta
        .and_then(|m| m.pagination)
        .ok_or("server list has no meta.pagination.total_pages")?;
    // The total lets `plan` notice a list that changed between two page reads.
    let total = pagination
        .total
        .ok_or("server list has no meta.pagination.total")?;
    Ok(Page {
        servers,
        total_pages: pagination.total_pages,
        total,
    })
}

/// Lowercase letters and digits, runs of anything else collapsed to one `-`, no edge `-`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Fills the template; `None` when a used placeholder has no value or the result is empty.
pub fn render_host(template: &str, s: &Server) -> Option<String> {
    let mut out = template.to_string();
    for (key, value) in [
        ("{identifier}", Some(s.identifier.to_ascii_lowercase())),
        ("{id}", Some(s.id.to_string())),
        ("{name}", Some(slug(&s.name))),
        ("{external_id}", s.external_id.as_deref().map(slug)),
    ] {
        if out.contains(key) {
            let v = value.filter(|v| !v.is_empty())?;
            out = out.replace(key, &v);
        }
    }
    Some(out.to_ascii_lowercase())
}

/// A route entry as the host's `routes_set` takes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub host: String,
    pub backend: String,
}

/// Routes for `servers`, and a line for everything left out and why.
pub fn build_routes(
    servers: &[Server],
    cfg: &Config,
    limits: &RouteLimits,
) -> (Vec<Entry>, Vec<String>) {
    let mut sorted: Vec<&Server> = servers.iter().collect();
    sorted.sort_by_key(|s| s.id);
    let mut entries: Vec<Entry> = Vec::new();
    let mut warnings = Vec::new();
    for s in sorted {
        if s.suspended || s.status.is_some() {
            continue;
        }
        let Some(primary) = s
            .allocation
            .and_then(|id| s.allocations.iter().find(|a| a.id == id))
        else {
            continue;
        };
        if primary.port == 0 {
            warnings.push(format!("server {}: allocation has port 0", s.id));
            continue;
        }
        let Some(host) = render_host(&cfg.host_template, s) else {
            warnings.push(format!(
                "server {}: host_template has no value for it",
                s.id
            ));
            continue;
        };
        let ip: Option<IpAddr> = match cfg.node_addresses.get(&s.node.to_string()) {
            Some(a) => a.parse().ok(),
            None => primary
                .ip
                .parse()
                .ok()
                .filter(|ip: &IpAddr| !ip.is_unspecified()),
        };
        let Some(ip) = ip else {
            warnings.push(format!(
                "server {}: allocation ip {:?} is not usable, set node_addresses[\"{}\"]",
                s.id, primary.ip, s.node
            ));
            continue;
        };
        if !crate::caps::valid_host(&host) {
            warnings.push(format!("server {}: {host:?} is not a valid hostname", s.id));
            continue;
        }
        if !limits.allows_host(&host) {
            warnings.push(format!(
                "server {}: host {host} is outside the declared routes.hosts",
                s.id
            ));
            continue;
        }
        if !limits.allows_backend(ip) {
            warnings.push(format!(
                "server {}: backend {ip} is outside the declared routes.backends",
                s.id
            ));
            continue;
        }
        if cfg.backend_ports.is_some_and(|r| !r.contains(primary.port)) {
            warnings.push(format!(
                "server {}: backend port {} is outside backend_ports",
                s.id, primary.port
            ));
            continue;
        }
        if entries.iter().any(|e| e.host == host) {
            warnings.push(format!(
                "server {}: host {host} is already taken by a lower server id",
                s.id
            ));
            continue;
        }
        if entries.len() >= limits.max_entries {
            warnings.push(format!(
                "server {}: dropped, routes.max_entries is {}",
                s.id, limits.max_entries
            ));
            continue;
        }
        let backend = SocketAddr::new(ip, primary.port).to_string();
        entries.push(Entry { host, backend });
    }
    (entries, warnings)
}

/// Most pages one tick reads (the host allows 8 http calls per guest call).
pub const MAX_PAGES: u32 = 8;

/// URL of page `page` of the server list.
pub fn page_url(panel_url: &str, page: u32) -> String {
    format!(
        "{}/api/application/servers?include=allocations&per_page=100&page={page}",
        panel_url.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"object":"list","data":[
      {"object":"server","attributes":{"id":7,"external_id":null,"uuid":"u","identifier":"abc12345","name":"My Server!","status":null,"suspended":false,"node":1,"allocation":11,"user":1,"egg":1,
       "relationships":{"allocations":{"object":"list","data":[
         {"object":"allocation","attributes":{"id":10,"ip":"0.0.0.0","alias":null,"port":25564,"notes":null,"assigned":true}},
         {"object":"allocation","attributes":{"id":11,"ip":"10.0.0.5","alias":null,"port":25565,"notes":null,"assigned":true}}]}}}}],
      "meta":{"pagination":{"total":1,"count":1,"per_page":50,"current_page":1,"total_pages":3,"links":{}}}}"#;

    fn server(id: u64, name: &str, alloc_ip: &str) -> Server {
        Server {
            id,
            identifier: format!("id{id}"),
            name: name.into(),
            external_id: None,
            suspended: false,
            status: None,
            node: 1,
            allocation: Some(id * 10),
            allocations: vec![Allocation {
                id: id * 10,
                ip: alloc_ip.into(),
                port: 25565,
            }],
        }
    }

    fn cfg(tpl: &str) -> Config {
        Config {
            panel_url: "https://panel.example".into(),
            host_template: tpl.into(),
            node_addresses: BTreeMap::new(),
            backend_ports: None,
        }
    }

    fn limits(max: usize) -> RouteLimits {
        RouteLimits::from_caps_json(&format!(
            r#"{{"routes":{{"hosts":["*.mc.example.com"],"backends":["10.0.0.0/8"],"max_entries":{max}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn parses_a_server_list() {
        let p = parse_page(LIST.as_bytes()).unwrap();
        assert_eq!(p.total_pages, 3);
        let s = &p.servers[0];
        assert_eq!((s.id, s.identifier.as_str(), s.node), (7, "abc12345", 1));
        assert_eq!(s.allocation, Some(11));
        assert_eq!(s.allocations.len(), 2);
        assert_eq!(s.allocations[1].port, 25565);
    }

    #[test]
    fn parse_rejects_non_lists() {
        assert!(parse_page(b"{\"errors\":[{}]}").is_err());
        assert!(parse_page(b"nope").is_err());
    }

    #[test]
    fn server_without_includes_has_no_allocations() {
        let body = LIST.replace("\"relationships\"", "\"other\"");
        let p = parse_page(body.as_bytes()).unwrap();
        assert!(p.servers[0].allocations.is_empty());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("My Server!"), "my-server");
        assert_eq!(slug("--A__b--"), "a-b");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn renders_placeholders() {
        let s = server(7, "My Server", "10.0.0.1");
        assert_eq!(
            render_host("{identifier}.mc.example.com", &s).unwrap(),
            "id7.mc.example.com"
        );
        assert_eq!(
            render_host("{name}-{id}.mc.example.com", &s).unwrap(),
            "my-server-7.mc.example.com"
        );
    }

    #[test]
    fn missing_external_id_skips_the_server() {
        let s = server(7, "x", "10.0.0.1");
        assert!(render_host("{external_id}.mc.example.com", &s).is_none());
    }

    #[test]
    fn config_requires_panel_and_template_and_rejects_unknown() {
        assert!(Config::parse(br#"{"panel_url":"https://p","host_template":"{id}.x"}"#).is_ok());
        assert!(Config::parse(br#"{"panel_url":"https://p"}"#).is_err());
        assert!(
            Config::parse(br#"{"panel_url":"https://p","host_template":"x","oops":1}"#).is_err()
        );
        assert!(Config::parse(b"").is_err());
    }

    #[test]
    fn backend_ports_parse_and_reject_nonsense() {
        let with = |v: &str| {
            Config::parse(
                format!(
                    r#"{{"panel_url":"https://p","host_template":"{{id}}.x","backend_ports":{v}}}"#
                )
                .as_bytes(),
            )
        };
        assert_eq!(
            with(r#""25565-25999""#).unwrap().backend_ports,
            Some(PortRange {
                lo: 25565,
                hi: 25999
            })
        );
        assert_eq!(
            with(r#""25565""#).unwrap().backend_ports,
            Some(PortRange {
                lo: 25565,
                hi: 25565
            })
        );
        for bad in [
            r#""0-10""#,
            r#""10-5""#,
            r#""a-b""#,
            r#""70000""#,
            r#""""#,
            "5",
        ] {
            assert!(with(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_port_outside_backend_ports_is_dropped_with_a_reason() {
        let mut s = server(1, "a", "10.0.0.9");
        s.allocations[0].port = 22;
        let mut c = cfg("{identifier}.mc.example.com");
        let (e, w) = build_routes(&[s.clone()], &c, &limits(10));
        assert_eq!(e.len(), 1, "unset allows any port: {w:?}");
        c.backend_ports = Some(PortRange {
            lo: 25565,
            hi: 25999,
        });
        let (e, w) = build_routes(&[s.clone()], &c, &limits(10));
        assert!(e.is_empty());
        assert!(
            w[0].contains("port 22") && w[0].contains("backend_ports"),
            "{w:?}"
        );
        s.allocations[0].port = 25565;
        assert_eq!(build_routes(&[s], &c, &limits(10)).0.len(), 1);
    }

    #[test]
    fn routes_use_the_primary_allocation() {
        let mut s = server(1, "a", "10.0.0.9");
        s.allocations.push(Allocation {
            id: 99,
            ip: "10.9.9.9".into(),
            port: 1,
        });
        let (e, w) = build_routes(&[s], &cfg("{identifier}.mc.example.com"), &limits(10));
        assert_eq!(
            e,
            vec![Entry {
                host: "id1.mc.example.com".into(),
                backend: "10.0.0.9:25565".into()
            }]
        );
        assert!(w.is_empty());
    }

    #[test]
    fn node_address_overrides_and_fills_unspecified_ip() {
        let mut c = cfg("{identifier}.mc.example.com");
        c.node_addresses.insert("1".into(), "10.0.0.5".into());
        let (e, _) = build_routes(&[server(1, "a", "0.0.0.0")], &c, &limits(10));
        assert_eq!(e[0].backend, "10.0.0.5:25565");
        let (e, _) = build_routes(&[server(1, "a", "203.0.113.4")], &c, &limits(10));
        assert_eq!(e[0].backend, "10.0.0.5:25565");
    }

    #[test]
    fn unspecified_ip_without_node_address_is_skipped_with_a_reason() {
        let (e, w) = build_routes(
            &[server(1, "a", "0.0.0.0")],
            &cfg("{identifier}.mc.example.com"),
            &limits(10),
        );
        assert!(e.is_empty());
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("node_addresses"));
    }

    #[test]
    fn suspended_and_unallocated_servers_are_skipped() {
        let mut a = server(1, "a", "10.0.0.1");
        a.suspended = true;
        let mut b = server(2, "b", "10.0.0.2");
        b.allocation = None;
        let (e, _) = build_routes(&[a, b], &cfg("{identifier}.mc.example.com"), &limits(10));
        assert!(e.is_empty());
    }

    #[test]
    fn entries_outside_the_declaration_are_dropped() {
        let (e, w) = build_routes(
            &[server(1, "a", "192.0.2.1"), server(2, "b", "10.0.0.2")],
            &cfg("{identifier}.mc.example.com"),
            &limits(10),
        );
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].host, "id2.mc.example.com");
        assert_eq!(w.len(), 1);
        let (e, _) = build_routes(
            &[server(1, "a", "10.0.0.1")],
            &cfg("{identifier}.other.example"),
            &limits(10),
        );
        assert!(e.is_empty());
    }

    #[test]
    fn duplicate_hosts_keep_the_lowest_server_id() {
        let (e, w) = build_routes(
            &[server(2, "Same", "10.0.0.2"), server(1, "same", "10.0.0.1")],
            &cfg("{name}.mc.example.com"),
            &limits(10),
        );
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].backend, "10.0.0.1:25565");
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn max_entries_truncates_deterministically() {
        let servers: Vec<_> = (1..=3).map(|i| server(i, "x", "10.0.0.1")).collect();
        let (e, w) = build_routes(&servers, &cfg("{identifier}.mc.example.com"), &limits(2));
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].host, "id2.mc.example.com");
        assert!(w.iter().any(|l| l.contains("max_entries")));
    }

    #[test]
    fn missing_pagination_is_an_error() {
        let body = LIST.replace("\"pagination\"", "\"other\"");
        assert!(parse_page(body.as_bytes())
            .unwrap_err()
            .contains("pagination"));
        let body = r#"{"object":"list","data":[]}"#;
        assert!(parse_page(body.as_bytes()).is_err());
    }

    #[test]
    fn hosts_the_runtime_would_refuse_are_dropped() {
        let long = "a".repeat(64);
        let (e, w) = build_routes(
            &[server(1, &long, "10.0.0.1"), server(2, "ok", "10.0.0.2")],
            &cfg("{name}.mc.example.com"),
            &limits(10),
        );
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].host, "ok.mc.example.com");
        assert!(w[0].contains("not a valid hostname"));
    }

    #[test]
    fn uppercase_template_literals_are_lowercased() {
        let (e, _) = build_routes(
            &[server(1, "a", "10.0.0.1")],
            &cfg("{identifier}.MC.Example.com"),
            &limits(10),
        );
        assert_eq!(e[0].host, "id1.mc.example.com");
    }

    #[test]
    fn port_zero_is_dropped() {
        let mut s = server(1, "a", "10.0.0.1");
        s.allocations[0].port = 0;
        let (e, w) = build_routes(&[s], &cfg("{identifier}.mc.example.com"), &limits(10));
        assert!(e.is_empty());
        assert!(w[0].contains("port 0"));
    }

    #[test]
    fn servers_with_a_status_are_skipped() {
        let mut s = server(1, "a", "10.0.0.1");
        s.status = Some("installing".into());
        let (e, _) = build_routes(&[s], &cfg("{identifier}.mc.example.com"), &limits(10));
        assert!(e.is_empty());
    }

    #[test]
    fn parses_status() {
        let body = LIST.replace("\"status\":null", "\"status\":\"install_failed\"");
        assert_eq!(
            parse_page(body.as_bytes()).unwrap().servers[0]
                .status
                .as_deref(),
            Some("install_failed")
        );
    }

    #[test]
    fn page_urls() {
        assert_eq!(
            page_url("https://panel.example/", 2),
            "https://panel.example/api/application/servers?include=allocations&per_page=100&page=2"
        );
    }
}

/// The `http_request` document for one authenticated GET. The token is a secret slot the
/// host expands into the header; it never reaches the guest.
pub fn request_doc(url: &str) -> String {
    serde_json::json!({
        "method": "GET",
        "url": url,
        "headers": {
            "Authorization": "Bearer ${secret:PELICAN_TOKEN}",
            "Accept": "application/json",
        },
    })
    .to_string()
}

/// The body of a `http_read` document, or why the call failed.
pub fn response_body(doc: &[u8]) -> Result<Vec<u8>, String> {
    #[derive(Deserialize)]
    struct Doc {
        status: Option<u16>,
        body: Option<String>,
        error: Option<String>,
    }
    let d: Doc = serde_json::from_slice(doc).map_err(|e| format!("bad http response: {e}"))?;
    if let Some(e) = d.error {
        return Err(format!("http request failed: {e}"));
    }
    match d.status {
        Some(200) => crate::b64::decode(&d.body.unwrap_or_default())
            .ok_or_else(|| "response body is not base64".to_string()),
        Some(s) => Err(format!("panel answered {s}")),
        None => Err("http response has no status".to_string()),
    }
}

/// Reads every page through `fetch` (URL in, response body out) and builds the routes.
/// Errors, publishing nothing, when a page fails or the list is longer than [`MAX_PAGES`]:
/// a partial set would withdraw the routes of the servers it did not see.
///
/// The pages are read one after another, so a server created or deleted in between shifts the
/// later pages. When the panel's total differs between two pages the whole listing is read once
/// more; a second mismatch is an error and nothing is published. (A create and a delete between
/// two reads leave the total unchanged and are not noticed; the next tick converges.)
pub fn plan(
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, String>,
    cfg: &Config,
    limits: &RouteLimits,
) -> Result<(Vec<Entry>, Vec<String>), String> {
    for _ in 0..2 {
        if let Some(servers) = read_all(&mut fetch, cfg)? {
            return Ok(build_routes(&servers, cfg, limits));
        }
    }
    Err("the server list changed while it was being read, twice; routes left as they were".into())
}

/// All servers, or `None` when the panel's total changed between two pages.
fn read_all(
    fetch: &mut impl FnMut(&str) -> Result<Vec<u8>, String>,
    cfg: &Config,
) -> Result<Option<Vec<Server>>, String> {
    let mut servers = Vec::new();
    let mut first_total = None;
    let mut page = 1;
    loop {
        let p = parse_page(&fetch(&page_url(&cfg.panel_url, page))?)?;
        if p.total_pages > MAX_PAGES {
            return Err(format!(
                "the panel has {} pages of servers, more than the {MAX_PAGES} one tick can read",
                p.total_pages
            ));
        }
        if *first_total.get_or_insert(p.total) != p.total {
            return Ok(None);
        }
        servers.extend(p.servers);
        if page >= p.total_pages {
            break;
        }
        page += 1;
    }
    Ok(Some(servers))
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn page(servers: &[(u64, &str)], total_pages: u32) -> Vec<u8> {
        page_of(servers, total_pages, servers.len())
    }

    /// A page of a listing the panel says holds `total` servers.
    fn page_of(servers: &[(u64, &str)], total_pages: u32, total: usize) -> Vec<u8> {
        let data: Vec<String> = servers
            .iter()
            .map(|(id, ip)| {
                format!(
                    r#"{{"object":"server","attributes":{{"id":{id},"external_id":null,"identifier":"id{id}","name":"n{id}","suspended":false,"node":1,"allocation":{id},
                    "relationships":{{"allocations":{{"data":[{{"object":"allocation","attributes":{{"id":{id},"ip":"{ip}","port":25565}}}}]}}}}}}}}"#
                )
            })
            .collect();
        format!(
            r#"{{"object":"list","data":[{}],"meta":{{"pagination":{{"total":{total},"total_pages":{total_pages}}}}}}}"#,
            data.join(",")
        )
        .into_bytes()
    }

    fn cfg() -> Config {
        Config::parse(br#"{"panel_url":"https://panel.example","host_template":"{identifier}.mc.example.com"}"#).unwrap()
    }

    fn limits() -> RouteLimits {
        RouteLimits::from_caps_json(r#"{"routes":{"hosts":["*.mc.example.com"],"backends":["10.0.0.0/8"],"max_entries":10}}"#).unwrap()
    }

    #[test]
    fn request_doc_carries_the_secret_placeholder_only() {
        let d = request_doc("https://panel.example/x");
        let v: serde_json::Value = serde_json::from_str(&d).unwrap();
        assert_eq!(v["method"], "GET");
        assert_eq!(v["url"], "https://panel.example/x");
        assert_eq!(
            v["headers"]["Authorization"],
            "Bearer ${secret:PELICAN_TOKEN}"
        );
    }

    #[test]
    fn response_body_decodes_a_200() {
        let doc = br#"{"status":200,"headers":{},"body":"aGk="}"#;
        assert_eq!(response_body(doc).unwrap(), b"hi");
    }

    #[test]
    fn response_body_reports_errors_and_bad_status() {
        assert!(response_body(br#"{"error":"nope"}"#)
            .unwrap_err()
            .contains("nope"));
        let e = response_body(br#"{"status":401,"headers":{},"body":""}"#).unwrap_err();
        assert!(e.contains("401"));
    }

    #[test]
    fn plan_reads_all_pages() {
        let mut urls = Vec::new();
        let (e, _) = plan(
            |u| {
                urls.push(u.to_string());
                Ok(if u.ends_with("page=1") {
                    page(&[(1, "10.0.0.1")], 2)
                } else {
                    page(&[(2, "10.0.0.2")], 2)
                })
            },
            &cfg(),
            &limits(),
        )
        .unwrap();
        assert_eq!(urls.len(), 2);
        assert_eq!(e.len(), 2);
    }

    #[test]
    fn repeated_and_out_of_order_reconciles_converge_on_the_latest_panel_state() {
        let states = [
            page(&[(1, "10.0.0.1")], 1),
            page(&[(1, "10.0.0.1"), (2, "10.0.0.2")], 1),
            page(&[(2, "10.0.0.2")], 1),
        ];
        let run = |i: usize| {
            plan(|_| Ok(states[i].clone()), &cfg(), &limits())
                .unwrap()
                .0
        };
        // Only the pure planning step: the host's own serialization and queue bound are tested
        // in wayhouse. Calls arrive repeated and out of order; each is a full re-list, so the outcome is
        // decided by the last panel state read and never by how often or in what order.
        for i in [0, 2, 1, 1, 0, 2, 2] {
            let _ = run(i);
        }
        assert_eq!(run(2), run(2));
        assert_eq!(run(2).len(), 1);
        assert_eq!(run(1).len(), 2);
    }

    #[test]
    fn a_listing_that_changed_between_pages_is_read_again() {
        let mut calls = 0;
        let (e, _) = plan(
            |u| {
                calls += 1;
                // First read: page 2 sees one more server than page 1 did.
                Ok(match (calls, u.ends_with("page=1")) {
                    (1, _) => page_of(&[(1, "10.0.0.1")], 2, 2),
                    (2, _) => page_of(&[(2, "10.0.0.2")], 2, 3),
                    (_, true) => page_of(&[(1, "10.0.0.1")], 2, 3),
                    (_, false) => page_of(&[(2, "10.0.0.2"), (3, "10.0.0.3")], 2, 3),
                })
            },
            &cfg(),
            &limits(),
        )
        .unwrap();
        assert_eq!(calls, 4);
        assert_eq!(e.len(), 3);
    }

    #[test]
    fn a_listing_that_keeps_changing_publishes_nothing() {
        let mut calls = 0;
        let r = plan(
            |u| {
                calls += 1;
                Ok(if u.ends_with("page=1") {
                    page_of(&[(1, "10.0.0.1")], 2, calls)
                } else {
                    page_of(&[(2, "10.0.0.2")], 2, calls + 10)
                })
            },
            &cfg(),
            &limits(),
        );
        assert!(r.unwrap_err().contains("changed while"));
        assert_eq!(calls, 4, "one retry, then give up");
    }

    #[test]
    fn plan_refuses_a_partial_view() {
        let r = plan(
            |_| Ok(page(&[(1, "10.0.0.1")], MAX_PAGES + 1)),
            &cfg(),
            &limits(),
        );
        assert!(r.unwrap_err().contains("pages"));
        let r = plan(|_| Err("boom".into()), &cfg(), &limits());
        assert!(r.unwrap_err().contains("boom"));
    }
}
