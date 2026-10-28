# pelican

Routes hostnames to the game servers of a [Pelican](https://pelican.dev) panel through wayhouse. (Pelican has plugins of its own; this is a *wayhouse* plugin.)

**One entry node.** Point a wildcard DNS record (for example `*.mc.example.com`) at the wayhouse entry node. Every minute the plugin lists the panel's servers and publishes `hostname -> node ip:port` routes. Adding, moving or deleting a Pelican server moves the route on the next tick: no per-host DNS or firewall edit.

## What it does

Each tick: `GET /api/application/servers?include=allocations` (up to 8 pages of 100), then for every server that is not suspended, has no `status` (null means installed and usable) and has a primary allocation: hostname from `host_template`, backend from the primary allocation's `ip:port`. Then one `routes_set` call with the whole set. Operator routes always win over plugin routes.

- A list page without `meta.pagination.total_pages` is an error, never "one page".
- If the panel cannot be read, answers with an error, or has more than 8 pages, nothing is published and the previous routes stay (a partial set would withdraw other servers' routes).
- If the panel's server total differs between two pages (a server was created or deleted while the pages were being read), the whole list is read once more; a second mismatch publishes nothing. A create and a delete in the same instant keep the total and are not noticed; the next tick corrects it.
- Entries outside the plugin's own declared `routes` (host not covered, backend outside the networks, over `max_entries`) are dropped with a warning in the plugin log, because the host would otherwise refuse the whole set.
- Two servers rendering to the same hostname: the lower server id wins, the other is logged.
- Only the Application API's read endpoints are called. The API key is never visible to the plugin: it is a secret slot the host expands into the `Authorization` header.

## Config (the install's `config`, non-secret)

```json
{
  "panel_host": "panel.example",
  "route_host": "*.mc.example.com",
  "host_template": "{identifier}.mc.example.com",
  "node_addresses": { "1": "10.0.0.5" },
  "backend_ports": "25565-25999"
}
```

- `host_template`: `{identifier}` (Pelican's short uuid), `{id}`, `{name}` and `{external_id}` (both lower-cased, non-alphanumerics folded to `-`). A server with no value for a used placeholder is skipped.
- `node_addresses`: node id to the address the entry node reaches that node at. It wins over the allocation's ip, and is required when the allocation is bound to `0.0.0.0`.
- `backend_ports` (optional): the ports a route may point at, `"lo-hi"` or one port. A server whose primary allocation is outside it is skipped with a log line. Set it: the `routes.backends` capability limits addresses, **not ports**, so without it an allocation on port 22 of an allowed host would be routed.

Secret slot `PELICAN_TOKEN`: an Application API key. Give it **Read** on *Servers* and *Allocations* and nothing else (Pelican keys have per-resource levels and an IP allowlist). Without *Allocations* the panel does not refuse the request: it answers the include as a `null_resource`, and the plugin reports that as an error and keeps the previous routes. Set it with `PUT /plugins/{id}/secrets/PELICAN_TOKEN`.

## Install options

The module declares install options (ABI 0.2), so one prebuilt module fits any deployment: the values below are the install `config`, and the host turns `panel_host`, `route_host`, `node_cidr` and `entry_ports` into the capabilities the operator approves. Nothing is built per deployment any more.

- `panel_host`: hostname of the panel. The plugin may call this host (HTTPS, port 443) and the `PELICAN_TOKEN` secret is bound to it.
- `route_host`: the hostnames the plugin may route, an exact name or `*.suffix`; cover the domain in `host_template`.
- `node_cidr` (default `10.0.0.0/8`): the network the nodes' addresses must be in, for routes and port routes.
- `entry_ports` (default `30000-30999`): the entry-port range wayhouse may assign to port routes.

## Operator guide: one entry node

1. **Wildcard DNS.** Point `*.mc.example.com` (the domain in `host_template`) at the wayhouse entry node. This is the only DNS record; new servers need none.
2. **Reachability.** The entry node must be able to open a TCP connection to every Pelican node's allocation `ip:port`, over a private network or a wayhouse tunnel. Pelican already needs those game ports open on its nodes, so the plugin adds no firewall rule; "no firewall edits" only holds if the entry node is allowed to reach the nodes from the start. Where an allocation is bound to `0.0.0.0` or a public address the entry node should not use, set `node_addresses`.
3. **Hostname routing and entry ports.** Routes keyed by hostname work for protocols that carry one: HTTP, TLS (SNI) and games a sniffer can read it from (for example Minecraft Java). Games without a hostname (Source, Valheim and similar, UDP included) are routed by entry port instead: when the module declares `port_routes` (it does), the plugin publishes one port route per usable server, keyed by the server's uuid, and wayhouse picks the entry port (see "Port routes" in the wayhouse plugin docs). The port stays with the server, also when its backend moves, and is shown first in the Plugins page. Open the entry range once: the listener's `bind:` range (with `plugin_routes: { type: port }`) and the capability's `ports` must cover it. The plugin hints the allocation port as the preferred entry port, but only when it lies inside the approved `ports`; otherwise wayhouse picks one. A port freed by a deleted server is quarantined for 24 h.
4. **API key.** In the panel create an Application API key with **Read** on *Servers* and *Allocations* (see above).
5. **Install the plugin** with the config above and set the secret: `PUT /plugins/{id}/secrets/PELICAN_TOKEN`. Approve the effective capabilities the upload shows for your config. Updating an install from a build without `on_webhook` to one with it needs that new trigger approved again.
6. **`host_template`.** Prefer `{identifier}` or `{id}`: they are always set and never change. `{name}` and `{external_id}` change when someone renames a server, and a server without a value for a used placeholder gets no route.
7. **Limits of one poll.** At most 8 pages of 100 servers (800) are read, and a response is capped at 1 MiB. A panel with more than that, or a response over the cap, publishes nothing and keeps the previous routes (the plugin logs why), so a very large panel needs a plugin change before it can use this. The whole reconcile, up to 8 panel requests, must also fit the host's time limit for one plugin call, on the webhook path as on the timer; a slow panel can make a call fail (webhook: `502`).

### Optional: webhooks for instant updates

Without this the poll converges within a minute. With it, a Pelican event triggers a reconcile at once. The webhook is only a **hint**: the plugin ignores the payload, re-lists the servers and publishes the result, so missed, repeated or reordered events are harmless. `on_timer` keeps running either way. The listener is a second exposed port on the entry node, which is why it is optional.

1. Start the controller with the webhook listener (off by default, its own bind and TLS): `--plugin-webhook-listen 0.0.0.0:8444 --plugin-webhook-tls-cert hooks.pem --plugin-webhook-tls-key hooks.key`.
2. Enable the install's webhook: `POST /plugins/{id}/webhook` answers `{"token": "whk_...", "path": "/plugins/{id}/hook"}`. The token is shown once; calling it again rotates it.
3. In the panel's admin area create a webhook (type *Regular*) with the endpoint `https://<entry node>:8444/plugins/{id}/hook`, a header `Authorization` with the value `Bearer whk_...`, and the events `eloquent.created: App\Models\Server`, `eloquent.updated: App\Models\Server` and `eloquent.deleted: App\Models\Server` (add the `Allocation` ones if you move allocations around). A transfer changes the server's node and allocation, which fires the `updated` event.
4. The plugin answers `202` when the reconcile published and `502` when it did not, so Pelican's delivery log for the webhook shows failures. With the *Regular* webhook type Pelican's source (`BaseSchema::retryAfter`) asks for no retry, so a failed delivery is not repeated; the next poll fixes it.
5. Bursts: the host runs one call per install at a time and queues at most 4, so an event flood costs a few reads of the panel and an unchanged route set is not committed again.

## What is verified, and how

**Against a live panel** (2026-10-22, a running Pelican panel with two nodes and three servers; anonymised captures in `tests/fixtures/*_live.json`, tested in `tests/contract.rs`):

- The list is `{object: "list", data: [{object: "server", attributes: {...}}], meta: {pagination: {total_pages, ...}}}`, and `?include=allocations` nests `attributes.relationships.allocations` as a list. The plugin parses this and routes every usable server.
- `allocation` is the id of the primary allocation, and it is not necessarily the first one listed (a server with allocations 12 and 13 had primary 13). An allocation's `alias` can hold a hostname.
- A node's `fqdn` is a DNS name (the panel's link to Wings), not an address the entry node can use, and the plugin cannot resolve DNS, so `node_addresses` stays configuration.
- A key with Read on *Servers*, *Allocations* and *Nodes* works for both endpoints.

**Against Pelican's source only** (panel commit `b7ea6d2`; fixtures `servers_list.json` and `servers_list_key_without_allocations.json` are written from `ServerData.php`, `AllocationData.php`, `Envelope.php`, `IncludeContext.php`, `PanelResponse.php`, `ServerState.php`, `ProcessWebhook.php`). Not seen on a live panel:

- `status` is `null` or one of `installing`, `install_failed`, `reinstall_failed`, `suspended`, `restoring_backup`. Skipping every non-null value is right: none can serve players. (The live servers all had `null`.)
- A key without Read on *Allocations* gets the include as a `null_resource`; the plugin reports an error and keeps the previous routes. The least privilege for the plugin is Read on *Servers* and *Allocations*; *Nodes* is not needed.
- Webhooks: a configured header is sent as stored, as a JSON POST with a 30 s timeout. Event names are `eloquent.{created,updated,deleted}: App\Models\<Model>`. Which events a transfer fires has not been observed.
