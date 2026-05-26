# LAN fallback (self-signed TLS + fingerprint pinning)

Use LAN fallback when Tailscale is unavailable, the node is expired, or an
operator is standing on the same Wi-Fi as the booth. The preferred path is
still [Tailscale serve](tailscale.md), which gives a real Let's Encrypt
certificate; LAN fallback uses a self-signed certificate and explicit
fingerprint pinning.

**Prefer [Tailscale](tailscale.md) for production** — it provides real
certificates and encrypted tunnels. Use LAN fallback only when necessary.

## Security model

LAN fallback is **disabled by default** and binds to loopback
(`127.0.0.1:8443`). Operators must explicitly opt in by setting
`debug.lan_enabled = true` in the configuration file. When the bind address
is non-loopback (e.g. `0.0.0.0:8443`), the debug surface enforces a minimum
token strength — the bearer token must be at least 16 characters. The server
will refuse to start if a non-loopback bind is requested without a valid token.

## How it works

`booth-debug` listens on the configured LAN address (default
`127.0.0.1:8443`) for the LAN path and requires the same Bearer debug token
as the Tailscale path. Because the certificate is self-signed, trust comes
from comparing the SHA-256 certificate fingerprint over an out-of-band
channel, not from a public CA chain.

## Reading the fingerprint

From a trusted shell on the Pi, ask the local loopback API for the active
certificate fingerprint:

```sh
curl -fsS \
  -H "Authorization: Bearer $BOOTH_DEBUG_TOKEN" \
  http://127.0.0.1:8080/v1/cert/fingerprint
```

You can also read it through the Tailscale URL while Tailscale is healthy:

```sh
curl -fsS \
  -H "Authorization: Bearer $BOOTH_DEBUG_TOKEN" \
  https://telephone-booth.<your-tailnet>.ts.net/v1/cert/fingerprint
```

Record the `sha256` value before pinning a LAN connection.

## Pinning with `CertFingerprintCard`

The operator UI's Debug settings include `CertFingerprintCard`, which is
responsible for showing and storing the pinned LAN certificate
fingerprint.

1. Open **Settings → Debug** and add or edit the booth.
2. Set the LAN URL, for example `https://192.168.1.42:8443`.
3. Paste the current debug Bearer token from `/etc/phone-booth/env`
   (`BOOTH_DEBUG_TOKEN`).
4. Connect once. `CertFingerprintCard` displays the presented SHA-256
   fingerprint.
5. Compare it to the trusted value collected from the Pi. If it matches,
   click **Pin fingerprint**.
6. Future LAN connections must present the same fingerprint. If it
   changes unexpectedly, the card blocks the connection until an operator
   verifies and re-pins it.

## Direct browser access

Browsers will warn because the LAN certificate is self-signed. That is
expected. Prefer the operator UI because it pins the fingerprint; only
bypass browser warnings for short local diagnostics.

## Rotating the LAN certificate

Restarting the booth may generate a new self-signed certificate depending
on the deployed debug-surface configuration. After any intentional
rotation:

```sh
sudo systemctl restart telephone-booth.service
curl -fsS -H "Authorization: Bearer $BOOTH_DEBUG_TOKEN" \
  http://127.0.0.1:8080/v1/cert/fingerprint
```

Compare the new `sha256` value with the operator UI and use
`CertFingerprintCard` to replace the old pin.

## Enabling the LAN listener

To expose the debug surface on the local network, set the following in
`/etc/phone-booth/config.toml`:

```toml
[debug]
lan_enabled = true
lan_bind = "0.0.0.0:8443"
token = "a-cryptographically-random-token-at-least-16-chars"
```

The token must be at least 16 characters long when the bind address is not
loopback. The server will refuse to start otherwise.

## Disabling the LAN listener

The LAN listener is disabled by default. If it was previously enabled, set
`debug.lan_enabled = false` in `/etc/phone-booth/config.toml` (or a future
packaged config override) and restart. The loopback listener used by
`tailscale serve` continues to work.
