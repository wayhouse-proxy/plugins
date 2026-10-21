# pelican

Routes hostnames to the game servers of a [Pelican](https://pelican.dev) panel through wayhouse. (Pelican has plugins of its own; this is a *wayhouse* plugin.)

**One entry node.** Point a wildcard DNS record (for example `*.mc.example.com`) at the wayhouse entry node. Every minute the plugin lists the panel's servers and publishes `hostname -> node ip:port` routes. Adding, moving or deleting a Pelican server moves the route on the next tick: no per-host DNS or firewall edit.

## What it does

Each tick: `GET /api/application/servers?include=allocations` (up to 8 pages of 100), then for every server that is not suspended, has no `status` (null means installed and usable) and has a primary allocation: hostname from `host_template`, backend from the primary allocation's `ip:port`. Then one `routes_set` call with the whole set. Operator routes always win over plugin routes.

- A list page without `meta.pagination.total_pages` is an error, never "one page".
- If the panel cannot be read, answers with an error, or has more than 8 pages, nothing is published and the previous routes stay (a partial set would withdraw other servers' routes).
- Entries outside the plugin's own declared `routes` (host not covered, backend outside the networks, over `max_entries`) are dropped with a warning in the plugin log, because the host would otherwise refuse the whole set.
- Two servers rendering to the same hostname: the lower server id wins, the other is logged.
- Only the Application API's read endpoints are called. The API key is never visible to the plugin: it is a secret slot the host expands into the `Authorization` header.

## Config (the install's `config`, non-secret)

```json
{
  "panel_url": "https://panel.example",
  "host_template": "{identifier}.mc.example.com",
  "node_addresses": { "1": "10.0.0.5" }
}
```

- `host_template`: `{identifier}` (Pelican's short uuid), `{id}`, `{name}` and `{external_id}` (both lower-cased, non-alphanumerics folded to `-`). A server with no value for a used placeholder is skipped.
- `node_addresses`: node id to the address the entry node reaches that node at. It wins over the allocation's ip, and is required when the allocation is bound to `0.0.0.0`.

Secret slot `PELICAN_TOKEN`: an Application API key. Give it read access to servers and allocations only (Pelican keys have per-resource levels and an IP allowlist). Set it with `PUT /plugins/{id}/secrets/PELICAN_TOKEN`.

## Build it for your deployment

A module's capability declaration is embedded in the module and read without running it, so the panel host, the hostnames the plugin may claim and the backend networks cannot be set at install time (tracked upstream). Copy [`caps.json`](caps.json), edit `http.hosts`, `secrets[0].hosts`, `routes.hosts`, `routes.backends` and `routes.max_entries`, and build with it:

```sh
WAYHOUSE_PELICAN_CAPS=/path/to/my-caps.json cargo build --release --target wasm32-unknown-unknown -p pelican
```

The checked-in `caps.json` uses placeholders (`panel.example`, `*.mc.example.com`, `10.0.0.0/8`) and is what CI builds.

## Not built yet

Pelican webhooks (`on_webhook`) and events: the plugin polls. Which Pelican webhook events fire for server create, delete and transfer is still unverified against a live panel, and the poll already converges within one interval.

Unverified against a live panel: the exact lowest API-key permission level that allows reading servers and allocations, and whether a panel behind a proxy leaves `allocation` and `relationships.allocations` as read from the source (`app/Data/Api/Application/ServerData.php`, `AllocationData.php`, `Envelope.php`).
