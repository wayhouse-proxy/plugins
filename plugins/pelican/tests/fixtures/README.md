# Fixtures

Responses of `GET /api/application/servers?include=allocations`, written from Pelican's source
(panel commit `b7ea6d2`: `app/Data/Api/Application/ServerData.php`, `AllocationData.php`,
`app/Data/Api/Envelope.php`, `IncludeContext.php`, `PanelResponse.php`, `app/Enums/ServerState.php`).
They are **not captured from a running panel**: nothing here claims more than the source says.
Replace them with captures from a live panel when one is available (wayhouse-proxy/plugins#11).

- `servers_list.json`: one server per `ServerState` value, a server with two allocations whose
  primary is not the first, an allocation bound to `0.0.0.0`, and a server without a primary.
- `servers_list_key_without_allocations.json`: what a key without *Allocations* read access gets:
  the include is not refused, it comes back as `{"object": "null_resource", "attributes": null}`.
