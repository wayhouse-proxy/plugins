//! The plugin against recorded Pelican Application API responses (see `tests/fixtures/README.md`
//! for where they come from).

use pelican::{
    caps::{PortLimits, RouteLimits},
    panel,
};

const LIST: &[u8] = include_bytes!("fixtures/servers_list.json");
const NO_ALLOCATIONS: &[u8] = include_bytes!("fixtures/servers_list_key_without_allocations.json");

fn cfg() -> panel::Config {
    panel::Config::parse(
        br#"{"panel_host":"panel.example","route_host":"*.mc.example.com","host_template":"{identifier}.mc.example.com","node_addresses":{"2":"10.0.0.6"}}"#,
    )
    .unwrap()
}

fn limits() -> RouteLimits {
    RouteLimits::from_caps_json(
        r#"{"routes":{"hosts":["*.mc.example.com"],"backends":["10.0.0.0/8"],"max_entries":10}}"#,
    )
    .unwrap()
}

#[test]
fn the_list_parses_with_pagination_and_allocations() {
    let page = panel::parse_page(LIST).unwrap();
    assert_eq!(page.total_pages, 1);
    assert_eq!(page.servers.len(), 8);
    assert_eq!(page.servers[0].allocations.len(), 2);
}

#[test]
fn the_primary_allocation_is_the_one_named_by_allocation_not_the_first() {
    let page = panel::parse_page(LIST).unwrap();
    let (entries, _) = panel::build_routes(&page.servers, &cfg(), &limits());
    let survival = entries
        .iter()
        .find(|e| e.host == "a1b2c3d4.mc.example.com")
        .unwrap();
    assert_eq!(survival.backend, "10.0.0.5:25565");
}

#[test]
fn every_non_null_status_is_skipped_and_only_usable_servers_route() {
    let page = panel::parse_page(LIST).unwrap();
    let statuses: Vec<_> = page
        .servers
        .iter()
        .filter_map(|s| s.status.as_deref())
        .collect();
    assert_eq!(
        statuses,
        [
            "suspended",
            "installing",
            "install_failed",
            "reinstall_failed",
            "restoring_backup"
        ]
    );
    let (entries, _) = panel::build_routes(&page.servers, &cfg(), &limits());
    let hosts: Vec<_> = entries.iter().map(|e| e.host.as_str()).collect();
    assert_eq!(
        hosts,
        ["a1b2c3d4.mc.example.com", "e5f6a7b8.mc.example.com"]
    );
}

#[test]
fn a_wildcard_bound_allocation_uses_the_node_address() {
    let page = panel::parse_page(LIST).unwrap();
    let (entries, _) = panel::build_routes(&page.servers, &cfg(), &limits());
    let creative = entries
        .iter()
        .find(|e| e.host == "e5f6a7b8.mc.example.com")
        .unwrap();
    assert_eq!(creative.backend, "10.0.0.6:25565");
}

#[test]
fn a_key_without_allocation_access_is_an_error_not_an_empty_route_set() {
    let err = panel::parse_page(NO_ALLOCATIONS).unwrap_err();
    assert!(err.contains("Allocations"), "{err}");
}

// Captured from a live panel (anonymised); see tests/fixtures/README.md.
const LIVE: &[u8] = include_bytes!("fixtures/servers_list_live.json");
const LIVE_NODES: &[u8] = include_bytes!("fixtures/nodes_live.json");

fn live_cfg() -> panel::Config {
    panel::Config::parse(
        br#"{"panel_host":"panel.example","route_host":"*.mc.example.com","host_template":"{identifier}.mc.example.com"}"#,
    )
    .unwrap()
}

fn live_limits() -> RouteLimits {
    RouteLimits::from_caps_json(
        r#"{"routes":{"hosts":["*.mc.example.com"],"backends":["192.0.2.0/24"],"max_entries":10}}"#,
    )
    .unwrap()
}

#[test]
fn a_live_panel_list_parses_and_every_usable_server_routes() {
    let page = panel::parse_page(LIVE).unwrap();
    assert_eq!(page.total_pages, 1);
    assert_eq!(page.servers.len(), 3);
    let (entries, warnings) = panel::build_routes(&page.servers, &live_cfg(), &live_limits());
    assert_eq!(entries.len(), 3, "{warnings:?}");
}

#[test]
fn live_primary_allocation_is_the_one_allocation_names_even_when_listed_second() {
    let page = panel::parse_page(LIVE).unwrap();
    let s = page.servers.iter().find(|s| s.id == 5).unwrap();
    assert_eq!(s.allocation, Some(13));
    assert_eq!(
        s.allocations[0].id, 12,
        "the primary is not the first listed"
    );
    let (entries, _) = panel::build_routes(&page.servers, &live_cfg(), &live_limits());
    let e = entries
        .iter()
        .find(|e| e.host == "00000005.mc.example.com")
        .unwrap();
    assert!(e.backend.ends_with(":25565"), "{e:?}");
}

