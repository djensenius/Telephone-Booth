# Packaging & systemd

Production booths run the Rust client from a Debian package built with
`cargo deb`. The package includes the runtime unit, Tailscale serve unit,
system user definitions, tmpfiles rules, sample config, and the
Tailscale provisioning helper.

## Building the `.deb`

CI and local release builds use the Pi and systemd features:

```sh
cross build -p booth-bin --release --target <triple> --features pi,systemd --no-default-features
cargo deb -p booth-bin --no-build --target <triple>
```

The `publish` workflow builds:

- `telephone-booth_<version>_arm64.deb` for `aarch64-unknown-linux-gnu`
  (Pi 4 / Pi 5 / CM4)
- `telephone-booth_<version>_armhf.deb` for
  `armv7-unknown-linux-gnueabihf` (Pi 3)

## Dependencies

`crates/booth-bin/Cargo.toml` declares Debian dependencies on:

- `tailscale` for `tailscaled`, MagicDNS, HTTPS certificates, and
  `tailscale serve`
- `alsa-utils` for ALSA diagnostics on the USB Focusrite path

PulseAudio is intentionally not recommended; ALSA is sufficient for the
booth hardware.

## Installed layout

| Path | Purpose |
| --- | --- |
| `/usr/bin/telephone-booth` | Rust runtime and diagnostics CLI |
| `/lib/systemd/system/telephone-booth.service` | Main phone client service |
| `/lib/systemd/system/telephone-booth-tailscale-serve.service` | `tailscale serve` proxy for `127.0.0.1:8080` |
| `/lib/systemd/system/telephone-booth-vmagent.service` | vmagent sidecar that scrapes `/metrics` and remote-writes to VictoriaMetrics |
| `/usr/lib/sysusers.d/telephone-booth.conf` | Creates the `phonebooth` system user |
| `/usr/lib/tmpfiles.d/telephone-booth.conf` | Creates `/var/lib/phone-booth`, `/var/lib/phone-booth/vmagent`, `/var/log/phone-booth`, `/etc/phone-booth` |
| `/etc/phone-booth/env` | Conffile with `BOOTH_OPERATOR_*`, `BOOTH_DEBUG_TOKEN`, `RUST_LOG` |
| `/etc/phone-booth/config.example.toml` | Reference TOML config for non-env settings |
| `/etc/phone-booth/vmagent.env` | Conffile with `BOOTH_VM_REMOTE_WRITE_URL` for the vmagent sidecar |
| `/etc/phone-booth/vmagent.yaml` | Conffile with the vmagent scrape config |
| `/usr/share/telephone-booth/setup-tailscale-serve.sh` | Idempotent Tailscale provisioning helper |
| `/usr/share/doc/telephone-booth/README.md` | Project README |

`/etc/phone-booth/env`, `/etc/phone-booth/vmagent.env`, and
`/etc/phone-booth/vmagent.yaml` are marked as Debian conffiles so local
edits survive upgrades.

## systemd services

`telephone-booth.service` runs as `phonebooth`, checks the runtime before
start, and uses `Type=notify` with a 30 second watchdog:

```sh
sudo systemctl status telephone-booth.service
sudo journalctl -u telephone-booth.service -f
```

`telephone-booth-tailscale-serve.service` is a oneshot unit that persists
Tailscale's serve config:

```sh
sudo systemctl status telephone-booth-tailscale-serve.service
tailscale serve status
```

The `postinst` maintainer script runs `systemd-sysusers`, runs
`systemd-tmpfiles --create`, enables both services, and starts them
best-effort. If `/usr/bin/vmagent` is present, it also enables and starts
`telephone-booth-vmagent.service`. `prerm` stops all three services
before removal.

## vmagent sidecar

The `.deb` lists `vmagent` as a `Recommends:` (not a hard dependency)
so the booth still installs on hosts where vmagent isn't packaged. To
push metrics to your VictoriaMetrics instance:

```sh
sudo apt install vmagent
sudo editor /etc/phone-booth/vmagent.env       # set BOOTH_VM_REMOTE_WRITE_URL
sudo install -m 0600 /dev/null /etc/phone-booth/vmagent-token
sudo editor /etc/phone-booth/vmagent-token     # paste the bearer token
sudo systemctl enable --now telephone-booth-vmagent.service
```

The scrape config lives at `/etc/phone-booth/vmagent.yaml`; it points
at `127.0.0.1:8080/metrics` (the booth's loopback debug listener) and
adds `booth_id` as an `external_labels` value. See
[observability.md](observability.md) for the full pipeline.

## Installing

```sh
sudo apt install ./telephone-booth_0.1.0_arm64.deb
sudo editor /etc/phone-booth/env
sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
sudo systemctl restart telephone-booth.service telephone-booth-tailscale-serve.service
```

## Upgrading

```sh
sudo apt install ./telephone-booth_<new>_arm64.deb
sudo systemctl status telephone-booth.service
telephone-booth tailscale-status
```

State under `/var/lib/phone-booth` and local environment config under
`/etc/phone-booth/env` are preserved.

## Removing

```sh
sudo systemctl disable --now telephone-booth.service telephone-booth-tailscale-serve.service
sudo apt remove telephone-booth
```

Use `apt purge telephone-booth` only when you also intend to remove local
configuration managed by dpkg.
