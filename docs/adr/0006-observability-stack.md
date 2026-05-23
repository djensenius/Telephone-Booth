# ADR 0006 — Observability stack: VictoriaMetrics remote_write + Grafana, event persistence in operator DB

**Status:** accepted.

## Context

We need a thorough operational picture of every booth:

- **Host vitals** — CPU temp, load, memory, disk, network throughput,
  uptime, audio device, Tailscale link, Pi throttling — for live
  inspection and historical dashboards.
- **Event log** — every pickup, hangup, digit dialed, state transition,
  recording start/stop, upload start/complete, every operator HTTP call,
  every error — for after-the-fact troubleshooting and as the source of
  truth for "what happened during this call?".
- **Time-series metrics** — counters and histograms derived from the
  same events, plus the host vitals, for Grafana dashboards (calls per
  minute, recording duration histogram, upload failure rate, etc.).

We considered three shapes for the metrics pipeline:

1. **Booth scraped directly by Prometheus.** Requires Prometheus to reach
   each booth over Tailscale. Doesn't compose well when booths come and
   go from the tailnet — we'd need either static service discovery or a
   sidecar to register. Also forces Prometheus to live "close enough" to
   every booth's Tailscale namespace.
2. **Booth pushes Prometheus remote_write directly.** Avoids the scrape
   problem but means re-implementing the remote_write protocol in Rust
   (`prost`, `snappy`, timestamps, histograms, stale-marker handling).
   This is a meaningfully tricky correctness surface for a hobby
   single-node deployment, and the workspace lint set
   (`unsafe_code = forbid`, `pedantic`, `nursery`) makes the
   protobuf-codegen story noisier.
3. **Booth exposes `/metrics`, vmagent sidecar handles remote_write.**
   `vmagent` is a single ~10 MB Go binary from the VictoriaMetrics
   project. It scrapes the booth's `/metrics` over loopback and
   remote-writes to an external VictoriaMetrics instance over the public
   internet (with bearer auth). Battle-tested, handles retries +
   back-pressure + protobuf encoding for us, and runs as a separate
   systemd unit on the booth so its lifecycle is independent of the
   Rust process.

For event persistence we considered:

a. Persisting events in a local SQLite file on the booth, replicated to
   the operator out of band. Loses history when a booth disk fails.
b. Persisting events in VictoriaMetrics. VM is a time-series database,
   not an event store; high-cardinality fields like `recording_id` and
   `session_id` make terrible labels.
c. Persisting events in the operator Postgres (Prisma `BoothEvent` +
   `CallSession` tables) and deriving metrics from them on the booth via
   `metrics-exporter-prometheus`. Postgres already exists; event tables
   support cursor pagination, type filtering, and SQL exploration.

## Decision

We adopt option **3 + c**:

- The booth exposes `GET /metrics` (Prometheus text exposition) on its
  loopback debug listener. Bearer auth is skipped on this route because
  Tailscale ACLs already gate loopback access. The LAN HTTPS listener
  never sees `/metrics`.
- A `telephone-booth-vmagent` systemd unit, installed by the same `.deb`
  as the booth, scrapes `/metrics` every 10 s and remote-writes to the
  external VictoriaMetrics instance. Configuration lives in
  `/etc/phone-booth/vmagent.yaml` + `/etc/phone-booth/vmagent.env`;
  bearer credentials live in `/etc/phone-booth/vmagent-token` (mode
  0600).
- Every `TelemetryEvent` that crosses the booth telemetry bus is
  forwarded to the operator API (`POST /v1/events`, bulk + idempotent)
  and persisted in the operator's Postgres as a `BoothEvent` row, with
  derived `CallSession` rows for the pickup-to-hangup span.
- `SystemSample` snapshots are pushed to the operator
  (`PUT /v1/system`) for the Live System panel. They are **not**
  persisted in Postgres; VictoriaMetrics owns historical host-vitals
  time series.
- Grafana dashboards live in this repo under `dashboards/` as JSON
  alongside a small README describing how to import them.

## Consequences

**Good:**

- The Rust client stays small. No `prost`, `snap`, no hand-rolled
  remote_write client, no protobuf code generation in the workspace.
- vmagent's queue + retry + back-pressure is the failure domain for
  pushing metrics to VM, not the booth runtime.
- The Postgres event log is queryable (cursor pagination, type filters,
  joins onto `CallSession`) and survives booth disk failures.
- Metric cardinality is bounded by the rules in
  `docs/observability.md`: `booth_id`, `outcome`, `digit`, `from`/`to`
  state, `cpu` index, `mountpoint`, `iface`, `route`,
  `status_class`, `reason`, `window`, `flag`. No high-cardinality
  fields ever become labels.

**Trade-offs:**

- We depend on a second service running on every booth. The package's
  `Recommends:` (not `Depends:`) lets vmagent be missing without
  breaking install. The `postinst` script enables the unit only when
  `/usr/bin/vmagent` is present; reinstalling vmagent later picks up
  the existing config.
- Events live in two stores (Postgres durable, VictoriaMetrics derived).
  We treat Postgres as the source of truth and VM as a queryable
  rollup. Retention of the Postgres event table is deferred to a
  follow-up ADR once we see a week's worth of growth.
- The booth `/metrics` text-format response is loopback-only and
  unauthenticated. Anyone who can reach 127.0.0.1 on the booth can
  scrape it; Tailscale ACLs are the only gate for non-loopback access.

## Notes

- The runtime captures the booth's `MetricsHandle` returned from
  `booth_metrics::install_registry` and passes a render closure to
  `booth_debug::serve_with_handles`. The closure is then mounted on the
  loopback sub-router. The LAN sub-router never receives the renderer.
- The first cut of dashboards covers three boards: overview (host
  vitals + call rate), call activity (durations, outcomes, dialed
  digits), and audio. Adding new dashboards is purely a `dashboards/`
  PR; nothing in the Rust workspace needs to change.