#[test]
fn live_node_fqdn_is_a_dns_name_so_node_addresses_stays_config() {
    let v: serde_json::Value = serde_json::from_slice(LIVE_NODES).unwrap();
    let nodes = v["data"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    for node in nodes {
        let fqdn = node["attributes"]["fqdn"].as_str().unwrap();
        assert!(fqdn.parse::<std::net::IpAddr>().is_err(), "{fqdn}");
    }
}

#[test]
fn live_allocation_alias_is_ignored_and_the_backend_is_the_allocation_ip_and_port() {
    let v: serde_json::Value = serde_json::from_slice(LIVE).unwrap();
    let aliased = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| {
            s["attributes"]["relationships"]["allocations"]["data"]
                .as_array()
                .unwrap()
        })
        .filter(|a| a["attributes"]["alias"].is_string())
        .count();
    assert!(aliased > 0, "the capture should hold aliased allocations");

    let page = panel::parse_page(LIVE).unwrap();
    let (entries, _) = panel::build_routes(&page.servers, &live_cfg(), &live_limits());
    assert_eq!(entries.len(), page.servers.len());
    for server in &page.servers {
        let primary = server
            .allocations
            .iter()
            .find(|a| Some(a.id) == server.allocation)
            .expect("the primary allocation is listed");
        let host = format!("{}.mc.example.com", server.identifier);
        let entry = entries.iter().find(|e| e.host == host).unwrap();
        assert_eq!(entry.backend, format!("{}:{}", primary.ip, primary.port));
        assert!(!entry.backend.contains("alias"), "{entry:?}");
    }
}

fn port_limits(proto: &str) -> PortLimits {
    PortLimits::from_caps_json(&format!(
        r#"{{"port_routes":{{"ports":["25000-25999"],"proto":{proto},"backends":["10.0.0.0/8"],"max_entries":10}}}}"#
    ))
    .unwrap()
    .unwrap()
}

#[test]
fn port_routes_key_on_the_server_uuid_and_hint_the_allocation_port() {
    let page = panel::parse_page(LIST).unwrap();
    let (entries, warnings) =
        panel::build_port_routes(&page.servers, &cfg(), &port_limits(r#"["udp","tcp"]"#));
    let keys: Vec<_> = entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "a1b2c3d4-0000-4000-8000-000000000000",
            "e5f6a7b8-0000-4000-8000-000000000000"
        ],
        "only usable servers, in server id order: {warnings:?}"
    );
    let survival = &entries[0];
    assert_eq!(survival.backend, "10.0.0.5:25565");
    assert_eq!(survival.proto, panel::PortProto::Both);
    assert_eq!(survival.prefer, Some(25565));
    // A wildcard-bound allocation goes to the configured node address, as for hostnames.
    assert_eq!(entries[1].backend, "10.0.0.6:25565");
    let doc = serde_json::to_value(survival).unwrap();
    assert_eq!(
        doc,
        serde_json::json!({
            "key": "a1b2c3d4-0000-4000-8000-000000000000",
            "backend": "10.0.0.5:25565",
            "proto": "both",
            "prefer": 25565
        })
    );
}

#[test]
fn an_approval_for_one_protocol_publishes_that_protocol() {
    let page = panel::parse_page(LIST).unwrap();
    for (proto, want) in [
        (r#"["udp"]"#, panel::PortProto::Udp),
        (r#"["tcp"]"#, panel::PortProto::Tcp),
    ] {
        let (entries, _) = panel::build_port_routes(&page.servers, &cfg(), &port_limits(proto));
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e.proto == want), "{proto}");
    }
}

#[test]
fn the_hint_is_left_out_when_the_allocation_port_is_not_an_approved_entry_port() {
    let page = panel::parse_page(LIST).unwrap();
    let narrow = PortLimits::from_caps_json(
        r#"{"port_routes":{"ports":["30000-30999"],"proto":["udp"],"backends":["10.0.0.0/8"],"max_entries":10}}"#,
    )
    .unwrap()
    .unwrap();
    let (entries, _) = panel::build_port_routes(&page.servers, &cfg(), &narrow);
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|e| e.prefer.is_none()));
    let doc = serde_json::to_value(&entries[0]).unwrap();
    assert!(doc.get("prefer").is_none(), "{doc}");
}

#[test]
fn backends_outside_the_port_route_networks_or_backend_ports_are_dropped_and_the_cap_holds() {
    let page = panel::parse_page(LIST).unwrap();
    let elsewhere = PortLimits::from_caps_json(
        r#"{"port_routes":{"ports":["25000-25999"],"proto":["udp"],"backends":["192.168.0.0/16"],"max_entries":10}}"#,
    )
    .unwrap()
    .unwrap();
    let (entries, warnings) = panel::build_port_routes(&page.servers, &cfg(), &elsewhere);
    assert!(entries.is_empty());
    assert!(
        warnings.iter().any(|w| w.contains("port_routes.backends")),
        "{warnings:?}"
    );

    let ported = panel::Config::parse(
        br#"{"panel_host":"panel.example","route_host":"*.mc.example.com","host_template":"{identifier}.mc.example.com","node_addresses":{"2":"10.0.0.6"},"backend_ports":"26000"}"#,
    )
    .unwrap();
    let (entries, _) = panel::build_port_routes(&page.servers, &ported, &port_limits(r#"["udp"]"#));
    assert!(entries.is_empty(), "outside backend_ports");

    let one = PortLimits::from_caps_json(
        r#"{"port_routes":{"ports":["25000-25999"],"proto":["udp"],"backends":["10.0.0.0/8"],"max_entries":1}}"#,
    )
    .unwrap()
    .unwrap();
    let (entries, warnings) = panel::build_port_routes(&page.servers, &cfg(), &one);
    assert_eq!(entries.len(), 1);
    assert!(
        warnings.iter().any(|w| w.contains("max_entries")),
        "{warnings:?}"
    );
}

#[test]
fn a_caps_document_without_port_routes_means_no_port_routes() {
    assert!(PortLimits::from_caps_json(r#"{"log":true}"#)
        .unwrap()
        .is_none());
    assert!(PortLimits::from_caps_json(r#"{"port_routes":{"ports":["x"],"proto":["udp"],"backends":["10.0.0.0/8"],"max_entries":1}}"#).is_err());
}
