# Tailscale-backed debug surface

Tailscale is the recommended way to reach the booth's debug panel. With
`tailscale serve` enabled, the Rust binary listens only on loopback while
Tailscale handles TLS termination and routing on the wider tailnet,
giving you a real Let's Encrypt cert and DNS name with zero firewall work.

## 1. Install `tailscaled` on the Pi

```sh
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up --hostname=booth-1
```

Approve the booth in your Tailscale admin console.

## 2. Enable MagicDNS + HTTPS in the admin console

In the Tailscale admin → **DNS**: enable **MagicDNS**, then enable
**HTTPS certificates**. This grants the booth a name like
`booth-1.<your-tailnet>.ts.net`.

## 3. Tell `tailscale serve` about the local debug server

The booth binary listens on `127.0.0.1:8080` (HTTP, plaintext, loopback
only). The `.deb` ships a systemd unit `tailscale-serve-booth.service`
that runs:

```sh
tailscale serve --bg --https=443 / http://127.0.0.1:8080
```

That publishes the debug panel at `https://booth-1.<your-tailnet>.ts.net`
with a Tailscale-managed Let's Encrypt cert.

If you'd rather wire it by hand:

```sh
sudo systemctl enable --now tailscale-serve-booth.service
tailscale serve status
```

## 4. Lock it down with ACLs

A minimal ACL that grants only your user access to the booth's debug
port:

```json
{
  "tagOwners": { "tag:booth": ["autogroup:admin"] },
  "acls": [
    { "action": "accept",
      "src":    ["autogroup:admin"],
      "dst":    ["tag:booth:443"] }
  ]
}
```

Tag the booth with `tag:booth` (`tailscale up --advertise-tags=tag:booth
--hostname=booth-1`) and re-authenticate.

## 5. Point the operator UI at the tailnet name

In the operator UI, **Settings → Debug**, set:

```
URL:   https://booth-1.<your-tailnet>.ts.net
Token: <contents of /etc/phone-booth/debug-token on the Pi>
```

The connection chip should turn green and say "Tailscale".

## Troubleshooting

- **`tailscale serve status` is empty** → the systemd unit didn't run;
  check `sudo journalctl -u tailscale-serve-booth.service`.
- **TLS handshake errors** → HTTPS certs aren't enabled in the admin
  console, or the tailnet is on a free plan that excludes the feature.
  Fall back to [LAN](lan-fallback.md) until that's resolved.
- **403 from the booth** → the Bearer debug token is wrong; rotate it
  with the steps in [`runbook.md`](runbook.md).
