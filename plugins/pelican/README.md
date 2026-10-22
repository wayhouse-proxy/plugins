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

Secret slot `PELICAN_TOKEN`: an Application API key. Give it **Read** on *Servers* and *Allocations* and nothing else (Pelican keys have per-resource levels and an IP allowlist). Without *Allocations* the panel does not refuse the request: it answers the include as a `null_resource`, and the plugin reports that as an error and keeps the previous routes. Set it with `PUT /plugins/{id}/secrets/PELICAN_TOKEN`.

## Build it for your deployment

A module's capability declaration is embedded in the module and read without running it, so the panel host, the hostnames the plugin may claim and the backend networks cannot be set at install time (tracked upstream). Copy [`caps.json`](caps.json), edit `http.hosts`, `secrets[0].hosts`, `routes.hosts`, `routes.backends` and `routes.max_entries`, and build with it:

```sh
WAYHOUSE_PELICAN_CAPS=/path/to/my-caps.json cargo build --release --target wasm32-unknown-unknown -p pelican
```

The checked-in `caps.json` uses placeholders (`panel.example`, `*.mc.example.com`, `10.0.0.0/8`) and is what CI builds.

## Operator guide: one entry node

1. **Wildcard DNS.** Point `*.mc.example.com` (the domain in `host_template`) at the wayhouse entry node. This is the only DNS record; new servers need none.
2. **Reachability.** The entry node must be able to open a TCP connection to every Pelican node's allocation `ip:port`, over a private network or a wayhouse tunnel. Pelican already needs those game ports open on its nodes, so the plugin adds no firewall rule; "no firewall edits" only holds if the entry node is allowed to reach the nodes from the start. Where an allocation is bound to `0.0.0.0` or a public address the entry node should not use, set `node_addresses`.
3. **Hostname-only routing.** Routes are keyed by hostname, so this works for protocols that carry one: HTTP, TLS (SNI) and games a sniffer can read it from (for example Minecraft Java). UDP games without a hostname (Source, Valheim and similar) cannot be routed this way; port-based routing is tracked in wayhouse#403.
4. **API key.** In the panel create an Application API key with **Read** on *Servers* and *Allocations* (see above).
5. **Build and install the plugin** with your own `caps.json` (see above), then install it with the config above and set the secret: `PUT /plugins/{id}/secrets/PELICAN_TOKEN`. Approve the declared capabilities. Updating an install from a build without `on_webhook` to one with it needs that new trigger approved again.
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

Checked against Pelican's source (panel commit `b7ea6d2`), **not against a running panel yet** (wayhouse-proxy/plugins#11): the response shapes in `tests/fixtures/` are written from `ServerData.php`, `AllocationData.php`, `Envelope.php`, `IncludeContext.php` and `PanelResponse.php`.

- `allocation` is the id of the server's primary (default) allocation; `relationships.allocations` lists all of them, the primary is the one `allocation` names. The pagination block is `meta.pagination` with `total_pages` (`lastPage()`, so 1 for an empty list).
- `status` is `null` or one of `installing`, `install_failed`, `reinstall_failed`, `suspended`, `restoring_backup`. Skipping every non-null value is right: none of them can serve players, and `suspended` also sets the deprecated `suspended` flag.
- Key permissions: the servers endpoint needs Read on *Servers*; the allocations include needs Read on *Allocations* and is otherwise returned as `null_resource`.
- Node address: `GET /api/application/nodes` returns a node `fqdn`, a DNS name for the panel-to-Wings link, not necessarily an address the entry node can use, and the plugin cannot resolve DNS. `node_addresses` therefore stays configuration.
- Webhooks: a configured header is sent as stored (`Http::withHeaders`), the body is a JSON POST with a 30 s timeout. Event names are `eloquent.{created,updated,deleted}: App\Models\<Model>`.
