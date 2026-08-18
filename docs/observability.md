# Observability

This document describes how the booth surfaces operational data — host
vitals, structured events, and Prometheus metrics — and how those flow to
the operator console and Grafana.

For the design rationale see
[ADR 0006](adr/0006-observability-stack.md).

## Surfaces at a glance

| Surface                                | What's there                                                                                                 |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| Booth `GET /v1/system` (loopback)      | Latest [`SystemSnapshot`](#systemsnapshot-fields) as JSON. Bearer auth honored on LAN, skipped on loopback. |
| Booth `GET /v1/events?since=…`         | Catch-up replay of the telemetry ring buffer.                                                                |
| Booth `GET /v1/ws/telemetry`           | Live WebSocket stream of telemetry records.                                                                  |
| Booth `GET /metrics` (loopback only)   | Prometheus text exposition. **No auth** — Tailscale ACL gates the loopback front door.                       |
| Operator `GET /v1/system/current`      | Latest snapshot per booth (in-memory cache, broadcast over status WS).                                       |
| Operator `GET /v1/events`              | Cursor-paginated, type-filterable event history.                                                             |
| Operator `GET /v1/events/stream`       | Server-sent events feed (same-origin cookie auth only).                                                      |
| Operator `GET /v1/sessions`            | One row per pickup-to-hangup with derived outcome.                                                           |
| `dashboards/*.json` + Grafana          | Overview, call activity, audio, and environmental thermals dashboards.                                      |

## Data flow

```text
TelemetryBus  ─┬─▶ booth-debug (ring buffer, /v1/events, /v1/ws/telemetry)
               │
               ├─▶ booth-metrics
               │      ├─ system sampler ── publishes SystemSample events
               │      └─ telemetry consumer ── updates Prometheus registry
               │
               ├─▶ event_forwarder task ── POST /v1/events (bulk, idempotent)
               │
               └─▶ system_pusher task ── PUT /v1/system (latest snapshot)

booth-debug /metrics (loopback) ◀── booth-metrics::MetricsHandle::render

Open-Meteo ── HTTPS every 10 min ──▶ weather collector ──▶ Prometheus registry

vmagent (sidecar) ── scrape /metrics every 10 s ──▶ VictoriaMetrics ──▶ Grafana
```

## Deployment topology

Three independent hosts in the steady-state deployment:

1. **Booth host (Raspberry Pi).** Runs the `telephone-booth` Rust binary
   and the `telephone-booth-vmagent` systemd sidecar. The booth's
   `GET /metrics` endpoint is bound to loopback only — nothing outside
   the booth ever scrapes it directly. When weather collection is enabled,
   the Rust process also makes one outbound HTTPS request to Open-Meteo every
   10 minutes.
2. **Operator host.** Runs the operator API + Postgres + the web UI.
   Receives `POST /v1/events` and `PUT /v1/system` from each booth over
   the public internet (bearer auth). This is where the queryable event
   log lives.
3. **Metrics host.** Runs VictoriaMetrics (the time-series database) and
   Grafana. **Not** the booth, and not necessarily the same machine as
   the operator. Each booth's local `vmagent` pushes here via
   Prometheus `remote_write` over HTTPS, authenticated with the bearer
   token in `/etc/phone-booth/vmagent-token`.

```text
┌──────────────────────────────┐    POST /v1/events,       ┌───────────────────┐
│ Booth host (Pi)              │    PUT /v1/system         │ Operator host     │
│   telephone-booth (Rust) ────┼──────────────────────────▶│   API + Postgres  │
│   vmagent (sidecar) ─┐       │                           │   + web UI        │
└──────────────────────┼───────┘                           └───────────────────┘
                       │ Prometheus remote_write (HTTPS, bearer)
                       ▼
                ┌─────────────────────────────────┐
                │ Metrics host                    │
                │   VictoriaMetrics ◀── Grafana   │
                └─────────────────────────────────┘
```

### Why VictoriaMetrics and not Prometheus?

VictoriaMetrics is a drop-in Prometheus-compatible TSDB: it ingests the
Prometheus text exposition format that the booth emits, accepts
Prometheus `remote_write` from `vmagent`, and Grafana queries it with
PromQL. If you already operate a Prometheus server elsewhere you have
two equally supported options:

