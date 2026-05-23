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
| `dashboards/*.json` + Grafana          | Three boards: overview, call activity, audio.                                                                |

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

vmagent (sidecar) ── scrape /metrics every 10 s ──▶ VictoriaMetrics ──▶ Grafana
```

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
| `uptime_seconds`          | Host uptime.                                                   |
| `throttling`              | Pi `vcgencmd get_throttled` flags. **Not populated in v1.**    |
| `tailscale`               | Connected state. **Not populated in v1.**                      |

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
| `booth_events_dropped_total`            | `reason`                            | Forwarder buffer overflow.                |
| `booth_network_receive_bytes_total`     | `iface`                             | sysinfo per-interface counters.           |
| `booth_network_transmit_bytes_total`    | `iface`                             | sysinfo per-interface counters.           |

### Gauges

| Metric                              | Labels                              |
| ----------------------------------- | ----------------------------------- |
| `booth_cpu_usage_ratio`             | `cpu` (`overall` or core index)     |
| `booth_load_average`                | `window` (`1m`, `5m`, `15m`)        |
| `booth_cpu_temperature_celsius`     | (none)                              |
| `booth_memory_used_bytes`           | (none)                              |
| `booth_memory_total_bytes`          | (none)                              |
| `booth_disk_used_bytes`             | `mountpoint`                        |
| `booth_disk_total_bytes`            | `mountpoint`                        |
| `booth_uptime_seconds`              | (none)                              |
| `booth_audio_input_dbfs`            | (none)                              |
| `booth_audio_output_dbfs`           | (none)                              |
| `booth_event_forward_inflight`      | (none)                              |

### Histograms

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
- **Operator UI shows stale system info** — check that
  `operator_forward.enabled = true` and the operator API is reachable.
  The booth logs `system_pusher: PUT /v1/system failed` when it can't
  reach the operator; the existing operator HTTP retry policy applies.
- **Postgres `BoothEvent` table is growing fast** — expected. Retention
  policy lands in a follow-up ADR once we see a week of production
  data.
