# Troubleshooting

If you don't find your symptom here, check the [debug panel](debug-panel.md)
— most issues become obvious once you see the live GPIO and audio meters. For
ongoing operational issues, see the [runbook](runbook.md).

## The booth is silent (no dial tone) when the receiver is lifted

- **Hook-switch wired backwards.** Set `gpio.invert.hook = true` and
  restart, or swap the wires.
- **Wrong audio device.** Run `telephone-booth --print-config` and check
  the `[audio]` block. If `device_substring` doesn't match anything,
  the binary falls back to the system default — confirm the Focusrite
  shows up in `aplay -L` / `pactl list short sinks`.
- **The service can't read its own config** (see below). If `journalctl`
  shows the built-in defaults (`device_substring` = `Focusrite`,
  `debounce_ms` = `25`) even though `/etc/phone-booth/config.toml` is
  populated, the service user can't read the file.

## The service ignores `config.toml` and runs on defaults

Symptom: the booth boots and runs, but behaves as if `config.toml` were
empty — the log shows `needle=Focusrite`, `debounce_ms=25`, and no
`[audio.mixer]` is applied — even though `sudo cat /etc/phone-booth/config.toml`
shows the right values.

Cause: the service user (`phonebooth`, with primary group `audio`) can't
**traverse** the `0750 root:phonebooth` config directory, so the loader
treats the file as absent. Older packages set
`SupplementaryGroups=gpio dialout` without `phonebooth`, which drops that
access. Reproduce it exactly as the unit runs:

```sh
sudo systemd-run --uid=phonebooth --gid=audio --pipe -q \
  cat /etc/phone-booth/config.toml
```

If that prints `Permission denied` while `sudo cat` works, that's the bug.
Current packages ship `SupplementaryGroups=gpio dialout phonebooth`; on an
older install add it via a drop-in:

```sh
sudo mkdir -p /etc/systemd/system/telephone-booth.service.d
printf '[Service]\nSupplementaryGroups=phonebooth\n' | \
  sudo tee /etc/systemd/system/telephone-booth.service.d/config-access.conf
sudo systemctl daemon-reload
sudo systemctl restart telephone-booth
```

Newer binaries also **fail loudly** in this situation (the `check`
pre-start step reports `cannot access config file …`) instead of silently
starting on defaults.

## Rotary dial seems to skip or stick

- **Debounce mistuned for your dial.** The default is `gpio.debounce_ms = 25`.
  The two failure modes pull in opposite directions, so tune by symptom:
  raise it (e.g. `30`–`40`) when a single click is _double-counted_ (digits
  read too high); lower it (e.g. `10`–`15`) when distinct pulses _merge_ or go
  missing (digits read too low). Don't raise it to fix merged pulses — a longer
  window makes merging worse.
- **Pulse + gate swapped.** Check the live telemetry: with the receiver
  off-hook and the dial untouched, the `rotary_gate` line should read
  `high`; pulling a digit should toggle `gate` low, then `pulse` low
  N times.
- **Phones with reverse-polarity contacts.** `gpio.invert.rotary_pulse`
  defaults to `true` (counting the break pulses of a normally-closed impulse
  contact). If your dial reads one digit too high or fires a phantom leading
  `1`, flip it to `false`; adjust `gpio.invert.rotary_gate` similarly.

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
- Azure SAS URLs expire per operator policy. The phone no longer receives an
  `expiresAt` field, so check operator logs if slow networks leave blobs
  uncompleted before the SAS expiry window.
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

## Cannot SSH to the Pi via `.local` hostname

- **`.local` is the standard mDNS domain** and should work on most networks.
  Use `ssh pi@telephone-booth.local` (not `.lan`).
- **`.lan` is router-specific** and may not be configured. If your router
  doesn't support `.lan`, use `.local` or the Pi's IP address instead.
- **mDNS not working?** Install `avahi-daemon` on the Pi (usually pre-installed
  on Raspberry Pi OS). On your client machine, ensure mDNS is enabled
  (macOS/Linux: built-in; Windows: install Bonjour Print Services).
- **Still can't resolve?** Find the Pi's IP with `tailscale status` or check
  your router's DHCP leases, then use `ssh pi@<IP-address>`.
- **Or use Tailscale SSH:** `ssh telephone-booth` (no `.local`, no IP needed)
