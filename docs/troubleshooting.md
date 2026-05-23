# Troubleshooting

If you don't find your symptom here, check the [debug panel](debug-panel.md)
— most issues become obvious once you see the live GPIO and audio meters.

## The booth is silent (no dial tone) when the receiver is lifted

- **Hook-switch wired backwards.** Set `gpio.invert.hook = true` and
  restart, or swap the wires.
- **Wrong audio device.** Run `telephone-booth --print-config` and check
  the `[audio]` block. If `device_substring` doesn't match anything,
  the binary falls back to the system default — confirm the Focusrite
  shows up in `aplay -L` / `pactl list short sinks`.

## Rotary dial seems to skip or stick

- **Debounce too short for your dial.** Try `gpio.debounce_ms = 8` or `10`.
- **Pulse + gate swapped.** Check the live telemetry: with the receiver
  off-hook and the dial untouched, the `rotary_gate` line should read
  `high`; pulling a digit should toggle `gate` low, then `pulse` low
  N times.
- **Phones with reverse-polarity contacts.** Set
  `gpio.invert.rotary_pulse` and/or `gpio.invert.rotary_gate` to `true`.

## "No audio device found" at startup

```sh
journalctl -u telephone-booth -g 'audio'
```

Most common cause: the Pi user lacks the `audio` group. The `.deb`'s
postinst adds the service user automatically; if you're running from
`cargo run`, add yourself:

```sh
sudo usermod -aG audio,gpio $USER
# log out and back in
```

## Operator unreachable

- Check the **debug panel → Operator** card. It shows the last successful
  contact, the last response code, and the WS connection state.
- `401` means the API token is wrong or revoked. Reissue per
  [`operator-api.md`](operator-api.md).
- Network: `curl -i https://operator.example.com/healthz` from the Pi.

## Tailscale debug URL gives a TLS warning

- Tailscale HTTPS certs aren't enabled for your tailnet. See
  [`tailscale.md`](tailscale.md) §2.
- Or you're on the LAN fallback URL; that **always** has a self-signed
  cert — pin its fingerprint in the operator UI.

## "Cert fingerprint mismatch" in the operator UI

The booth's self-signed cert was regenerated (manually, or by
re-installing). Re-pin it: operator UI → Debug → click the booth → **Re-pin
certificate** and confirm the fingerprint from
`/etc/phone-booth/debug-cert.fingerprint`.

## Recordings never finish uploading

- Check `[debug]/audio` for the file's size and sha256.
- Azure SAS URL may have expired. The slot's `expiresAt` is in the
  upload-slot response; if your network is slow, you may need to bump
  `AZURE_SAS_TTL_MINUTES` on the operator backend.
- Confirm the operator can reach Azure: from the operator container,
  `curl -I "<your blob endpoint>"`.

## Booth state stuck

`POST /debug/hangup` (requires `debug.allow_controls = true`) forces the
state machine back to `Idle` without restarting the service. If even that
doesn't help:

```sh
sudo systemctl restart telephone-booth
```

If a restart is required _often_, that's a bug — please file an issue with
the relevant `journalctl -u telephone-booth` excerpt.
