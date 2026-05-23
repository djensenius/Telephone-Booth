# Runbook (day-2 ops)

## Rotating the operator API token

1. Operator UI → Settings → API tokens → **Create** (label `booth-1-rotated-yyyy-mm-dd`).
2. Copy the plaintext token.
3. On the Pi: edit `/etc/phone-booth/config.toml`, replace
   `operator.token`, save.
4. `sudo systemctl restart telephone-booth`.
5. Operator UI → **Revoke** the old token.
6. Confirm the new token works: debug panel → Operator card → "last
   contact" timestamp should be recent.

## Rotating the debug Bearer token

```sh
sudo rm /etc/phone-booth/debug-token
sudo systemctl restart telephone-booth
sudo cat /etc/phone-booth/debug-token
```

Paste into the operator UI → Debug → "Update token". The Tailscale URL
stays the same; only the bearer changes.

## Regenerating the debug TLS cert

See [`lan-fallback.md`](lan-fallback.md#rotating-the-cert).

## Restoring recordings

Recordings live at `/var/lib/telephone-booth/recordings/<sha256>.flac`
until they upload, then the file remains on disk until the next prune
(default: when free space drops below 200 MB).

To force-upload a stranded recording:

```sh
sudo -u telephone-booth /usr/bin/telephone-booth replay-uploads
```

(Available with `debug.allow_controls = true`; see
[`debug-panel.md`](debug-panel.md).)

## Reading logs

```sh
journalctl -u telephone-booth -f                # live
journalctl -u telephone-booth --since "1h ago"  # backlog
journalctl -u telephone-booth -o json | \
   jq 'select(.MESSAGE | contains("error"))'    # filter
```

The same structured stream feeds the `/debug/logs` endpoint and the
WebSocket telemetry firehose, so you can debug from anywhere on the
tailnet.

## Updating the binary in place

`apt install ./telephone-booth_<new>.deb` swaps the binary and restarts
the service in one step. Config and state are preserved.

## Disaster recovery

Worst case: the SD card died.

1. Image a fresh Raspberry Pi OS 64-bit card; configure SSH + Wi-Fi via
   the imager.
2. `sudo apt install ./telephone-booth_<latest>.deb`.
3. Restore `/etc/phone-booth/config.toml` from your password manager / Vault.
4. Issue a fresh operator API token and a fresh debug Bearer token
   (the previous ones are now lost; revoke them in the operator UI).
5. Re-pin the debug cert fingerprint in the operator UI.

Pending recordings on the old card are unrecoverable unless you imaged it
before swapping; if the card is alive enough, `rsync -av
/var/lib/telephone-booth/recordings/` to the new Pi and `replay-uploads`.
