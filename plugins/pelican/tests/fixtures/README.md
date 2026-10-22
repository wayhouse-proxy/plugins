# Fixtures

**Live captures (2026-10-22, anonymised):** `servers_list_live.json` and `nodes_live.json` are responses of `GET /api/application/servers?include=allocations&per_page=100` and `GET /api/application/nodes` from a running Pelican panel. Names, uuids, identifiers, IPs, aliases and node hostnames are replaced by placeholders; ids, ports, structure and the pagination block are as returned. All three servers had `status: null`.

The files below are not from a panel.

Responses of `GET /api/application/servers?include=allocations`, written from Pelican's source
(panel commit `b7ea6d2`: `app/Data/Api/Application/ServerData.php`, `AllocationData.php`,
`app/Data/Api/Envelope.php`, `IncludeContext.php`, `PanelResponse.php`, `app/Enums/ServerState.php`).
They are **not captured from a running panel**: nothing here claims more than the source says.
Replace them with captures from a live panel when one is available (wayhouse-proxy/plugins#11).

- `servers_list.json`: one server per `ServerState` value, a server with two allocations whose
  primary is not the first, an allocation bound to `0.0.0.0`, and a server without a primary.
- `servers_list_key_without_allocations.json`: what a key without *Allocations* read access gets:
  the include is not refused, it comes back as `{"object": "null_resource", "attributes": null}`.