- **Keep VictoriaMetrics as written.** Point its `remote_write` URL at
  your VM instance (`/api/v1/write`). No code changes; this is the path
  the dashboards in `dashboards/` are tested against.
- **Use Prometheus instead of VictoriaMetrics.** Point `vmagent`'s
  `remote_write` URL at a Prometheus server with the
  `--web.enable-remote-write-receiver` flag enabled (Prometheus ≥ 2.33).
  The booth and dashboards do not change — they only speak the
  Prometheus ecosystem protocols.

What the booth does **not** support out of the box is direct scraping
of `/metrics` by a remote Prometheus over an arbitrary network — the
route is bound to loopback on purpose, so the scrape has to originate
on the booth itself. The `vmagent` sidecar exists precisely to bridge
that gap. If you would rather not run `vmagent`, you would need to add
a non-loopback bind for `/metrics` (with bearer auth) and accept the
Tailscale-ACL surface that comes with it. See
[ADR 0006](adr/0006-observability-stack.md) for the trade-offs we
weighed before landing on the sidecar.

### Scraping the booth from a Prometheus elsewhere on your tailnet

If you already run a Prometheus (or VictoriaMetrics, or any other
Prometheus-compatible TSDB) on another host in the same Tailscale
tailnet as the booth, you don't have to run the `vmagent` sidecar at
all — `tailscale serve` is already proxying the booth's loopback HTTP
listener on port 443 of the booth's MagicDNS name (see
[`docs/tailscale.md`](tailscale.md)), and the `/metrics` route lives on
that same loopback router. From a tailnet peer, the booth's metrics
are therefore reachable at:

```text
https://<booth-host>.<your-tailnet>.ts.net/metrics
```

Tailscale issues real Let's Encrypt certificates for MagicDNS names, so
no cert pinning or `tls_config: insecure_skip_verify: true` is needed —
default HTTPS verification works.

Add this to your remote Prometheus's `scrape_configs` (one entry per
booth, or use file/DNS-based service discovery if you have many):

```yaml
scrape_configs:
  - job_name: telephone-booth
    metrics_path: /metrics
    scheme: https
    scrape_interval: 15s
    scrape_timeout: 10s
    static_configs:
      - targets:
          # MagicDNS hostnames — one per booth.
          - booth-01.your-tailnet.ts.net
          - booth-02.your-tailnet.ts.net
        labels:
          job: booth
    # Promote the MagicDNS short name to a `booth_id` label so the
    # shipped Grafana dashboards (which key off `booth_id`) work
    # without further relabelling. The booth also sets `booth_id`
    # internally on every series; the relabel below only matters
    # for older booths that predate that label or for booths where
    # `observability.booth_id` doesn't match the MagicDNS name.
    relabel_configs:
      - source_labels: [__address__]
        regex: '([^.]+)\..*'
        target_label: instance
        replacement: '$1'
```

The Prometheus host needs Tailscale ACL permission to reach
`tag:booth:443`. The minimal ACL stanza, assuming you've tagged your
Prometheus host(s) with `tag:prometheus`:

```jsonc
{
  "acls": [
    {
      "action": "accept",
      "src": ["tag:prometheus"],
      "dst": ["tag:booth:443"]
    }
  ]
}
```

A few things to be aware of:

- **The `/metrics` route deliberately skips bearer auth.** Tailscale
  ACLs are the gate — anyone on the tailnet who can reach the booth
  on `:443` can read its metrics. This matches the rest of the
  loopback debug surface (see `crates/booth-debug/src/lib.rs`). Lock
  down `tag:booth:443` to your Prometheus / admin hosts in the ACL if
  that's not what you want.
