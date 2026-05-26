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

## Prerequisites

Before setting up Tailscale on your booth:

1. **Create a Tailscale account** and tailnet if you don't already have one
   (sign up at <https://login.tailscale.com>)
2. **Enable MagicDNS** in your Tailscale admin console:
   - Go to <https://login.tailscale.com/admin/dns>
   - Enable **MagicDNS**
3. **Enable HTTPS certificates**:
   - In the same DNS settings, enable **HTTPS** (enables automatic Let's
     Encrypt certificate provisioning)

## Initial setup and authentication

### Install Tailscale

```sh
curl -fsSL https://tailscale.com/install.sh | sh
```

### Authenticate with persistence and hostname

For a production booth that should stay online and accessible, use these
flags during the initial `tailscale up`:

```sh
sudo tailscale up \
  --hostname=telephone-booth \
  --ssh \
  --accept-routes \
  --advertise-exit-node=false
```

**Flags explained:**

- `--hostname=telephone-booth` — sets a stable, human-readable MagicDNS name
  (`telephone-booth.<tailnet>.ts.net`). Without this, Tailscale auto-generates
  a name from the OS hostname which may not be descriptive.
- `--ssh` — enables Tailscale SSH so you can `ssh telephone-booth` from any
  device on your tailnet without managing SSH keys. **Important:** When
  Tailscale SSH is enabled, it takes over SSH access and the regular SSH
  daemon (sshd) on port 22 becomes inaccessible from the local LAN by default.
  To preserve local LAN SSH access (e.g., `ssh pi@telephone-booth.local` or
  `ssh pi@<IP>`), you must either:
  1. Disable Tailscale SSH and use traditional SSH with key-based auth, or
  2. Access the device through Tailscale's network (`ssh telephone-booth`), or
  3. Configure Tailscale SSH to allow fallback to local SSH (see Tailscale
     docs on SSH configuration).
- `--accept-routes` — allows the node to use subnet routes advertised by
  other nodes (useful if your operator backend is on a different subnet).
- `--advertise-exit-node=false` — explicitly disables exit node mode
  (default, but documented here for clarity).

**Note on persistence:**

- **Devices are persistent by default.** They stay authenticated until you
  explicitly disable the key in the admin console or run `tailscale logout`.
- **Do NOT use `--ephemeral` for production booths.** Ephemeral nodes are
  deleted from the tailnet when they disconnect, which is only useful for
  short-lived CI runners or one-off testing. If you accidentally used
  `--ephemeral`, re-authenticate without that flag.

After running `tailscale up`, Tailscale will print an auth URL. Open it in a
browser, sign in, and authorize the device.

### Verify authentication

```sh
tailscale status
```

You should see output like:

```text
100.x.y.z   phone-booth          user@example.com  linux   active; ...
```

Test SSH access from another device on your tailnet:

```sh
ssh telephone-booth
```

This should connect without password prompts (Tailscale SSH uses your tailnet
identity for auth).

### Ensure Tailscale starts on boot

The Tailscale installer enables the systemd service by default, but verify:

```sh
sudo systemctl enable tailscaled
sudo systemctl status tailscaled
```

The service should show `enabled` and `active (running)`. After a reboot,
Tailscale will automatically reconnect to your tailnet.

**Important:** The booth's `telephone-booth-tailscale-serve.service` unit has
a systemd dependency on `tailscaled.service`, so the serve configuration will
wait for Tailscale to be ready before starting.

## Optional: Tailscale tags and ACLs

For better security and organization, tag the booth and restrict access via
ACLs.

### Tag the booth

Re-authenticate with a tag (requires updating ACLs first — see below):

```sh
sudo tailscale up \
  --hostname=telephone-booth \
  --ssh \
  --accept-routes \
  --advertise-tags=tag:booth
```

### Define tag ownership in ACL

In your Tailscale admin console, go to **Access Controls** and add:

```json
{
  "tagOwners": {
    "tag:booth": ["autogroup:admin"]
  },
  "acls": [
    {
      "action": "accept",
      "src": ["autogroup:admin"],
      "dst": ["tag:booth:443"]
    },
    {
      "action": "accept",
      "src": ["tag:booth"],
      "dst": ["your-operator-backend-host:443"]
    }
  ]
}
```

This ensures:

- Only admins can manage booths tagged with `tag:booth`.
- Admins can reach the booth on port 443 (the debug surface HTTPS endpoint).
- The booth can reach the operator backend on port 443.

**Note:** For broader admin access to all booth ports (including SSH), use
`"dst": ["tag:booth:*"]` instead of `"dst": ["tag:booth:443"]`. See the
additional [ACL example](#acl-example) at the end of this document for
comparison.

## MagicDNS hostname

After authentication, your booth is accessible at:

```text
https://telephone-booth.<your-tailnet>.ts.net/
```

The exact tailnet suffix depends on your Tailscale account (e.g.
`my-org.ts.net` or `tail1234.ts.net`).

## Provisioning

1. Install the `.deb`; it depends on `tailscale` and installs
   `telephone-booth-tailscale-serve.service`.
2. Edit `/etc/phone-booth/env` and set required environment variables (see
   [Configuration](#environment-variables) below).
3. Configure Tailscale serve:

   ```sh
   sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
   sudo systemctl enable --now telephone-booth.service telephone-booth-tailscale-serve.service
   ```

The `enable --now` ensures both services start immediately **and** restart
automatically on reboot. The `telephone-booth-tailscale-serve.service` unit
depends on `tailscaled.service`, so systemd will wait for Tailscale to be
ready before configuring the serve proxy.

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
  https://telephone-booth.<your-tailnet>.ts.net/healthz
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

## Environment variables

The booth requires several environment variables to be set in
`/etc/phone-booth/env`. This file is sourced by the systemd service unit and
should be mode `0640`, owned by `root:phonebooth`.

### Required variables

| Variable | Description | Where to get it |
| -------- | ----------- | --------------- |
| `BOOTH_OPERATOR_BASE_URL` | Base URL of the operator API (e.g. `https://operator.example.com`) | Your operator deployment URL. If running the operator locally for dev, use the Tailscale MagicDNS URL or public URL. |
| `BOOTH_OPERATOR_TOKEN` | Bearer API token for authenticating with the operator backend | **Generate in the operator UI:** Sign in → Dial **6** → Settings → API tokens → Create. Copy the token (shown only once). Format: `tbo_...` (32 random bytes, base64-encoded). See [operator-api.md](operator-api.md#issuing-a-token). |
| `BOOTH_DEBUG_TOKEN` | Bearer token for authenticating requests to the debug surface | **Generate yourself:** Use `openssl rand -base64 24` or similar to create a strong random token (≥16 chars). Store this securely — anyone with this token can access the debug panel and view booth state, telemetry, and audio levels. |

### Optional variables

| Variable | Description | Default |
| -------- | ----------- | ------- |
| `RUST_LOG` | Tracing filter (e.g. `info`, `debug`, `warn`) | `info` |
| `BOOTH_AUDIO_DEVICE` | Substring to match audio device name (e.g. `Focusrite`, `USB`) | `Focusrite` |
| `BOOTH_OBSERVABILITY_ENABLED` | Enable metrics + operator event forwarding (`true` or `false`) | `true` |
| `BOOTH_OBSERVABILITY_BOOTH_ID` | Human-readable booth identifier for metrics labels | `booth-01` |
| `BOOTH_OBSERVABILITY_FORWARD_ENABLED` | Forward telemetry events to operator (`true` or `false`) | `true` |

For GPIO pin configuration, audio settings, and other non-secret config, use
`/etc/phone-booth/config.toml` instead of environment variables. See
[configuration.md](configuration.md) for the full reference.

### Secret precedence

The booth supports two patterns for providing secrets:

1. **Direct env var:** `BOOTH_OPERATOR_TOKEN=tbo_abc123...`
2. **File reference:** `BOOTH_OPERATOR_TOKEN_FILE=/run/secrets/operator-token`

If both are set, the direct var wins. File values are trimmed for trailing
newlines so they work with systemd credentials and Kubernetes-style secret
mounts.

**Never log secrets:** The booth redacts `BOOTH_OPERATOR_TOKEN` and
`BOOTH_DEBUG_TOKEN` to the last 4 characters in logs and `print-config`
output.

### Example `/etc/phone-booth/env`

```sh
# Operator API connection
BOOTH_OPERATOR_BASE_URL=https://operator.example.com
BOOTH_OPERATOR_TOKEN=tbo_4b3c9f8e7d6a5b4c3d2e1f0a9b8c7d6e
BOOTH_DEBUG_TOKEN=a-strong-random-string-at-least-16-chars

# Logging
RUST_LOG=info

# Audio device (optional; defaults to "Focusrite")
BOOTH_AUDIO_DEVICE=Focusrite

# Observability (optional; defaults shown)
BOOTH_OBSERVABILITY_ENABLED=true
BOOTH_OBSERVABILITY_BOOTH_ID=booth-01
BOOTH_OBSERVABILITY_FORWARD_ENABLED=true
```

After editing `/etc/phone-booth/env`, restart the service:

```sh
sudo systemctl restart telephone-booth.service
```

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
sudo tailscale up --hostname=telephone-booth --advertise-tags=tag:booth
```

## Failure modes and recovery

- **After reboot, Tailscale doesn't reconnect**: verify the service is
  enabled with `sudo systemctl status tailscaled`. If it's disabled, run
  `sudo systemctl enable tailscaled` and `sudo systemctl start tailscaled`.
  The `telephone-booth-tailscale-serve.service` will auto-start once
  Tailscale is ready.
- **Local LAN SSH (`ssh pi@telephone-booth.local`) stopped working after
  enabling Tailscale SSH**: Tailscale SSH takes over SSH access by default.
  To access via local LAN, either: (a) use `ssh telephone-booth` from a device
  on your tailnet, (b) temporarily disable Tailscale SSH with
  `sudo tailscale set --ssh=false`, or (c) connect a keyboard/monitor to the
  Pi for local console access. To preserve both Tailscale SSH and local LAN
  SSH, you need to configure Tailscale SSH policies in your admin console to
  allow fallback.
- **Tailscale is down or the node is expired**: `telephone-booth
  tailscale-status` or `tailscale status` fails. Re-run
  `sudo tailscale up --hostname=telephone-booth --ssh --accept-routes`, then
  `sudo systemctl restart telephone-booth-tailscale-serve`. Use the
  [LAN fallback](lan-fallback.md) while tailnet access is down.
- **Serve config is missing**: run
  `sudo /usr/share/telephone-booth/setup-tailscale-serve.sh` or
  `sudo systemctl restart telephone-booth-tailscale-serve.service`.
- **Certificate errors**: verify HTTPS certificates are enabled in the
  Tailscale admin console. Let's Encrypt certificates are managed and
  renewed by Tailscale; the booth service does not need ACME timers.
- **401/403 from the booth**: the Bearer token is missing or stale. Rotate
  `BOOTH_DEBUG_TOKEN`, restart `telephone-booth.service`, and update the
  operator UI.
