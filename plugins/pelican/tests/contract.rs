//! The plugin against recorded Pelican Application API responses (see `tests/fixtures/README.md`
//! for where they come from).

use pelican::{caps::RouteLimits, panel};

const LIST: &[u8] = include_bytes!("fixtures/servers_list.json");
const NO_ALLOCATIONS: &[u8] = include_bytes!("fixtures/servers_list_key_without_allocations.json");

fn cfg() -> panel::Config {
    panel::Config::parse(
        br#"{"panel_url":"https://panel.example","host_template":"{identifier}.mc.example.com","node_addresses":{"2":"10.0.0.6"}}"#,
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
