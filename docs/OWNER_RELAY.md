# Owner-operated relay

Rampage prefers authenticated direct QUIC and can run entirely LAN-local without any relay. A hard
NAT, carrier-grade NAT, or restrictive firewall can make direct connectivity impossible. In that
case `rampage-relay` provides a self-hosted fallback without a Tailscale account or an upstream
public relay fleet.

## What is automatic

After a valid `rampage-relay.json` is placed in the owner desktop's runtime directory and Rampage is
restarted, the desktop:

1. starts the loopback controller;
2. injects the configured HTTPS relay URL before the controller binds its mesh endpoint;
3. starts the bundled relay after the controller token exists;
4. lets the relay fetch a fresh Governor-signed endpoint allowlist from the exact loopback route;
5. publishes the relay URL only inside signed, expiring enrollment and offer records; and
6. stops the relay with the desktop-owned sidecar process tree.

Workers verify the signed controller record and enable only those explicit relay URLs. Iroh still
attempts direct traversal and uses the relay as fallback.

## One-time reverse-proxy setup

The owner needs a DNS name and an HTTPS endpoint that can reach the relay machine. No software can
act as a globally reachable relay when every inbound route to that machine is blocked; users behind
CGNAT need an owner-controlled publicly reachable host or should remain direct/LAN-only.

With Rampage running, create the configuration in its runtime directory:

```powershell
rampage-relay init `
  --public-url https://relay.example.com `
  --controller-token-file C:\path\to\rampage\runtime\controller.token `
  --config C:\path\to\rampage\runtime\rampage-relay.json
```

The generated safe default listens on `127.0.0.1:3340`; it refuses a non-loopback bind without
built-in TLS. Point an HTTPS reverse proxy at that socket. Caddy, for example, needs only:

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:3340
}
```

Restart Rampage, then verify the live authorization boundary:

```powershell
rampage-relay check --config C:\path\to\rampage\runtime\rampage-relay.json
```

The check prints the fabric digest, durable generation, authorized endpoint count, expiry, and
public URL. It never prints the controller token.

## Built-in TLS

For a host that terminates TLS directly, add this object to the generated JSON and choose distinct
HTTP/HTTPS sockets:

```json
{
  "tls": {
    "https_bind_addr": "0.0.0.0:443",
    "certificate_path": "C:/relay/fullchain.pem",
    "private_key_path": "C:/relay/private-key.pem"
  },
  "quic_bind_addr": "0.0.0.0:7842"
}
```

The service refuses to start unless the certificate and key form a valid Rustls server
configuration. Certificate and key inputs are size-bounded before parsing, and the auxiliary HTTP
listener remains loopback-only even in built-in TLS mode. The optional QUIC address-discovery
listener is accepted only with built-in TLS.

## Security and resource boundaries

- Public/default dependency relays are disabled.
- Relay identity is transport identity; execution authority still requires enrollment and an exact
  Governor lease.
- Admission uses a fresh, maximum-15-minute manifest bound to the configured Governor key and
  derived fabric digest.
- Controller access is loopback-only and token protected. Redirects are disabled and the relay
  accepts only the exact `/v1/mesh/relay-access` route.
- Signed-file mode is available for an expert-operated remote host; stale or tampered copies fail
  closed and must be refreshed before expiry.
- Metrics can bind only to loopback. Per-client rate, burst, per-endpoint connections, total
  connections, and key-cache capacity are bounded in the configuration.
- Relay visibility does not make storage or model payloads plaintext. Application traffic remains
  end-to-end authenticated QUIC, while artifacts retain their encrypted CAS and signed lease rules.

## Evidence boundary

The repository test forces a QUIC payload through an in-process owner relay with IP transports
disabled and validates signed-file tamper/expiry denial. This is not yet evidence for a public WAN
deployment, every NAT type, sustained relay throughput, or a newly signed installer. Those require
the separate multi-network and distribution qualification campaigns. See the retained
[owner-relay milestone evidence](OWNER_RELAY_EVIDENCE.md) for exact commands and process artifacts.
