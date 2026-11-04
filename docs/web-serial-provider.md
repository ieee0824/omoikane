# Web Serial provider sessions

`SerialProvider` separates origin-scoped permission from an opened connection.
The Rust provider trait changes with the session ownership fix; JavaScript and
C FFI signatures remain unchanged. External Rust implementations must migrate.

| Operation | Provider contract |
| --- | --- |
| `get_ports`, `request_port`, `forget` | Keep origin-scoped permission behavior. |
| `open` | Return a fresh `SerialSessionId` for every successful open. Never reuse an identity within the provider lifetime, including after disconnect. |
| `read`, `write`, `get_signals`, `set_signals` | Accept the returned identity and check it atomically with the port operation. An expired or wrong session returns `InvalidStateError` without modifying a newer connection. |
| `close` | Close the matching identity. An already expired identity succeeds without device I/O or changing another connection, allowing its owner to finish local cleanup after disconnect. |
| `retire` | Accept origin, port and identity. Atomically invalidate only that logical session. A stale identity leaves newer sessions untouched. |

Create IDs with `SerialSessionId::new` from a provider-owned non-reusing
allocator. Keep the current ID in the same protected state as the connection;
a host-side precheck followed by an unguarded origin/port operation permits a
reconnect race. Exhaustion must fail rather than wrap and reuse an ID.
Validate expired identities before fallible I/O or consuming pending work for
the current connection. A stale operation must leave that work untouched.

A normal close failure leaves logical ownership available for retry. JavaScript
streams may already be ended when backend close fails; the owning port can retry
close without reopening or using another connection. Retirement
must invalidate logical ownership even when physical cleanup fails. The provider
owns physical cleanup and must reject a later open, for example with
`NetworkError`, until its device handle can safely be reused. Retirement must
never schedule cleanup that can act on a newer session by origin/port alone.

The host records the opening Document, origin, returned identity and issuing
provider. Replacing the configured provider affects future sessions, while
existing sessions continue using their issuing provider. A Document's retirement
removes its ownership records and retires those exact sessions. Permission
sharing across Documents does not share connection ownership.

The private JavaScript bridge also issues a non-reused host connection ID for
each open. Stream callbacks capture that ID rather than looking up whichever
connection most recently used the same Document and port. Host IDs are distinct
from provider IDs because a replacement provider may issue the same numeric ID.
Closing a port cancels its readable stream and aborts its writable stream before
releasing the backend session. An expired host ID can finish local close cleanup
without closing a replacement session; other operations reject expired IDs.

There is no compatibility default that silently forwards token-bearing calls to
the old origin/port-only interface: it cannot provide the required reconnect
isolation. The deterministic provider validates under its port mutex and clears
current sessions on disconnect while keeping its identity allocator monotonic.

## Output signal updates (#1711)

`SerialOutputSignals` is a public, re-exported Rust type. Changing its three
members from `bool` to `Option<bool>` is a breaking source and serialization
change even though the provider method name and argument type name stay the same.
External struct literals must use `Some(true)` or `Some(false)` for requested
changes and `None` for untouched outputs. A default value is an empty update,
not a request to lower every line. The JavaScript/native bridge rejects an empty
update before calling a provider.

Providers must preserve omitted outputs, applying specified values in the Web
Serial order: dataTerminalReady, requestToSend, break. Keep current device state
as concrete booleans; optional values belong only at the request boundary. This
does not imply atomic hardware changes; existing session checks remain required.

Serialization omits `None`; deserialization maps missing members to `None`,
accepts only boolean values for present members, and rejects explicit JSON null
and unknown fields. This differs from JavaScript dictionary conversion:
`setSignals({requestToSend:null})` converts null to false and sends
`Some(false)`, while undefined is omitted. Getter conversion occurs before
method state checks; the original session is revalidated afterward.
