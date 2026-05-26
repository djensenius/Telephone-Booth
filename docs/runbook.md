# Runbook (day-2 ops)

For troubleshooting specific problems, see
[troubleshooting.md](troubleshooting.md). For observability setup and monitoring
dashboards, see [observability.md](observability.md).

## Runtime CLI

```sh
telephone-booth run [--config /etc/phone-booth/config.toml] [--mock]
telephone-booth print-config [--config /etc/phone-booth/config.toml]
telephone-booth check [--config /etc/phone-booth/config.toml]
telephone-booth simulate <pulses>
telephone-booth tailscale-status
```

- `run` starts the service. Exit code `0` means clean shutdown; nonzero
  means config, adapter, or runtime startup failed.
- `print-config` renders effective TOML with tokens redacted.
- `check` is intended for `ExecStartPre`; it exits nonzero if config
  validation, audio probing, or GPIO reservation fails.
- `simulate` injects rotary pulses into the pure state machine.
- `tailscale-status` shells out to Tailscale's JSON status commands and
  prints the MagicDNS name, final HTTPS URL, serve config, and health
  messages.

## Daily checks

```sh
systemctl is-active telephone-booth.service
systemctl is-active telephone-booth-tailscale-serve.service
telephone-booth tailscale-status
journalctl -u telephone-booth.service --since "24h ago" -p warning
```

Confirm the URL printed by `tailscale-status` matches the operator UI's
Debug URL and that `serve_config` points at `http://127.0.0.1:8080`.

## Rotating the operator API token

1. Operator UI → Settings → API tokens → **Create** (label
   `booth-1-rotated-yyyy-mm-dd`).
2. Copy the plaintext token.
3. On the Pi: edit `/etc/phone-booth/env`, replace
   `BOOTH_OPERATOR_TOKEN`, save.
4. `sudo systemctl restart telephone-booth.service`.
5. Operator UI → **Revoke** the old token.
6. Confirm the new token works: debug panel → Operator card → "last
   contact" timestamp should be recent.

## Rotating the debug Bearer token

```sh
sudo editor /etc/phone-booth/env      # replace BOOTH_DEBUG_TOKEN
sudo systemctl restart telephone-booth.service
telephone-booth tailscale-status
```

Paste the new token into the operator UI → Debug → **Update token**. The
MagicDNS HTTPS URL and Let's Encrypt certificate stay the same.

## Triggering simulated events

Host-side state-machine smoke test:

```sh
telephone-booth simulate 5
```

Runtime debug controls require `debug.allow_controls = true` and a valid
Bearer token. From an allowed tailnet device:

```sh
curl -fsS -X POST \
  -H "Authorization: Bearer $BOOTH_DEBUG_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"count":5}' \
  https://telephone-booth.<your-tailnet>.ts.net/v1/simulate/pulse
```

Disable controls again after testing and restart the service.

## Tailscale recovery

```sh
sudo tailscale status
sudo tailscale up --hostname=telephone-booth
sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
sudo systemctl restart telephone-booth-tailscale-serve.service
telephone-booth tailscale-status
```

Use [LAN fallback](lan-fallback.md) if the tailnet or ACLs are blocking
access.

## Regenerating or re-pinning the LAN cert

See [`lan-fallback.md`](lan-fallback.md#rotating-the-lan-certificate) and
use the operator UI's `CertFingerprintCard` to replace the pin.

## Reading logs

```sh
journalctl -u telephone-booth.service -f                # live
journalctl -u telephone-booth.service --since "1h ago"  # backlog
journalctl -u telephone-booth.service -o json | \
   jq 'select(.MESSAGE | contains("error"))'            # filter
```

The same structured stream feeds the debug surface and WebSocket
telemetry firehose.

## Updating the binary in place

```sh
sudo apt install ./telephone-booth_<new>_arm64.deb
sudo systemctl status telephone-booth.service
telephone-booth tailscale-status
```

Config under `/etc/phone-booth/env` and state under `/var/lib/phone-booth`
are preserved.

## Disaster recovery

Worst case: the SD card died.

1. Image a fresh Raspberry Pi OS 64-bit card; configure SSH + Wi-Fi via
   the imager.
2. `sudo apt install ./telephone-booth_<latest>_arm64.deb`.
3. Restore `/etc/phone-booth/env` from your password manager / Vault.
4. Run `sudo /usr/share/telephone-booth/setup-tailscale-serve.sh`.
5. Issue fresh operator and debug tokens; revoke previous tokens in the
   operator UI.
6. Re-pin the LAN certificate fingerprint if LAN fallback is used.

Pending recordings on the old card are unrecoverable unless you imaged it
before swapping; if the card is alive enough, copy `/var/lib/phone-booth/`
to the new Pi before restarting uploads.
