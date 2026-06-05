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
Tailscale's serve config. It waits for the `tailscaled` backend to report
ready before applying the serve config, so it comes up cleanly after a
reboot without a manual restart:

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

The recommended path on a fresh Pi is the APT repository (see
[ADR 0007](adr/0007-apt-distribution.md)). One-time setup:

```sh
curl -fsSL https://djensenius.github.io/Telephone-Booth/telephone-booth-archive-keyring.gpg \
  | sudo install -m 0644 /dev/stdin /usr/share/keyrings/telephone-booth-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/telephone-booth-archive-keyring.gpg] https://djensenius.github.io/Telephone-Booth stable main" \
  | sudo tee /etc/apt/sources.list.d/telephone-booth.list
sudo apt update
sudo apt install telephone-booth
sudo editor /etc/phone-booth/env
sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
sudo systemctl restart telephone-booth.service telephone-booth-tailscale-serve.service
```

Once `telephone-booth` is installed, future `apt` cycles handle upgrades —
the package itself ships the `/etc/apt/sources.list.d/telephone-booth.list`
and matching keyring, so re-installing the source on every Pi by hand is
only required when bootstrapping a brand-new SD card without internet
during first boot.

### Manual one-off install

If you prefer to install a specific `.deb` directly (for testing a
pre-release, or on a Pi without internet during the first boot):

```sh
sudo apt install ./telephone-booth_0.1.0_arm64.deb
sudo editor /etc/phone-booth/env
sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
sudo systemctl restart telephone-booth.service telephone-booth-tailscale-serve.service
```

The package ships the APT source list and keyring as part of its assets,
so the first manual install automatically registers the repo for future
`apt upgrade` cycles.

## Upgrading

```sh
sudo apt update
sudo apt upgrade telephone-booth
sudo systemctl status telephone-booth.service
telephone-booth tailscale-status
```

State under `/var/lib/phone-booth` and local environment config under
`/etc/phone-booth/env` are preserved across upgrades (those files are
registered as dpkg conffiles).

### Automatic upgrades

The `telephone-booth` package `Recommends: unattended-upgrades` and ships
`/etc/apt/apt.conf.d/50-telephone-booth-unattended`, which configures
`unattended-upgrades` to pull updates from the project's APT origin. To
opt in on a Pi:

```sh
sudo apt install unattended-upgrades
sudo systemctl enable --now unattended-upgrades.service
```

`unattended-upgrades` runs daily via `apt-daily-upgrade.timer` and will
restart `telephone-booth.service` after upgrades (systemd notifies on the
`Type=notify` unit). To opt out, edit
`/etc/apt/apt.conf.d/50-telephone-booth-unattended` (it is a conffile, so
your edits survive package upgrades).

## Removing

```sh
sudo systemctl disable --now telephone-booth.service telephone-booth-tailscale-serve.service
sudo apt remove telephone-booth
```

Use `apt purge telephone-booth` only when you also intend to remove local
configuration managed by dpkg.