- **`booth_id` is set by the booth itself** from
  `observability.booth_id` (see [`docs/configuration.md`](configuration.md#observability)).
  Make sure each booth has a unique value so multi-booth dashboards
  separate cleanly.
- **You can run both paths simultaneously.** Scraping over Tailscale
  and `remote_write` via `vmagent` are independent — useful while
  you're migrating. If you decide the Tailscale scrape is your only
  ingestion path, disable the sidecar on each booth with
  `sudo systemctl disable --now telephone-booth-vmagent.service` so
  it stops trying to push to a `remote_write` URL you no longer
  operate.
- **Scrape interval.** vmagent ships with `scrape_interval: 10s`; the
  example above uses `15s` to match Prometheus's default. Either is
  fine — the booth's metric set is small enough that 10–60 s
  intervals all stay well within budget.

### Host metrics with node_exporter

The booth's `/metrics` route only exposes the application's own
counters and gauges (see the [metrics catalog](#metrics-catalog)). It
does **not** export OS-level vitals such as per-core CPU, filesystem
fullness, network throughput, or load average. If you want those
alongside the booth metrics — for example to alert on a full SD card or
a thermally throttled Pi — install Prometheus
[`node_exporter`](https://github.com/prometheus/node_exporter) on the
booth host and scrape it from the same remote Prometheus.

Raspberry Pi OS is Debian-based, so the packaged build is the simplest
install — it ships a hardened systemd unit and survives `apt` upgrades:

```bash
sudo apt-get update
sudo apt-get install prometheus-node-exporter
```

By default the Debian package listens on `0.0.0.0:9100`. Keep it off the
public/LAN interfaces and let Tailscale be the gate, exactly like the
booth's own `/metrics` route — bind it to loopback and expose it over
the tailnet with `tailscale serve`. Edit
`/etc/default/prometheus-node-exporter`:

```text
# /etc/default/prometheus-node-exporter
ARGS="--web.listen-address=127.0.0.1:9100"
```

```bash
sudo systemctl restart prometheus-node-exporter
# The packaged Tailscale Serve unit publishes loopback :9100 on the tailnet
# as HTTPS :9100, alongside the booth's HTTPS :443 metrics/debug surface.
sudo systemctl restart telephone-booth-tailscale-serve.service
```

### GL.iNet router power and thermal telemetry

The installation's GL.iNet E5800 exposes OpenWrt host metrics through
`prometheus-node-exporter-lua` on port 9100. The collector under
[`packaging/openwrt/`](../packaging/openwrt/README.md) adds:

- Battery charge, temperature, voltage, current, health, cycle count, and
  abnormal state.
- Charger presence, online state, fast-charge state, charging status, and
  configured voltage/current limits.
- Every readable thermal zone, keyed by its kernel type and zone name.

The collector drops known invalid thermal sentinels rather than publishing
them as real temperatures. Prometheus retains the 15-second history. A
separate OpenWrt `procd` service pushes only the latest structured snapshot
to the Operator every 30 seconds using a telemetry-scoped token.

The Operator backend is not on the tailnet. Historical graph requests go
through Grafana's authenticated datasource proxy; port 9090 remains private.
See [ADR 0010](adr/0010-router-telemetry.md).

`node_exporter` is now reachable from any tailnet peer at:

```text
https://<booth-host>.<your-tailnet>.ts.net:9100/metrics
```

Grant the Prometheus host ACL permission to reach the new port — extend
the `tag:booth` stanza from the previous section:

```jsonc
{
  "acls": [
    {
      "action": "accept",
      "src": ["tag:prometheus"],
      "dst": ["tag:booth:443", "tag:booth:9100"]
    }
  ]
}
```

Then add a second scrape job to the remote Prometheus, next to the
`telephone-booth` job. Relabel `node_*` series with the same `booth_id`
the application metrics use so host and app series join cleanly in
Grafana:

```yaml
scrape_configs:
  - job_name: telephone-booth-node
    metrics_path: /metrics
    scheme: https
    scrape_interval: 15s
    scrape_timeout: 10s
    static_configs:
      - targets:
          # MagicDNS hostnames with the node_exporter port.
          - booth-01.your-tailnet.ts.net:9100
          - booth-02.your-tailnet.ts.net:9100
    relabel_configs:
      - source_labels: [__address__]
        regex: '([^.]+)\..*'
        target_label: booth_id
        replacement: '$1'
```

A few notes:

- **Loopback bind + `tailscale serve` keeps the trust model identical**
  to the booth's `/metrics` route: WireGuard encrypts the hop, the
  MagicDNS cert satisfies HTTPS verification (no `insecure_skip_verify`),
  and Tailscale ACLs — not bearer auth — gate access. Lock
  `tag:booth:9100` down to your Prometheus/admin hosts.
- **This is independent of the `vmagent` sidecar.** `node_exporter`
  publishes host vitals; `vmagent` (when enabled) pushes the booth's
  application metrics. Run either, both, or neither.
- **Off-the-shelf dashboards work.** The community
  [Node Exporter Full](https://grafana.com/grafana/dashboards/1860)
  board keys off `instance`/`job`; the `booth_id` relabel above also
  lets you cross-filter against the booth's own dashboards in
  `dashboards/`.

## Telemetry events

Every event on the booth telemetry bus is also forwarded to the operator
as a `BoothEvent` row. Events the forwarder emits include:

- `state_transition` — every state-machine transition.
- `call_started`, `call_ended` — derived by the runtime session tracker
  (UUIDv4 session id minted on pickup; outcome computed from the phase
  at hangup; see `crates/booth-bin/src/observability.rs`).
- `digit_dialed` — one row per completed pulse group, with the digit and
  the live session id.
- `recording_started`, `recording_stopped` — the latter carries
  `duration_ms` and `bytes`.
- `upload_started`, `upload_completed`, `upload_failed` — with timing
  and (for failures) a sanitized message.
- `operator_request`, `operator_response` — every outbound HTTP call to
  the operator API, except the `/v1/events` and `/v1/system` calls the
  forwarder itself makes (those are filtered to avoid an
  infinite-feedback loop).
- `error`, `log` — explicit error records and high-signal logs.
- `audio_device_change` — current input/output device names.
- `gpio_edge` — debounced edges with the originating pin role.
- `system_sample` — periodic `SystemSnapshot` (also pushed via
  `PUT /v1/system`).

Every outgoing event carries:

- `event_id = "{boot_id}:{telemetry_record_id}"` — used together with
  `boothId` as the unique key on the operator side so bulk inserts are
  idempotent under retry.
- `boot_id` — UUIDv4 minted at runtime start. Lets cross-reboot ordering
  fall back to wall clock instead of the per-process monotonic clock.
- `occurred_at` — booth wall-clock RFC3339 at publish time.
- `version` — the running `telephone-booth` client version (from
  `CARGO_PKG_VERSION`, e.g. `0.3.2`). Also included in the body of
  `PUT /v1/system` so operators can see at a glance which booth build is
  online.

## `SystemSnapshot` fields

The full type lives in `booth-hal::SystemSnapshot`. Every field is
optional so adding new metrics never breaks the wire format. The current
fields:

| Field                     | Notes                                                          |
| ------------------------- | -------------------------------------------------------------- |
| `temperature_celsius`     | Pi-only via `/sys/class/thermal/thermal_zone0/temp`.            |
| `cpu`                     | Overall and per-core usage ratio, load avg 1/5/15 minutes.     |
| `memory`                  | Used / total bytes.                                            |
| `disk[]`                  | One entry per mountpoint with used / total bytes.              |
| `network[]`               | Per-interface receive / transmit byte counters.                |
| `process`                 | Booth process rss / virt / cpu.                                |
| `audio`                   | Latest input/output device names.                              |
| `fan`                     | PWM command, thermal cooling state, and optional measured RPM. |
| `uptime_seconds`          | Host uptime.                                                   |
| `throttling`              | Pi `vcgencmd get_throttled` flags. **Not populated in v1.**    |
| `tailscale`               | Connected state. **Not populated in v1.**                      |
| `runtime_mode`            | One of `real`, `mock`, `simulator`. Tells the operator UI whether to show a `MOCK` / `SIM` badge. Mock and simulator booths do real network I/O against the operator, so this field is the only signal the backend has that the booth is non-production. `None` for older booths predating this field. |

Pi-only fields are `None` on macOS dev machines. The simulator and
host-runtime test path both exercise the sysinfo branch so dashboards
look populated on a developer laptop.

## Metrics catalog

Every series carries `booth_id` as a global label. Cardinality is bounded
by the table below; no free-form strings (sessionId, recording ids,
error messages) ever become labels.

### Counters

| Metric                                  | Labels                              | Source                                    |
| --------------------------------------- | ----------------------------------- | ----------------------------------------- |
| `booth_calls_started_total`             | (none)                              | `CallStarted` events.                     |
| `booth_calls_total`                     | `outcome`                           | `CallEnded` events, one of: `hung_up_before_dial`, `hung_up_during_prompt`, `hung_up_during_recording`, `hung_up_during_upload`, `recording_completed`, `recording_failed`, `upload_failed`, `operator_error`, `aborted`. |
| `booth_digits_dialed_total`             | `digit`                             | `DigitDialed` events; `digit` ∈ `0..9`.   |
| `booth_state_transitions_total`         | `from`, `to`                        | `StateTransition` events.                 |
| `booth_upload_failures_total`           | `reason`                            | `UploadFailed` events; small bounded enum. |
| `booth_operator_requests_total`         | `route`, `status_class`             | `OperatorResponse` events.                |
| `booth_errors_total`                    | `source`                            | `Error` events; `source` is a bounded enum. |
| `booth_network_receive_bytes_total`     | `iface`                             | sysinfo per-interface counters.           |
| `booth_network_transmit_bytes_total`    | `iface`                             | sysinfo per-interface counters.           |
| `booth_outdoor_weather_fetch_failures_total` | `source`, `reason`              | Failed Open-Meteo requests; `reason` is `transport`, `http`, `decode`, or `invalid_data`. |

### Gauges

| Metric                              | Labels                              |
| ----------------------------------- | ----------------------------------- |
| `booth_cpu_usage_ratio`             | (none) — overall host CPU usage ratio in `[0.0, 1.0]`. Per-core series is collected in `SystemSnapshot` but is not yet exported as Prometheus labels. |
| `booth_load_average`                | `window` (`1m`, `5m`, `15m`)        |
| `booth_cpu_temperature_celsius`     | (none)                              |
| `booth_memory_used_bytes`           | (none)                              |
| `booth_memory_total_bytes`          | (none)                              |
| `booth_disk_used_bytes`             | `mountpoint`                        |
| `booth_disk_total_bytes`            | `mountpoint`                        |
| `booth_uptime_seconds`              | (none)                              |
| `booth_fan_available`                | (none) — 1 when a kernel fan interface exists, otherwise 0. |
| `booth_fan_tachometer_available`     | (none) — 1 when the current sample contains measured RPM. |
| `booth_fan_commanded_on`             | (none) — 1 when the kernel requests non-zero PWM, otherwise 0. |
| `booth_fan_pwm_ratio`                | (none) — requested duty ratio in `[0.0, 1.0]`. |
| `booth_fan_speed_rpm`                | (none) — measured RPM, or `NaN` without tachometer feedback. |
| `booth_fan_cooling_state`            | (none) — active kernel thermal cooling state. |
| `booth_fan_max_cooling_state`        | (none) — highest cooling state supported by the driver. |
| `booth_outdoor_weather_temperature_celsius` | `source` — modeled outdoor air temperature. |
| `booth_outdoor_weather_apparent_temperature_celsius` | `source` — modeled apparent temperature. |
| `booth_outdoor_weather_relative_humidity_percent` | `source` — relative humidity in `[0, 100]`. |
| `booth_outdoor_weather_cloud_cover_percent` | `source` — total cloud cover in `[0, 100]`. |
| `booth_outdoor_weather_precipitation_millimeters` | `source` — precipitation in the provider's current interval. |
| `booth_outdoor_weather_wind_speed_kilometers_per_hour` | `source` — wind speed at 10 metres. |
| `booth_outdoor_weather_shortwave_radiation_watts_per_square_meter` | `source` — downward shortwave radiation. |
| `booth_outdoor_weather_code`         | `source` — numeric WMO weather interpretation code. |
| `booth_outdoor_weather_condition_info` | `source`, `condition` — one-hot bounded condition category. |
| `booth_outdoor_weather_observation_timestamp_seconds` | `source` — provider observation time as Unix seconds. |
| `booth_outdoor_weather_last_success_timestamp_seconds` | `source` — booth receipt time for the latest valid sample. |
| `booth_outdoor_weather_fetch_success` | `source` — 1 when the latest fetch succeeded, otherwise 0. |
| `booth_audio_peak_amplitude`        | `channel` (`input`, `output`) — linear peak amplitude in `[0.0, 1.0]` from the last `AudioLevel` event. |
| `booth_audio_rms_amplitude`         | `channel` (`input`, `output`) — linear RMS amplitude in `[0.0, 1.0]` from the last `AudioLevel` event. |
| `booth_info`                        | `mode` (`real`, `mock`, `simulator`) — always 1.0; lets Grafana / VictoriaMetrics filter dashboards by runtime mode (e.g. `booth_calls_total * on(booth_id) group_left() booth_info{mode="real"}` to exclude mock / simulator booths). Cardinality is bounded to 3. |

`AudioLevel` events are emitted at 20 Hz per channel side (input, output) for
as long as an audio stream is open, independent of the device's channel
count, and including while the signal is silent. When a stream is torn down the
meter flushes its partial block and then publishes one final reading with
`peak` and `rms` at `0.0`. That trailing reading is what returns VU meters —
in the debug UI, `GET /audio`, and the operator clients — to the floor;
without it a consumer cannot tell "silent" from "no longer reporting" and
would hold the last non-zero level indefinitely. Absence of events still
means "no audio path is open", so clients should also treat a stale feed as
unknown rather than as a level.

## Fan monitoring

On Linux, `booth-metrics` discovers the kernel `pwmfan` hwmon device and the
`pwm-fan` thermal cooling device. Every system sample adds a `fan` object when
either interface exists:

| Field               | Meaning |
| ------------------- | ------- |
| `commandedOn`       | Whether the kernel requested a PWM value above zero. |
| `pwmRatio`          | Requested PWM duty as a ratio from 0.0 to 1.0. |
| `coolingState`      | Active thermal state selected from the overlay's curve. |
| `maxCoolingState`   | Maximum state supported by that curve. |
| `rpm`               | Measured rotor speed, only when the kernel driver exposes tachometer feedback. |

The reference Noctua wiring intentionally leaves the green tachometer wire
disconnected, so `rpm` is absent and `booth_fan_speed_rpm` reports `NaN`.
`commandedOn` and `pwmRatio` describe the electrical command; they cannot
confirm that the rotor is physically moving. The overview dashboard therefore
labels PWM speed as a command rather than measured RPM.

Prometheus gauges cannot be deleted from the in-process registry. Every sample
therefore updates the availability gauges and writes `NaN` to unavailable
optional values. This prevents a previous PWM command or RPM reading from
remaining visible after a transient sysfs failure or device removal.

Inspect the live JSON directly:

```sh
curl -s http://127.0.0.1:8080/v1/system | jq .fan
```

Or inspect the Prometheus series:

```sh
curl -s http://127.0.0.1:8080/metrics | grep '^booth_fan_'
```

## Histograms

| Metric                                   | Labels    |
| ---------------------------------------- | --------- |
| `booth_recording_duration_seconds`       | (none)    |
| `booth_upload_duration_seconds`          | `outcome` |
| `booth_upload_bytes`                     | (none)    |
| `booth_operator_request_duration_seconds`| `route`   |

Routes are template-normalized (`/v1/events`, not the full URL with
query). `reason` and `source` come from small bounded enums so they
never explode cardinality. Default histogram buckets are
`metrics-exporter-prometheus`'s defaults (exponential out to several
seconds); tune in `booth-metrics` if dashboards need finer resolution.

## Outdoor weather

When `[weather]` is enabled, `booth-bin` polls Open-Meteo's current forecast
API over HTTPS and writes modeled outdoor context directly to the Prometheus
registry. The default cadence is 600 seconds, so one booth makes 144 requests
per day. The collector uses the workspace's existing `reqwest` client; no new
HTTP dependency or operator-backend path is involved.

The request asks for:

- Air temperature and apparent temperature in Celsius.
- Relative humidity and total cloud cover in percent.
- Interval precipitation in millimetres.
- Wind speed at 10 metres in kilometres per hour.
- Downward shortwave radiation in watts per square metre.
- The numeric WMO weather code and provider observation time.

Open-Meteo combines and interpolates weather-model output. These values are
regional modeled conditions, **not** measurements inside the booth enclosure.
Use them to correlate outside conditions with Pi CPU, router battery, modem
zones, and fan command; do not use them as evidence of enclosure safety.

The WMO code maps to one of 17 stable condition labels, including `clear_sky`,
`partly_cloudy`, `rain`, `snowfall`, and `thunderstorm`. The
`booth_outdoor_weather_condition_info` metric publishes the active condition
as a one-hot series so Grafana can display text without turning arbitrary
provider strings into labels. `source` is fixed to `open_meteo`.

On a successful request the collector validates all numeric ranges, updates
the weather gauges, records provider and receipt timestamps, and sets
`booth_outdoor_weather_fetch_success` to 1. A transport, HTTP, decode, or
validation failure:

- Leaves the last valid environmental values intact.
- Sets `booth_outdoor_weather_fetch_success` to 0.
- Increments `booth_outdoor_weather_fetch_failures_total` with one bounded
  `reason` label.

Use this PromQL expression for sample age:

```promql
time() - booth_outdoor_weather_last_success_timestamp_seconds
```

The shipped Thermals dashboard warns after 20 minutes and marks the feed
critical after one hour.

Coordinates are required when weather is enabled. The reference deployment
uses rounded coordinates so the request does not need street-level precision.
Coordinates never appear in metric labels or collector logs, but they are sent
to Open-Meteo. Open-Meteo's privacy terms say API server logs may contain
coordinates for up to 90 days.

Open-Meteo API data are licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Displays must link
to [Open-Meteo](https://open-meteo.com/); the checked-in Thermals dashboard
includes a **Weather data by Open-Meteo.com** dashboard link.

## Configuration

The booth observability stack is configured under `[observability]` in
`config.toml`. See [`docs/configuration.md`](configuration.md#observability)
for the full table; key knobs:

| Key                                            | Default     | Purpose                                                                 |
| ---------------------------------------------- | ----------- | ----------------------------------------------------------------------- |
| `observability.enabled`                        | `true`      | Master switch. When `false`, no metrics registry, no system sampler.    |
| `observability.booth_id`                       | `"booth-01"`| Embedded as `booth_id` label on every series + every operator event.   |
| `observability.sample_interval_ms`             | `5000`      | How often the system sampler runs.                                      |
| `observability.operator_forward.enabled`       | `true`      | Push events + system snapshots to the operator.                         |
| `observability.operator_forward.batch_max`     | `200`       | Maximum events per `POST /v1/events` batch.                             |
| `observability.operator_forward.flush_interval_ms` | `2000`  | Force a flush every N ms even if the batch isn't full.                  |
| `observability.operator_forward.buffer_max`    | `4096`      | Hard cap on the in-memory queue; drop-oldest on overflow.               |

A small subset of these settings can be overridden via environment
variables (single underscore between segments):

- `BOOTH_OBSERVABILITY_ENABLED` — master kill switch.
- `BOOTH_OBSERVABILITY_BOOTH_ID` — e.g. `BOOTH_OBSERVABILITY_BOOTH_ID=booth-42`.
- `BOOTH_OBSERVABILITY_FORWARD_ENABLED` — toggle the operator forwarder.

All other observability settings are config-file only.

Weather collection is configured separately:

| Key                            | Default | Purpose |
| ------------------------------ | ------- | ------- |
| `weather.enabled`              | `false` | Start the Open-Meteo collector; also requires observability to be enabled. |
| `weather.latitude`             | unset   | Deployment latitude in `[-90, 90]`; required when enabled. |
| `weather.longitude`            | unset   | Deployment longitude in `[-180, 180]`; required when enabled. |
| `weather.poll_interval_seconds`   | `600`   | Poll cadence; minimum 60 seconds. |
| `weather.request_timeout_seconds` | `15`    | Per-request HTTP timeout; range 1–60 seconds. |

Every weather key has a matching environment override:
`BOOTH_WEATHER_ENABLED`, `BOOTH_WEATHER_LATITUDE`,
`BOOTH_WEATHER_LONGITUDE`, `BOOTH_WEATHER_POLL_INTERVAL_SECONDS`, and
`BOOTH_WEATHER_REQUEST_TIMEOUT_SECONDS`.

Remote-write configuration is **not** in `config.toml`. vmagent reads
its scrape config from `/etc/phone-booth/vmagent.yaml` and its
remote_write URL from `/etc/phone-booth/vmagent.env`
(`BOOTH_VM_REMOTE_WRITE_URL`). Bearer credentials live in
`/etc/phone-booth/vmagent-token` (`0600 root:root`).

## Packaging

The `telephone-booth` `.deb` installs the vmagent sidecar config but
lists `vmagent` as a `Recommends:` rather than a hard dependency so the
booth still installs on hosts that don't yet have vmagent available.
The `postinst` script enables and starts
`telephone-booth-vmagent.service` only when `/usr/bin/vmagent` exists.
Install vmagent with `apt-get install vmagent` (or the VictoriaMetrics
.deb) and restart the unit to start pushing metrics.

## Grafana dashboards

JSON dashboards live in `dashboards/` and are intended to be imported
into a Grafana that already has a VictoriaMetrics data source named
`VictoriaMetrics`. The folder ships:

- `booth-overview.json` — Pi vitals + call rate + uptime.
- `booth-call-activity.json` — calls/min, digit histogram, recording
  duration histogram, upload-failure rate.
- `booth-audio.json` — input/output dBFS, device changes.
- `booth-thermals.json` — Pi, router battery and modem zones, fan command
  and optional RPM, plus modeled outdoor weather and freshness.

See `dashboards/README.md` for import instructions.

## Clock semantics

Three timestamps are relevant when investigating an event:

1. **`occurred_at`** — booth wall clock at publish. Subject to NTP jitter
   and clock skew between booths.
2. **`received_at`** — operator's wall clock when the API accepted the
   event. Used as the default sort order in the operator UI because it
   reflects the operator's perspective of "when did we hear about it?".
3. **VictoriaMetrics scrape time** — vmagent stamps each sample with
   the scrape instant on the vmagent host. Grafana dashboards therefore
   render the metric timeline against vmagent's wall clock.

Weather adds two explicit timestamps inside the metric values:
`booth_outdoor_weather_observation_timestamp_seconds` is the provider's
model timestamp, while `booth_outdoor_weather_last_success_timestamp_seconds`
is the booth's wall clock after a valid response is received. Grafana uses the
latter for freshness because it answers the operational question "when did
this booth last obtain valid weather data?".

A booth with a badly skewed clock will still produce ordered timelines
in Grafana (vmagent stamps replace the local timestamp) and ordered
event listings in the operator UI (`received_at` is monotonic on the
operator).

## Troubleshooting

- **`/metrics` returns 404** — observability is disabled. Set
  `observability.enabled = true` and restart the booth, or check
  `journalctl -u telephone-booth -e` for `failed to install metrics
  registry`.
- **No data in Grafana** — vmagent isn't running or can't reach the
  remote_write endpoint. Check `journalctl -u telephone-booth-vmagent`.
  Then `curl -s http://127.0.0.1:8429/api/v1/status/config` on the
  booth to confirm vmagent loaded the scrape config.
- **Weather panels show no data** — confirm `weather.enabled = true`,
  both coordinates are configured, and observability is enabled. Inspect
  `journalctl -u telephone-booth -e` for `outdoor weather fetch failed`,
  then query `booth_outdoor_weather_fetch_success` and
  `booth_outdoor_weather_fetch_failures_total`.
- **Weather is stale but the last values still render** — this is expected
  failure behavior. The collector preserves the last valid values while
  setting fetch success to 0. Check the dashboard's freshness stat or subtract
  `booth_outdoor_weather_last_success_timestamp_seconds` from `time()`.
- **Remote Prometheus scrape returns 404 / connection refused** —
  `tailscale serve` isn't proxying the loopback listener, or the
  booth's debug surface is down. From the Prometheus host run
  `curl -fsS https://<booth>.<your-tailnet>.ts.net/metrics | head` and
  compare with `curl -fsS http://127.0.0.1:8080/metrics | head` on the
  booth itself. If the second works and the first does not, re-run
  `sudo /usr/share/telephone-booth/setup-tailscale-serve.sh` on the
  booth and confirm `sudo tailscale serve status` lists port 443 → 8080
  and port 9100 → 9100. The systemd unit retries automatically when
  Tailscale is still connecting during boot.
- **Remote Prometheus scrape returns 403 / TLS errors** — Tailscale
  ACL is blocking the Prometheus host, or the host isn't on the
  tailnet. Confirm the ACL grants the Prometheus host (or its tag)
  access to `tag:booth:443` and that `tailscale status` on the
  Prometheus host shows it as online.
- **Operator UI shows stale system info** — check that
  `operator_forward.enabled = true` and the operator API is reachable.
  The booth logs `system_pusher: PUT /v1/system failed` when it can't
  reach the operator; the existing operator HTTP retry policy applies.
- **Postgres `BoothEvent` table is growing fast** — expected. Retention
  policy lands in a follow-up ADR once we see a week of production
  data.
