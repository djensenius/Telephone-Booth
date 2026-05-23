# Tailscale-backed debug surface

Telephone Booth exposes its debug surface on plaintext loopback HTTP at
`127.0.0.1:8080`. Production access should go through `tailscale serve`:
Tailscale terminates HTTPS on port 443, publishes the booth at a real
MagicDNS name, and obtains/renews a real Let's Encrypt certificate. The
booth process does not run ACME, store public TLS private keys, or open a
WAN-facing port.

## Why `tailscale serve` owns HTTPS

- Tailscale already knows the booth's tailnet identity and MagicDNS name.
- Enabling HTTPS certificates in the admin console lets Tailscale request
  and renew Let's Encrypt certs automatically.
- The Rust service stays simple and binds only to `127.0.0.1:8080` for
  the Tailscale path; LAN fallback is separate and self-signed.
- Tailnet ACLs decide who can reach `:443`, while the booth still requires
  the debug Bearer token for HTTP and WebSocket requests.

## MagicDNS hostname

Enable **MagicDNS** and **HTTPS certificates** in the Tailscale admin
console. Give the Pi a stable name before provisioning:

```sh
sudo tailscale set --hostname=phone-booth
# or during first auth:
sudo tailscale up --hostname=phone-booth
```

The final URL will look like:

```text
https://phone-booth.<your-tailnet>.ts.net/
```

## Provisioning

1. Install the `.deb`; it depends on `tailscale` and installs
   `telephone-booth-tailscale-serve.service`.
2. Edit `/etc/phone-booth/env` and set `BOOTH_OPERATOR_BASE_URL`,
   `BOOTH_OPERATOR_TOKEN`, `BOOTH_DEBUG_TOKEN`, and `RUST_LOG=info`.
3. Authenticate Tailscale and configure serve:

   ```sh
   sudo tailscale set --hostname=phone-booth
   sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
   sudo systemctl enable --now telephone-booth.service telephone-booth-tailscale-serve.service
   ```

The helper is idempotent. If the node is not authenticated it runs
`tailscale up` and prints the interactive auth URL. It then applies the
expected serve target:

```sh
tailscale serve --bg --https=443 --set-path=/ http://127.0.0.1:8080
```

## Verify

On the Pi:

```sh
telephone-booth tailscale-status
sudo systemctl status telephone-booth-tailscale-serve.service
```

From any allowed tailnet device:

```sh
curl -fsS \
  -H "Authorization: Bearer $BOOTH_DEBUG_TOKEN" \
  https://phone-booth.<your-tailnet>.ts.net/healthz
```

In the operator UI, set the debug URL to the MagicDNS HTTPS URL and paste
the same debug token. The connection indicator should report the
Tailscale path.

## Rotate the debug token

```sh
sudo install -m 0640 -o root -g phonebooth /dev/null /etc/phone-booth/env.new
sudo cp /etc/phone-booth/env /etc/phone-booth/env.new
sudo editor /etc/phone-booth/env.new   # replace BOOTH_DEBUG_TOKEN
sudo mv /etc/phone-booth/env.new /etc/phone-booth/env
sudo systemctl restart telephone-booth.service
telephone-booth tailscale-status
```

Then update the token stored in the operator UI. The HTTPS URL and
Let's Encrypt certificate stay the same.

## ACL example

Restrict access to administrators or a dedicated tag:

```json
{
  "tagOwners": { "tag:booth": ["autogroup:admin"] },
  "acls": [
    {
      "action": "accept",
      "src": ["autogroup:admin"],
      "dst": ["tag:booth:443"]
    }
  ]
}
```

Authenticate the booth with the tag if you use one:

```sh
sudo tailscale up --hostname=phone-booth --advertise-tags=tag:booth
```

## Failure modes and recovery

- **Tailscale is down or the node is expired**: `telephone-booth
  tailscale-status` or `tailscale status` fails. Re-run `sudo tailscale
  up`, then `sudo systemctl restart telephone-booth-tailscale-serve`.
  Use the [LAN fallback](lan-fallback.md) while tailnet access is down.
- **Serve config is missing**: run
  `sudo /usr/share/telephone-booth/setup-tailscale-serve.sh` or
  `sudo systemctl restart telephone-booth-tailscale-serve.service`.
- **Certificate errors**: verify HTTPS certificates are enabled in the
  Tailscale admin console. Let's Encrypt certificates are managed and
  renewed by Tailscale; the booth service does not need ACME timers.
- **401/403 from the booth**: the Bearer token is missing or stale. Rotate
  `BOOTH_DEBUG_TOKEN`, restart `telephone-booth.service`, and update the
  operator UI.
